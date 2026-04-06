// VirtIO network device backend with Linux TAP.

use std::io;
use std::os::unix::io::RawFd;

use crate::device::{read_config_sub, VirtioDevice, VIRTIO_F_VERSION_1};
use crate::queue::{VirtQueue, VRING_DESC_F_WRITE};

const VIRTIO_DEVICE_NET: u32 = 1;

// Feature bits.
const VIRTIO_NET_F_MAC: u64 = 1 << 5;
const VIRTIO_NET_F_STATUS: u64 = 1 << 16;

// Net header size for VirtIO v1 (modern).
// Includes num_buffers field: flags(1) + gso_type(1) + hdr_len(2)
// + gso_size(2) + csum_start(2) + csum_offset(2) + num_buffers(2) = 12.
pub const VIRTIO_NET_HDR_SIZE: usize = 12;

// TAP ioctl constants (Linux x86_64 / generic).
const TUNSETIFF: libc::c_ulong = 0x400454ca;
const IFF_TAP: libc::c_short = 0x0002;
const IFF_NO_PI: libc::c_short = 0x1000;

#[repr(C)]
struct Ifreq {
    ifr_name: [u8; libc::IFNAMSIZ],
    ifr_flags: libc::c_short,
    _pad: [u8; 22],
}

/// Open a TAP device by name. Returns the file descriptor.
pub fn tap_open(ifname: &str) -> io::Result<RawFd> {
    let fd = unsafe {
        libc::open(
            b"/dev/net/tun\0".as_ptr() as *const libc::c_char,
            libc::O_RDWR | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut ifr: Ifreq = unsafe { std::mem::zeroed() };
    let name_bytes = ifname.as_bytes();
    let copy_len = name_bytes.len().min(libc::IFNAMSIZ - 1);
    ifr.ifr_name[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
    ifr.ifr_flags = IFF_TAP | IFF_NO_PI;

    let ret = unsafe {
        libc::ioctl(fd, TUNSETIFF, &ifr as *const Ifreq)
    };
    if ret < 0 {
        let err = io::Error::last_os_error();
        unsafe { libc::close(fd); }
        return Err(err);
    }

    Ok(fd)
}

/// Parse a MAC address string "XX:XX:XX:XX:XX:XX".
pub fn parse_mac(s: &str) -> Result<[u8; 6], String> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return Err(format!("invalid MAC: {}", s));
    }
    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16)
            .map_err(|e| format!("invalid MAC octet '{}': {}", part, e))?;
    }
    Ok(mac)
}

/// VirtIO network device backed by a Linux TAP interface.
pub struct VirtioNet {
    tap_fd: RawFd,
    mac: [u8; 6],
}

unsafe impl Send for VirtioNet {}

impl VirtioNet {
    /// Create a new VirtioNet from an already-opened TAP fd
    /// and a MAC address.
    pub fn new(tap_fd: RawFd, mac: [u8; 6]) -> Self {
        Self { tap_fd, mac }
    }

    /// Convenience: open TAP by name, parse MAC string.
    pub fn open(ifname: &str, mac_str: &str) -> io::Result<Self> {
        let tap_fd = tap_open(ifname)?;
        let mac = parse_mac(mac_str).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidInput, e)
        })?;
        Ok(Self { tap_fd, mac })
    }

    /// Transmit a single packet from the TX queue descriptor
    /// chain to the TAP device.
    fn tx_one(
        &self,
        chain: &[crate::queue::Desc],
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> u32 {
        // Gather all readable (device-readable = driver-written)
        // descriptors. First VIRTIO_NET_HDR_SIZE bytes are the
        // virtio net header which we skip when writing to TAP.
        let mut iov: Vec<libc::iovec> = Vec::new();
        let mut total_len = 0usize;

        for desc in chain {
            if desc.flags & VRING_DESC_F_WRITE != 0 {
                continue;
            }
            let guest_off = match desc.addr.checked_sub(ram_base) {
                Some(o) if o + (desc.len as u64) <= ram_size => o,
                _ => return 0,
            };
            iov.push(libc::iovec {
                iov_base: unsafe {
                    ram.add(guest_off as usize) as *mut libc::c_void
                },
                iov_len: desc.len as usize,
            });
            total_len += desc.len as usize;
        }

        if total_len <= VIRTIO_NET_HDR_SIZE || iov.is_empty() {
            return 0;
        }

        // Skip the virtio net header by adjusting the first iov.
        let mut skip = VIRTIO_NET_HDR_SIZE;
        let mut start = 0usize;
        while skip > 0 && start < iov.len() {
            if iov[start].iov_len <= skip {
                skip -= iov[start].iov_len;
                start += 1;
            } else {
                iov[start].iov_base = unsafe {
                    (iov[start].iov_base as *mut u8).add(skip)
                        as *mut libc::c_void
                };
                iov[start].iov_len -= skip;
                skip = 0;
            }
        }

        if start >= iov.len() {
            return 0;
        }

        let payload_len: usize = iov[start..].iter().map(|v| v.iov_len).sum();
        let written = unsafe {
            libc::writev(
                self.tap_fd,
                iov[start..].as_ptr(),
                (iov.len() - start) as libc::c_int,
            )
        };

        if written < 0 {
            eprintln!(
                "virtio-net TX: writev failed (fd={}, {} bytes): {}",
                self.tap_fd,
                payload_len,
                io::Error::last_os_error()
            );
            return 0;
        }

        eprintln!(
            "virtio-net TX: {} bytes -> TAP fd={}",
            written, self.tap_fd
        );

        (written as u32).wrapping_add(VIRTIO_NET_HDR_SIZE as u32)
    }
}

impl VirtioDevice for VirtioNet {
    fn device_id(&self) -> u32 {
        VIRTIO_DEVICE_NET
    }

    fn num_queues(&self) -> usize {
        2
    }

    fn features(&self) -> u64 {
        VIRTIO_F_VERSION_1 | VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS
    }

    fn config_read(&self, offset: u64, size: u32) -> u64 {
        // struct virtio_net_config {
        //   u8  mac[6];      // offset 0
        //   u16 status;      // offset 6
        //   u16 max_virtqueue_pairs;  // offset 8
        // }
        match offset {
            0..=5 => read_config_sub(&self.mac, offset as usize, size),
            6..=7 => {
                let status: u16 = 1; // VIRTIO_NET_S_LINK_UP
                let bytes = status.to_le_bytes();
                read_config_sub(&bytes, (offset - 6) as usize, size)
            }
            8..=9 => {
                let pairs: u16 = 1;
                let bytes = pairs.to_le_bytes();
                read_config_sub(&bytes, (offset - 8) as usize, size)
            }
            _ => 0,
        }
    }

    unsafe fn handle_queue(
        &mut self,
        queue_idx: usize,
        queue: &mut VirtQueue,
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> u32 {
        match queue_idx {
            1 => self.handle_tx(queue, ram, ram_base, ram_size),
            _ => 0,
        }
    }

    fn rx_fd(&self) -> Option<i32> {
        Some(self.tap_fd)
    }
}

/// Fill one RX descriptor with [virtio_net_hdr][packet].
/// Called by the MMIO transport's RX thread.
///
/// # Safety
/// Caller must ensure `ram` is valid for
/// [`ram_base`, `ram_base + ram_size`).
pub unsafe fn fill_rx_queue(
    packet: &[u8],
    queue: &mut VirtQueue,
    ram: *mut u8,
    ram_base: u64,
    ram_size: u64,
) -> u32 {
    let avail_idx = queue.read_avail_idx(ram, ram_base, ram_size);
    if queue.last_avail_idx == avail_idx {
        eprintln!(
            "virtio-net fill_rx: no avail bufs (last_avail={}, avail_idx={})",
            queue.last_avail_idx, avail_idx
        );
        return 0;
    }

    let desc_head = queue.read_avail_ring(
        queue.last_avail_idx,
        ram,
        ram_base,
        ram_size,
    );
    let chain = queue.walk_chain(desc_head, ram, ram_base, ram_size);

    let total_payload = VIRTIO_NET_HDR_SIZE + packet.len();
    let hdr = [0u8; VIRTIO_NET_HDR_SIZE];
    let full_data: Vec<u8> =
        hdr.iter().chain(packet.iter()).copied().collect();

    let mut data_off = 0usize;
    let mut total_written = 0u32;

    for desc in &chain {
        if desc.flags & VRING_DESC_F_WRITE == 0 {
            continue;
        }
        let guest_off = match desc.addr.checked_sub(ram_base) {
            Some(o) if o + (desc.len as u64) <= ram_size => o,
            _ => break,
        };
        let remaining = total_payload.saturating_sub(data_off);
        let copy_len = remaining.min(desc.len as usize);
        if copy_len > 0 {
            std::ptr::copy_nonoverlapping(
                full_data[data_off..].as_ptr(),
                ram.add(guest_off as usize),
                copy_len,
            );
            data_off += copy_len;
            total_written += copy_len as u32;
        }
        if data_off >= total_payload {
            break;
        }
    }

    if total_written == 0 {
        return 0;
    }

    let used_idx = {
        let off = queue.used_addr + 2 - ram_base;
        if off + 2 > ram_size {
            return 0;
        }
        (ram.add(off as usize) as *const u16).read_unaligned()
    };

    queue.write_used(
        used_idx,
        desc_head as u32,
        total_written,
        ram,
        ram_base,
        ram_size,
    );
    let new_used = used_idx.wrapping_add(1);
    queue.write_used_idx(new_used, ram, ram_base, ram_size);
    queue.last_avail_idx = queue.last_avail_idx.wrapping_add(1);

    1
}

impl VirtioNet {
    /// Process pending TX descriptors → write to TAP.
    unsafe fn handle_tx(
        &self,
        queue: &mut VirtQueue,
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> u32 {
        let avail_idx = queue.read_avail_idx(ram, ram_base, ram_size);
        let mut processed = 0u32;
        let mut used_idx = {
            let off = queue.used_addr + 2 - ram_base;
            if off + 2 > ram_size {
                return 0;
            }
            (ram.add(off as usize) as *const u16).read_unaligned()
        };

        while queue.last_avail_idx != avail_idx {
            let desc_head = queue.read_avail_ring(
                queue.last_avail_idx,
                ram,
                ram_base,
                ram_size,
            );
            let chain =
                queue.walk_chain(desc_head, ram, ram_base, ram_size);
            let written = self.tx_one(&chain, ram, ram_base, ram_size);
            queue.write_used(
                used_idx,
                desc_head as u32,
                written,
                ram,
                ram_base,
                ram_size,
            );
            used_idx = used_idx.wrapping_add(1);
            queue.last_avail_idx = queue.last_avail_idx.wrapping_add(1);
            processed += 1;
        }

        queue.write_used_idx(used_idx, ram, ram_base, ram_size);
        processed
    }

}

impl Drop for VirtioNet {
    fn drop(&mut self) {
        if self.tap_fd >= 0 {
            unsafe {
                libc::close(self.tap_fd);
            }
        }
    }
}


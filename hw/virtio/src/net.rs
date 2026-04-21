// VirtIO network device with TAP and pipe backends.

use std::os::unix::io::RawFd;

use crate::queue::{VirtQueue, VRING_DESC_F_WRITE};
use crate::VirtioDevice;

// VirtIO net feature bits.
const VIRTIO_NET_F_MAC: u64 = 1 << 5;
const VIRTIO_NET_F_MRG_RXBUF: u64 = 1 << 15;
const VIRTIO_NET_F_STATUS: u64 = 1 << 16;
const VIRTIO_F_VERSION_1: u64 = 1 << 32;

// VirtIO net header sizes (bytes).
pub const VIRTIO_NET_HDR_SIZE_BASE: usize = 10;
pub const VIRTIO_NET_HDR_SIZE_MRG: usize = 12;

const VIRTIO_DEVICE_NET: u32 = 1;

/// Default MAC address: 52:54:00:12:34:56.
pub const DEFAULT_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];

// ── Backend trait ──────────────────────────────────────

/// Pluggable network I/O backend.
pub trait NetBackend: Send {
    fn fd(&self) -> RawFd;
    fn read_packet(&self, buf: &mut [u8]) -> std::io::Result<usize>;
    fn write_packet(&self, buf: &[u8]) -> std::io::Result<usize>;
}

// ── TAP backend ────────────────────────────────────────

/// Linux TAP device backend.
pub struct TapBackend {
    fd: RawFd,
}

impl TapBackend {
    /// Open (or create) a TAP device with `ifname`.
    pub fn new(ifname: &str) -> std::io::Result<Self> {
        // SAFETY: opening /dev/net/tun is a standard
        // TAP-creation syscall sequence.
        let fd = unsafe {
            libc::open(c"/dev/net/tun".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC)
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }

        // Build ifreq for TUNSETIFF.
        let mut ifr = [0u8; 40]; // struct ifreq
        let name_bytes = ifname.as_bytes();
        let copy_len = name_bytes.len().min(15);
        ifr[..copy_len].copy_from_slice(&name_bytes[..copy_len]);
        // ifr_flags at offset 16: IFF_TAP | IFF_NO_PI
        let flags: u16 = 0x0002 | 0x1000; // IFF_TAP | IFF_NO_PI
        ifr[16..18].copy_from_slice(&flags.to_le_bytes());

        // SAFETY: ioctl(TUNSETIFF) configures the TAP
        // device on the open fd.
        let ret = unsafe {
            libc::ioctl(
                fd,
                0x400454CA_u64 as libc::c_ulong, // TUNSETIFF
                ifr.as_ptr(),
            )
        };
        if ret < 0 {
            let err = std::io::Error::last_os_error();
            // SAFETY: closing an fd we own.
            unsafe { libc::close(fd) };
            return Err(err);
        }

        Ok(Self { fd })
    }
}

impl NetBackend for TapBackend {
    fn fd(&self) -> RawFd {
        self.fd
    }

    fn read_packet(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        // SAFETY: read from our owned TAP fd into the
        // caller-supplied buffer.
        let n = unsafe {
            libc::read(
                self.fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }

    fn write_packet(&self, buf: &[u8]) -> std::io::Result<usize> {
        // SAFETY: write to our owned TAP fd from the
        // caller-supplied buffer.
        let n = unsafe {
            libc::write(self.fd, buf.as_ptr() as *const libc::c_void, buf.len())
        };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
}

impl Drop for TapBackend {
    fn drop(&mut self) {
        // SAFETY: closing an fd we exclusively own.
        unsafe { libc::close(self.fd) };
    }
}

// ── Pipe backend (for testing) ─────────────────────────

/// Loopback pipe backend for unit tests.
pub struct PipeBackend {
    read_fd: RawFd,
    write_fd: RawFd,
}

impl PipeBackend {
    pub fn new() -> std::io::Result<Self> {
        let mut fds = [0i32; 2];
        // SAFETY: pipe() writes two fds into `fds`.
        let ret = unsafe { libc::pipe(fds.as_mut_ptr()) };
        if ret < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            read_fd: fds[0],
            write_fd: fds[1],
        })
    }

    /// Write a packet that can be received via
    /// `read_packet`.
    pub fn inject_packet(&self, data: &[u8]) -> std::io::Result<usize> {
        // SAFETY: write to our owned pipe fd.
        let n = unsafe {
            libc::write(
                self.write_fd,
                data.as_ptr() as *const libc::c_void,
                data.len(),
            )
        };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
}

impl NetBackend for PipeBackend {
    fn fd(&self) -> RawFd {
        self.read_fd
    }

    fn read_packet(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        // SAFETY: read from our owned pipe fd.
        let n = unsafe {
            libc::read(
                self.read_fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }

    fn write_packet(&self, buf: &[u8]) -> std::io::Result<usize> {
        // SAFETY: write to our owned pipe fd.
        let n = unsafe {
            libc::write(
                self.write_fd,
                buf.as_ptr() as *const libc::c_void,
                buf.len(),
            )
        };
        if n < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
}

impl Drop for PipeBackend {
    fn drop(&mut self) {
        // SAFETY: closing fds we exclusively own.
        unsafe {
            libc::close(self.read_fd);
            libc::close(self.write_fd);
        }
    }
}

// ── MAC address parser ─────────────────────────────────

/// Parse a MAC address in "XX:XX:XX:XX:XX:XX" format.
pub fn parse_mac(s: &str) -> Result<[u8; 6], String> {
    if s.is_empty() {
        return Err("empty MAC string".into());
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return Err(format!("expected 6 octets, got {}", parts.len()));
    }
    let mut mac = [0u8; 6];
    for (i, part) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(part, 16)
            .map_err(|e| format!("bad octet '{}': {}", part, e))?;
    }
    Ok(mac)
}

// ── VirtIO net device ──────────────────────────────────

/// VirtIO network device.
pub struct VirtioNet {
    pub backend: Box<dyn NetBackend>,
    pub mac: [u8; 6],
    pub features: u64,
    pub acked_features: u64,
}

impl VirtioNet {
    pub fn new(backend: Box<dyn NetBackend>, mac: [u8; 6]) -> Self {
        Self {
            backend,
            mac,
            features: VIRTIO_F_VERSION_1
                | VIRTIO_NET_F_MAC
                | VIRTIO_NET_F_STATUS
                | VIRTIO_NET_F_MRG_RXBUF,
            acked_features: 0,
        }
    }

    pub fn new_default(backend: Box<dyn NetBackend>) -> Self {
        Self::new(backend, DEFAULT_MAC)
    }

    /// Virtio-net header size based on negotiated
    /// features.
    pub fn hdr_size(&self) -> usize {
        if self.acked_features & VIRTIO_NET_F_MRG_RXBUF != 0 {
            VIRTIO_NET_HDR_SIZE_MRG
        } else {
            VIRTIO_NET_HDR_SIZE_BASE
        }
    }
}

impl VirtioDevice for VirtioNet {
    fn device_id(&self) -> u32 {
        VIRTIO_DEVICE_NET
    }

    fn features(&self) -> u64 {
        self.features
    }

    fn ack_features(&mut self, features: u64) {
        self.acked_features = features;
    }

    fn num_queues(&self) -> usize {
        2 // 0 = RX, 1 = TX
    }

    fn config_read(&self, offset: u64, size: u32) -> u64 {
        // Config space layout:
        //   0-5   : mac[6]
        //   6-7   : status (u16, 1 = link up)
        //   8-9   : max_virtqueue_pairs (u16, 1)
        let mut buf = [0u8; 10];
        buf[..6].copy_from_slice(&self.mac);
        buf[6..8].copy_from_slice(&1u16.to_le_bytes());
        buf[8..10].copy_from_slice(&1u16.to_le_bytes());

        read_sub(&buf, offset as usize, size)
    }

    fn reset(&mut self) {
        self.acked_features = 0;
    }

    /// Process a virtqueue notification.
    ///
    /// # Safety
    /// Caller must ensure `ram` is valid for
    /// [`ram_base`, `ram_base + ram_size`).
    unsafe fn handle_queue(
        &mut self,
        idx: u32,
        queue: &mut VirtQueue,
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> u32 {
        match idx {
            // RX queue: host-initiated, nothing to do
            // on a guest kick.
            0 => 0,
            // TX queue: drain available buffers and
            // write payloads to the backend.
            1 => unsafe { self.tx_process(queue, ram, ram_base, ram_size) },
            _ => 0,
        }
    }
}

impl VirtioNet {
    /// Walk the TX queue and send each packet to the
    /// backend.
    ///
    /// # Safety
    /// Caller must ensure `ram` is valid for
    /// [`ram_base`, `ram_base + ram_size`).
    unsafe fn tx_process(
        &mut self,
        queue: &mut VirtQueue,
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> u32 {
        let avail_idx =
            unsafe { queue.read_avail_idx(ram, ram_base, ram_size) };
        let mut processed = 0u32;
        let mut used_idx = {
            let off = queue.used_addr + 2 - ram_base;
            if off + 2 > ram_size {
                return 0;
            }
            // SAFETY: bounds-checked above.
            unsafe { (ram.add(off as usize) as *const u16).read_unaligned() }
        };

        let hdr_size = self.hdr_size();

        while queue.last_avail_idx != avail_idx {
            let desc_head = unsafe {
                queue.read_avail_ring(
                    queue.last_avail_idx,
                    ram,
                    ram_base,
                    ram_size,
                )
            };
            let chain = queue.walk_chain(desc_head, ram, ram_base, ram_size);

            // Collect entire packet from descriptor
            // chain.
            let mut pkt = Vec::new();
            for desc in &chain {
                let off = match desc.addr.checked_sub(ram_base) {
                    Some(o) if o + desc.len as u64 <= ram_size => o,
                    _ => continue,
                };
                // SAFETY: bounds-checked above.
                let slice = unsafe {
                    std::slice::from_raw_parts(
                        ram.add(off as usize),
                        desc.len as usize,
                    )
                };
                pkt.extend_from_slice(slice);
            }

            // Skip the virtio-net header.
            if pkt.len() > hdr_size {
                let _ = self.backend.write_packet(&pkt[hdr_size..]);
            }

            let written = 0u32;
            unsafe {
                queue.write_used(
                    used_idx,
                    desc_head as u32,
                    written,
                    ram,
                    ram_base,
                    ram_size,
                );
            }
            used_idx = used_idx.wrapping_add(1);
            queue.last_avail_idx = queue.last_avail_idx.wrapping_add(1);
            processed += 1;
        }

        unsafe {
            queue.write_used_idx(used_idx, ram, ram_base, ram_size);
        }
        processed
    }
}

// ── RX path (host-initiated) ──────────────────────────

/// Fill the RX virtqueue with a received packet.
///
/// Prepends a zero-filled virtio-net header, then writes
/// the packet payload into RX descriptors. Returns the
/// number of used ring entries written.
///
/// # Safety
/// Caller must ensure `ram` is valid for
/// [`ram_base`, `ram_base + ram_size`).
pub unsafe fn fill_rx_queue(
    net: &VirtioNet,
    queue: &mut VirtQueue,
    ram: *mut u8,
    ram_base: u64,
    ram_size: u64,
    packet: &[u8],
) -> u32 {
    let hdr_size = net.hdr_size();
    let total_len = hdr_size + packet.len();

    let avail_idx = unsafe { queue.read_avail_idx(ram, ram_base, ram_size) };
    if queue.last_avail_idx == avail_idx {
        return 0; // no available descriptors
    }

    let mut used_idx = {
        let off = queue.used_addr + 2 - ram_base;
        if off + 2 > ram_size {
            return 0;
        }
        // SAFETY: bounds-checked above.
        unsafe { (ram.add(off as usize) as *const u16).read_unaligned() }
    };

    // Build the full frame: header + payload.
    let mut frame = vec![0u8; total_len];
    frame[hdr_size..].copy_from_slice(packet);

    let desc_head = unsafe {
        queue.read_avail_ring(queue.last_avail_idx, ram, ram_base, ram_size)
    };
    let chain = queue.walk_chain(desc_head, ram, ram_base, ram_size);

    // Copy frame bytes into writable descriptors.
    let mut remaining = &frame[..];
    for desc in &chain {
        if remaining.is_empty() {
            break;
        }
        if desc.flags & VRING_DESC_F_WRITE == 0 {
            continue;
        }
        let off = match desc.addr.checked_sub(ram_base) {
            Some(o) if o + desc.len as u64 <= ram_size => o,
            _ => continue,
        };
        let copy_len = remaining.len().min(desc.len as usize);
        // SAFETY: bounds-checked above.
        unsafe {
            std::ptr::copy_nonoverlapping(
                remaining.as_ptr(),
                ram.add(off as usize),
                copy_len,
            );
        }
        remaining = &remaining[copy_len..];
    }

    let written = (total_len - remaining.len()) as u32;
    unsafe {
        queue.write_used(
            used_idx,
            desc_head as u32,
            written,
            ram,
            ram_base,
            ram_size,
        );
    }
    used_idx = used_idx.wrapping_add(1);
    queue.last_avail_idx = queue.last_avail_idx.wrapping_add(1);

    unsafe {
        queue.write_used_idx(used_idx, ram, ram_base, ram_size);
    }
    1
}

// ── Helpers ────────────────────────────────────────────

fn read_sub(bytes: &[u8], off: usize, size: u32) -> u64 {
    match size {
        1 => bytes.get(off).copied().unwrap_or(0) as u64,
        2 => {
            let b = [
                bytes.get(off).copied().unwrap_or(0),
                bytes.get(off + 1).copied().unwrap_or(0),
            ];
            u16::from_le_bytes(b) as u64
        }
        4 => {
            let mut b = [0u8; 4];
            for (i, item) in b.iter_mut().enumerate() {
                *item = bytes.get(off + i).copied().unwrap_or(0);
            }
            u32::from_le_bytes(b) as u64
        }
        8 => {
            let mut b = [0u8; 8];
            for (i, item) in b.iter_mut().enumerate() {
                *item = bytes.get(off + i).copied().unwrap_or(0);
            }
            u64::from_le_bytes(b)
        }
        _ => 0,
    }
}

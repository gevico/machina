// VirtIO network device tests.

use std::os::unix::io::RawFd;

use machina_hw_virtio::device::{VirtioDevice, VIRTIO_F_VERSION_1};
use machina_hw_virtio::net::{
    fill_rx_queue, parse_mac, VirtioNet, VIRTIO_NET_HDR_SIZE,
};
use machina_hw_virtio::queue::{
    VirtQueue, VRING_DESC_F_NEXT, VRING_DESC_F_WRITE,
};

// VirtIO net feature bits (from the spec).
const VIRTIO_NET_F_MAC: u64 = 1 << 5;
const VIRTIO_NET_F_STATUS: u64 = 1 << 16;

// ---- helpers: simulated guest RAM + virtqueue ----

struct GuestRam {
    ptr: *mut u8,
    layout: std::alloc::Layout,
    base: u64,
    size: usize,
}

impl GuestRam {
    fn new(size: usize, base: u64) -> Self {
        let layout =
            std::alloc::Layout::from_size_align(size, 4096).unwrap();
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!ptr.is_null());
        Self {
            ptr,
            layout,
            base,
            size,
        }
    }

    fn gpa(&self, offset: usize) -> u64 {
        self.base + offset as u64
    }

    fn write_bytes(&self, offset: usize, data: &[u8]) {
        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                self.ptr.add(offset),
                data.len(),
            );
        }
    }

    fn read_bytes(&self, offset: usize, len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len];
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.ptr.add(offset),
                v.as_mut_ptr(),
                len,
            );
        }
        v
    }

    fn read_u16(&self, offset: usize) -> u16 {
        unsafe { (self.ptr.add(offset) as *const u16).read_unaligned() }
    }

    fn read_u32(&self, offset: usize) -> u32 {
        unsafe { (self.ptr.add(offset) as *const u32).read_unaligned() }
    }
}

impl Drop for GuestRam {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.ptr, self.layout) }
    }
}

const DESC_OFF: usize = 0;
const AVAIL_OFF: usize = 4096;
const USED_OFF: usize = 8192;
const BUF_OFF: usize = 12288;

fn write_desc(
    ram: &GuestRam,
    idx: u16,
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
) {
    let off = DESC_OFF + (idx as usize) * 16;
    unsafe {
        let p = ram.ptr.add(off);
        (p as *mut u64).write_unaligned(addr);
        (p.add(8) as *mut u32).write_unaligned(len);
        (p.add(12) as *mut u16).write_unaligned(flags);
        (p.add(14) as *mut u16).write_unaligned(next);
    }
}

fn push_avail(ram: &GuestRam, avail_idx: u16, desc_idx: u16) {
    unsafe {
        let ring_off = AVAIL_OFF + 4 + (avail_idx as usize) * 2;
        (ram.ptr.add(ring_off) as *mut u16).write_unaligned(desc_idx);
        (ram.ptr.add(AVAIL_OFF + 2) as *mut u16)
            .write_unaligned(avail_idx + 1);
    }
}

fn new_queue(ram: &GuestRam) -> VirtQueue {
    let mut q = VirtQueue::new();
    q.num = 256;
    q.desc_addr = ram.gpa(DESC_OFF);
    q.avail_addr = ram.gpa(AVAIL_OFF);
    q.used_addr = ram.gpa(USED_OFF);
    q.ready = true;
    q
}

fn dummy_net() -> VirtioNet {
    VirtioNet::new(-1, [0x52, 0x54, 0x00, 0x12, 0x34, 0x56])
}

struct PipePair {
    read_fd: RawFd,
    write_fd: RawFd,
}

impl PipePair {
    fn new() -> Self {
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        Self {
            read_fd: fds[0],
            write_fd: fds[1],
        }
    }

    fn read_all(&self) -> Vec<u8> {
        unsafe {
            let flags = libc::fcntl(self.read_fd, libc::F_GETFL);
            libc::fcntl(
                self.read_fd,
                libc::F_SETFL,
                flags | libc::O_NONBLOCK,
            );
        }
        let mut buf = vec![0u8; 65536];
        let n = unsafe {
            libc::read(
                self.read_fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if n <= 0 {
            return Vec::new();
        }
        buf.truncate(n as usize);
        buf
    }
}

impl Drop for PipePair {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.read_fd);
        }
    }
}

// ---- parse_mac tests ----

#[test]
fn test_parse_mac_valid() {
    assert_eq!(
        parse_mac("52:54:00:12:34:56").unwrap(),
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]
    );
}

#[test]
fn test_parse_mac_all_ff() {
    assert_eq!(parse_mac("ff:ff:ff:ff:ff:ff").unwrap(), [0xff; 6]);
}

#[test]
fn test_parse_mac_too_few_octets() {
    assert!(parse_mac("52:54:00:12:34").is_err());
}

#[test]
fn test_parse_mac_too_many_octets() {
    assert!(parse_mac("52:54:00:12:34:56:78").is_err());
}

#[test]
fn test_parse_mac_bad_hex() {
    assert!(parse_mac("ZZ:54:00:12:34:56").is_err());
}

#[test]
fn test_parse_mac_empty() {
    assert!(parse_mac("").is_err());
}

// ---- VirtioDevice trait tests ----

#[test]
fn test_net_device_id() {
    assert_eq!(dummy_net().device_id(), 1);
}

#[test]
fn test_net_num_queues() {
    assert_eq!(dummy_net().num_queues(), 2);
}

#[test]
fn test_net_features_version1() {
    let f = dummy_net().features();
    assert_ne!(f & VIRTIO_F_VERSION_1, 0);
}

#[test]
fn test_net_features_mac() {
    let f = dummy_net().features();
    assert_ne!(f & VIRTIO_NET_F_MAC, 0);
}

#[test]
fn test_net_features_status() {
    let f = dummy_net().features();
    assert_ne!(f & VIRTIO_NET_F_STATUS, 0);
}

// ---- config space tests ----

#[test]
fn test_net_config_read_mac_bytes() {
    let mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
    let net = VirtioNet::new(-1, mac);
    for i in 0..6u64 {
        assert_eq!(
            net.config_read(i, 1),
            mac[i as usize] as u64,
            "MAC byte {i}"
        );
    }
}

#[test]
fn test_net_config_read_mac_u16() {
    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    let net = VirtioNet::new(-1, mac);
    let v = net.config_read(0, 2);
    assert_eq!(v, u16::from_le_bytes([0x52, 0x54]) as u64);
}

#[test]
fn test_net_config_read_status_link_up() {
    let net = dummy_net();
    assert_eq!(net.config_read(6, 2), 1);
}

#[test]
fn test_net_config_read_max_vq_pairs() {
    let net = dummy_net();
    assert_eq!(net.config_read(8, 2), 1);
}

#[test]
fn test_net_config_read_out_of_range() {
    let net = dummy_net();
    assert_eq!(net.config_read(100, 4), 0);
}

// ---- RX path: fill_rx_queue tests ----

#[test]
fn test_net_rx_single_packet() {
    let ram = GuestRam::new(65536, 0x8000_0000);
    let mut q = new_queue(&ram);

    write_desc(&ram, 0, ram.gpa(BUF_OFF), 2048, VRING_DESC_F_WRITE, 0);
    push_avail(&ram, 0, 0);

    let packet = vec![0xAA_u8; 64];
    let n = unsafe {
        fill_rx_queue(&packet, &mut q, ram.ptr, ram.base, ram.size as u64)
    };
    assert_eq!(n, 1, "should inject one descriptor");
    assert_eq!(q.last_avail_idx, 1);

    assert_eq!(ram.read_u16(USED_OFF + 2), 1);
    assert_eq!(ram.read_u32(USED_OFF + 4), 0);
    assert_eq!(
        ram.read_u32(USED_OFF + 8),
        (VIRTIO_NET_HDR_SIZE + 64) as u32,
    );

    let hdr = ram.read_bytes(BUF_OFF, VIRTIO_NET_HDR_SIZE);
    assert_eq!(hdr, vec![0u8; VIRTIO_NET_HDR_SIZE]);

    let payload = ram.read_bytes(BUF_OFF + VIRTIO_NET_HDR_SIZE, 64);
    assert_eq!(payload, vec![0xAA; 64]);
}

#[test]
fn test_net_rx_no_available_descriptors() {
    let ram = GuestRam::new(65536, 0x8000_0000);
    let mut q = new_queue(&ram);

    let n = unsafe {
        fill_rx_queue(&[0xBB; 60], &mut q, ram.ptr, ram.base, ram.size as u64)
    };
    assert_eq!(n, 0);
    assert_eq!(q.last_avail_idx, 0);
}

#[test]
fn test_net_rx_chained_descriptors() {
    let ram = GuestRam::new(65536, 0x8000_0000);
    let mut q = new_queue(&ram);

    write_desc(
        &ram,
        0,
        ram.gpa(BUF_OFF),
        16,
        VRING_DESC_F_WRITE | VRING_DESC_F_NEXT,
        1,
    );
    write_desc(
        &ram,
        1,
        ram.gpa(BUF_OFF + 16),
        2032,
        VRING_DESC_F_WRITE,
        0,
    );
    push_avail(&ram, 0, 0);

    let pkt_data = vec![0xCC_u8; 100];
    let n = unsafe {
        fill_rx_queue(
            &pkt_data,
            &mut q,
            ram.ptr,
            ram.base,
            ram.size as u64,
        )
    };
    assert_eq!(n, 1);

    let d0 = ram.read_bytes(BUF_OFF, 16);
    assert_eq!(&d0[..VIRTIO_NET_HDR_SIZE], &[0u8; 10]);
    assert_eq!(&d0[10..16], &pkt_data[..6]);

    let d1 = ram.read_bytes(BUF_OFF + 16, 94);
    assert_eq!(&d1[..94], &pkt_data[6..]);
}

#[test]
fn test_net_rx_multiple_packets_sequential() {
    let ram = GuestRam::new(65536, 0x8000_0000);
    let mut q = new_queue(&ram);

    for i in 0..3u16 {
        let off = BUF_OFF + (i as usize) * 2048;
        write_desc(&ram, i, ram.gpa(off), 2048, VRING_DESC_F_WRITE, 0);
        push_avail(&ram, i, i);
    }

    for i in 0..3u32 {
        let pkt = vec![(i + 1) as u8; 60];
        let n = unsafe {
            fill_rx_queue(
                &pkt,
                &mut q,
                ram.ptr,
                ram.base,
                ram.size as u64,
            )
        };
        assert_eq!(n, 1, "packet {i}");
    }

    assert_eq!(q.last_avail_idx, 3);
    assert_eq!(ram.read_u16(USED_OFF + 2), 3);

    for i in 0..3usize {
        let off = BUF_OFF + i * 2048;
        let val = (i + 1) as u8;
        let payload = ram.read_bytes(off + VIRTIO_NET_HDR_SIZE, 60);
        assert_eq!(payload, vec![val; 60], "packet {i} content");
    }
}

// ---- TX path tests (using pipe as TAP substitute) ----

#[test]
fn test_net_tx_single_packet() {
    let pipe = PipePair::new();
    let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
    let mut net = VirtioNet::new(pipe.write_fd, mac);

    let ram = GuestRam::new(65536, 0x8000_0000);
    let mut q = new_queue(&ram);

    let eth_frame = [0xDD_u8; 60];
    let mut pkt = vec![0u8; VIRTIO_NET_HDR_SIZE];
    pkt.extend_from_slice(&eth_frame);
    ram.write_bytes(BUF_OFF, &pkt);

    write_desc(&ram, 0, ram.gpa(BUF_OFF), pkt.len() as u32, 0, 0);
    push_avail(&ram, 0, 0);

    let processed = unsafe {
        net.handle_queue(1, &mut q, ram.ptr, ram.base, ram.size as u64)
    };
    assert_eq!(processed, 1);
    assert_eq!(q.last_avail_idx, 1);
    assert_eq!(ram.read_u16(USED_OFF + 2), 1);

    let received = pipe.read_all();
    assert_eq!(received, eth_frame);
}

#[test]
fn test_net_tx_chained_hdr_and_data() {
    let pipe = PipePair::new();
    let mut net = VirtioNet::new(pipe.write_fd, [0; 6]);

    let ram = GuestRam::new(65536, 0x8000_0000);
    let mut q = new_queue(&ram);

    let hdr = [0u8; VIRTIO_NET_HDR_SIZE];
    ram.write_bytes(BUF_OFF, &hdr);
    write_desc(
        &ram,
        0,
        ram.gpa(BUF_OFF),
        VIRTIO_NET_HDR_SIZE as u32,
        VRING_DESC_F_NEXT,
        1,
    );

    let eth = [0xEE_u8; 128];
    ram.write_bytes(BUF_OFF + 256, &eth);
    write_desc(&ram, 1, ram.gpa(BUF_OFF + 256), 128, 0, 0);

    push_avail(&ram, 0, 0);

    let n = unsafe {
        net.handle_queue(1, &mut q, ram.ptr, ram.base, ram.size as u64)
    };
    assert_eq!(n, 1);

    let received = pipe.read_all();
    assert_eq!(received, eth);
}

#[test]
fn test_net_tx_multiple_packets() {
    let pipe = PipePair::new();
    let mut net = VirtioNet::new(pipe.write_fd, [0; 6]);

    let ram = GuestRam::new(65536, 0x8000_0000);
    let mut q = new_queue(&ram);

    for i in 0..3u16 {
        let eth = vec![i as u8 + 0xA0; 60];
        let mut pkt = vec![0u8; VIRTIO_NET_HDR_SIZE];
        pkt.extend_from_slice(&eth);
        let off = BUF_OFF + (i as usize) * 256;
        ram.write_bytes(off, &pkt);
        write_desc(&ram, i, ram.gpa(off), pkt.len() as u32, 0, 0);
        push_avail(&ram, i, i);
    }

    let n = unsafe {
        net.handle_queue(1, &mut q, ram.ptr, ram.base, ram.size as u64)
    };
    assert_eq!(n, 3);
    assert_eq!(q.last_avail_idx, 3);

    let all = pipe.read_all();
    assert_eq!(all.len(), 3 * 60);
    for i in 0..3usize {
        let frame = &all[i * 60..(i + 1) * 60];
        let expected = vec![0xA0 + i as u8; 60];
        assert_eq!(frame, &expected[..], "frame {i}");
    }
}

#[test]
fn test_net_tx_queue0_is_noop() {
    let pipe = PipePair::new();
    let mut net = VirtioNet::new(pipe.write_fd, [0; 6]);

    let ram = GuestRam::new(65536, 0x8000_0000);
    let mut q = new_queue(&ram);

    let mut pkt = vec![0u8; VIRTIO_NET_HDR_SIZE];
    pkt.extend_from_slice(&[0xFF; 60]);
    ram.write_bytes(BUF_OFF, &pkt);
    write_desc(&ram, 0, ram.gpa(BUF_OFF), pkt.len() as u32, 0, 0);
    push_avail(&ram, 0, 0);

    let n = unsafe {
        net.handle_queue(0, &mut q, ram.ptr, ram.base, ram.size as u64)
    };
    assert_eq!(n, 0);
    assert_eq!(pipe.read_all().len(), 0);
}

// ---- RX → TX round-trip test ----

#[test]
fn test_net_rx_then_tx_round_trip() {
    let pipe = PipePair::new();
    let mut net = VirtioNet::new(pipe.write_fd, [0; 6]);

    let ram = GuestRam::new(65536, 0x8000_0000);

    // -- RX side: inject packet into queue 0 --
    let mut rx_q = new_queue(&ram);

    let rx_buf_off = BUF_OFF;
    write_desc(
        &ram,
        0,
        ram.gpa(rx_buf_off),
        2048,
        VRING_DESC_F_WRITE,
        0,
    );
    push_avail(&ram, 0, 0);

    let original = b"Hello, virtio-net!";
    let injected = unsafe {
        fill_rx_queue(original, &mut rx_q, ram.ptr, ram.base, ram.size as u64)
    };
    assert_eq!(injected, 1);

    let total_len = VIRTIO_NET_HDR_SIZE + original.len();
    let rx_data = ram.read_bytes(rx_buf_off, total_len);

    // -- TX side: forward the received data --
    let tx_desc_off = 16384;
    let tx_avail_off = 20480;
    let tx_used_off = 24576;
    let tx_buf_off = 28672;

    let mut tx_q = VirtQueue::new();
    tx_q.num = 256;
    tx_q.desc_addr = ram.gpa(tx_desc_off);
    tx_q.avail_addr = ram.gpa(tx_avail_off);
    tx_q.used_addr = ram.gpa(tx_used_off);
    tx_q.ready = true;

    ram.write_bytes(tx_buf_off, &rx_data);

    unsafe {
        let p = ram.ptr.add(tx_desc_off);
        (p as *mut u64).write_unaligned(ram.gpa(tx_buf_off));
        (p.add(8) as *mut u32).write_unaligned(rx_data.len() as u32);
        (p.add(12) as *mut u16).write_unaligned(0);
        (p.add(14) as *mut u16).write_unaligned(0);
    }
    unsafe {
        (ram.ptr.add(tx_avail_off + 4) as *mut u16).write_unaligned(0);
        (ram.ptr.add(tx_avail_off + 2) as *mut u16).write_unaligned(1);
    }

    let n = unsafe {
        net.handle_queue(1, &mut tx_q, ram.ptr, ram.base, ram.size as u64)
    };
    assert_eq!(n, 1);

    let received = pipe.read_all();
    assert_eq!(received, original);
}

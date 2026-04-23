// VirtIO block device backend: mmap'd raw file.

use std::fs::OpenOptions;
use std::io;
use std::path::Path;

use memmap2::MmapMut;

use crate::queue::{Desc, VirtQueue, VRING_DESC_F_WRITE};
use crate::VirtioDevice;

const SECTOR_SIZE: u64 = 512;

// Block request types.
const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;

// Block request status.
const VIRTIO_BLK_S_OK: u8 = 0;
const VIRTIO_BLK_S_IOERR: u8 = 1;
const VIRTIO_BLK_S_UNSUPP: u8 = 2;

// Feature bit.
pub const VIRTIO_F_VERSION_1: u64 = 1 << 32;

/// VirtIO block device backed by a raw file.
pub struct VirtioBlk {
    mmap: MmapMut,
    capacity: u64,
}

// SAFETY: MmapMut pointer is stable for the file's lifetime.
unsafe impl Send for VirtioBlk {}

impl VirtioBlk {
    /// Open a raw disk image and mmap it.
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        let len = file.metadata()?.len() as usize;
        if len == 0 || !len.is_multiple_of(SECTOR_SIZE as usize) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "disk image size {} not aligned \
                     to {}",
                    len, SECTOR_SIZE
                ),
            ));
        }
        // SAFETY: file is opened read-write and remains
        // alive for the lifetime of the mmap.
        let mmap = unsafe { MmapMut::map_mut(&file)? };
        Ok(Self {
            capacity: len as u64 / SECTOR_SIZE,
            mmap,
        })
    }

    /// Device capacity in 512-byte sectors.
    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Raw pointer to the start of the mapped data.
    fn data_ptr(&self) -> *mut u8 {
        self.mmap.as_ptr() as *mut u8
    }

    /// Process a single block request from a descriptor
    /// chain. Returns total bytes written to
    /// device-writable descriptors.
    fn process_request(
        &self,
        chain: &[Desc],
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> u32 {
        if chain.len() < 2 {
            return 0;
        }

        // First descriptor: header (16 bytes,
        // device-readable).
        let hdr = &chain[0];
        let hdr_off = match hdr.addr.checked_sub(ram_base) {
            Some(o) if o + 16 <= ram_size => o,
            _ => return 0,
        };
        if hdr.len < 16 {
            return 0;
        }
        let (req_type, sector) = unsafe {
            let p = ram.add(hdr_off as usize);
            let t = (p as *const u32).read_unaligned();
            let s = (p.add(8) as *const u64).read_unaligned();
            (t, s)
        };

        // Last descriptor: status (1 byte,
        // device-writable).
        let status_desc = &chain[chain.len() - 1];
        let status_off =
            status_desc.addr.checked_sub(ram_base).unwrap_or(u64::MAX);
        let status_valid = status_off < ram_size
            && status_desc.flags & VRING_DESC_F_WRITE != 0;

        let mut total_written = 0u32;
        let status = match req_type {
            VIRTIO_BLK_T_IN => self.do_read(
                sector,
                &chain[1..chain.len() - 1],
                ram,
                ram_base,
                ram_size,
                &mut total_written,
            ),
            VIRTIO_BLK_T_OUT => self.do_write(
                sector,
                &chain[1..chain.len() - 1],
                ram,
                ram_base,
                ram_size,
            ),
            _ => VIRTIO_BLK_S_UNSUPP,
        };

        // Write status byte.
        if status_valid {
            unsafe {
                *ram.add(status_off as usize) = status;
            }
            total_written += 1;
        }
        total_written
    }

    fn do_read(
        &self,
        sector: u64,
        data_descs: &[Desc],
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
        total_written: &mut u32,
    ) -> u8 {
        let mut disk_off = match sector.checked_mul(SECTOR_SIZE) {
            Some(o) => o,
            None => return VIRTIO_BLK_S_IOERR,
        };
        let disk_cap = self.capacity * SECTOR_SIZE;
        for desc in data_descs {
            if desc.flags & VRING_DESC_F_WRITE == 0 {
                continue;
            }
            let guest_off = match desc.addr.checked_sub(ram_base) {
                Some(o) => o,
                None => return VIRTIO_BLK_S_IOERR,
            };
            let len = desc.len as u64;
            if guest_off + len > ram_size {
                return VIRTIO_BLK_S_IOERR;
            }
            if disk_off + len > disk_cap {
                return VIRTIO_BLK_S_IOERR;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.data_ptr().add(disk_off as usize),
                    ram.add(guest_off as usize),
                    len as usize,
                );
            }
            disk_off += len;
            *total_written += desc.len;
        }
        VIRTIO_BLK_S_OK
    }

    fn do_write(
        &self,
        sector: u64,
        data_descs: &[Desc],
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) -> u8 {
        let mut disk_off = match sector.checked_mul(SECTOR_SIZE) {
            Some(o) => o,
            None => return VIRTIO_BLK_S_IOERR,
        };
        let disk_cap = self.capacity * SECTOR_SIZE;
        for desc in data_descs {
            if desc.flags & VRING_DESC_F_WRITE != 0 {
                continue;
            }
            let guest_off = match desc.addr.checked_sub(ram_base) {
                Some(o) => o,
                None => return VIRTIO_BLK_S_IOERR,
            };
            let len = desc.len as u64;
            if guest_off + len > ram_size {
                return VIRTIO_BLK_S_IOERR;
            }
            if disk_off + len > disk_cap {
                return VIRTIO_BLK_S_IOERR;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(
                    ram.add(guest_off as usize),
                    self.data_ptr().add(disk_off as usize),
                    len as usize,
                );
            }
            disk_off += len;
        }
        VIRTIO_BLK_S_OK
    }
}

const VIRTIO_DEVICE_BLK: u32 = 2;

impl VirtioDevice for VirtioBlk {
    fn device_id(&self) -> u32 {
        VIRTIO_DEVICE_BLK
    }

    fn features(&self) -> u64 {
        VIRTIO_F_VERSION_1
    }

    fn ack_features(&mut self, _features: u64) {}

    fn num_queues(&self) -> usize {
        1
    }

    fn config_read(&self, offset: u64, size: u32) -> u64 {
        match offset {
            // capacity (u64 at offset 0)
            0..=7 => {
                let bytes = self.capacity.to_le_bytes();
                read_sub(&bytes, offset as usize, size)
            }
            // blk_size (u32 at offset 20)
            20..=23 => {
                let bytes = 512u32.to_le_bytes();
                read_sub(&bytes, (offset - 20) as usize, size)
            }
            _ => 0,
        }
    }

    unsafe fn handle_queue(
        &mut self,
        _idx: u32,
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
            unsafe { (ram.add(off as usize) as *const u16).read_unaligned() }
        };

        while queue.last_avail_idx != avail_idx {
            let desc_head = queue.read_avail_ring(
                queue.last_avail_idx,
                ram,
                ram_base,
                ram_size,
            );
            let chain = queue.walk_chain(desc_head, ram, ram_base, ram_size);
            let written = self.process_request(&chain, ram, ram_base, ram_size);
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

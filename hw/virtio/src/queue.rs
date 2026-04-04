// VirtIO split virtqueue: descriptor table, available
// ring, and used ring in guest physical memory.

/// Maximum queue size.
pub const MAX_QUEUE_SIZE: u32 = 256;

/// Descriptor flags.
pub const VRING_DESC_F_NEXT: u16 = 1;
pub const VRING_DESC_F_WRITE: u16 = 2;

/// A single vring descriptor (16 bytes in guest memory).
#[derive(Clone, Copy, Debug)]
pub struct Desc {
    pub addr: u64,
    pub len: u32,
    pub flags: u16,
    pub next: u16,
}

/// Per-queue state managed by the transport.
pub struct VirtQueue {
    pub ready: bool,
    pub num: u32,
    pub desc_addr: u64,
    pub avail_addr: u64,
    pub used_addr: u64,
    pub last_avail_idx: u16,
}

impl Default for VirtQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtQueue {
    pub fn new() -> Self {
        Self {
            ready: false,
            num: 0,
            desc_addr: 0,
            avail_addr: 0,
            used_addr: 0,
            last_avail_idx: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Read a descriptor from the descriptor table.
    fn read_desc(
        &self,
        idx: u16,
        ram: *const u8,
        ram_base: u64,
        ram_size: u64,
    ) -> Option<Desc> {
        let addr = self.desc_addr.checked_add((idx as u64) * 16)?;
        let off = addr.checked_sub(ram_base)?;
        if off.checked_add(16)? > ram_size {
            return None;
        }
        let p = unsafe { ram.add(off as usize) };
        Some(unsafe {
            Desc {
                addr: (p as *const u64).read_unaligned(),
                len: (p.add(8) as *const u32).read_unaligned(),
                flags: (p.add(12) as *const u16).read_unaligned(),
                next: (p.add(14) as *const u16).read_unaligned(),
            }
        })
    }

    /// Read the available ring index (avail.idx).
    ///
    /// # Safety
    /// Caller must ensure `ram` is valid for the range
    /// [`ram_base`, `ram_base + ram_size`).
    pub unsafe fn read_avail_idx(
        &self,
        ram: *const u8,
        ram_base: u64,
        ram_size: u64,
    ) -> u16 {
        let addr = match self.avail_addr.checked_add(2) {
            Some(a) => a,
            None => return self.last_avail_idx,
        };
        let off = match addr.checked_sub(ram_base) {
            Some(o) => o,
            None => return self.last_avail_idx,
        };
        if off + 2 > ram_size {
            return self.last_avail_idx;
        }
        unsafe { (ram.add(off as usize) as *const u16).read_unaligned() }
    }

    /// Read an entry from the available ring.
    ///
    /// # Safety
    /// Caller must ensure `ram` is valid for the range
    /// [`ram_base`, `ram_base + ram_size`).
    pub unsafe fn read_avail_ring(
        &self,
        ring_idx: u16,
        ram: *const u8,
        ram_base: u64,
        ram_size: u64,
    ) -> u16 {
        if self.num == 0 {
            return 0;
        }
        let i = (ring_idx as u64) % (self.num as u64);
        let off = self.avail_addr + 4 + i * 2 - ram_base;
        if off + 2 > ram_size {
            return 0;
        }
        unsafe { (ram.add(off as usize) as *const u16).read_unaligned() }
    }

    /// Write an entry to the used ring.
    ///
    /// # Safety
    /// Caller must ensure `ram` is valid for the range
    /// [`ram_base`, `ram_base + ram_size`).
    pub unsafe fn write_used(
        &self,
        used_idx: u16,
        desc_id: u32,
        len: u32,
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) {
        if self.num == 0 {
            return;
        }
        let i = (used_idx as u64) % (self.num as u64);
        // used.ring[i] at used_addr + 4 + i*8.
        let off = self.used_addr + 4 + i * 8 - ram_base;
        if off + 8 > ram_size {
            return;
        }
        unsafe {
            let p = ram.add(off as usize);
            (p as *mut u32).write_unaligned(desc_id);
            (p.add(4) as *mut u32).write_unaligned(len);
        }
    }

    /// Write the used ring index (used.idx).
    ///
    /// # Safety
    /// Caller must ensure `ram` is valid for the range
    /// [`ram_base`, `ram_base + ram_size`).
    pub unsafe fn write_used_idx(
        &self,
        idx: u16,
        ram: *mut u8,
        ram_base: u64,
        ram_size: u64,
    ) {
        let off = self.used_addr + 2 - ram_base;
        if off + 2 > ram_size {
            return;
        }
        unsafe {
            (ram.add(off as usize) as *mut u16).write_unaligned(idx);
        }
    }

    /// Walk a descriptor chain starting at `head`.
    /// Returns a Vec of descriptors. Bounded by queue
    /// size to prevent infinite loops.
    pub fn walk_chain(
        &self,
        head: u16,
        ram: *const u8,
        ram_base: u64,
        ram_size: u64,
    ) -> Vec<Desc> {
        let mut chain = Vec::new();
        let mut idx = head;
        let limit = self.num as usize;
        for _ in 0..limit {
            let desc = match self.read_desc(idx, ram, ram_base, ram_size) {
                Some(d) => d,
                None => break,
            };
            chain.push(desc);
            if desc.flags & VRING_DESC_F_NEXT == 0 {
                break;
            }
            idx = desc.next;
        }
        chain
    }
}

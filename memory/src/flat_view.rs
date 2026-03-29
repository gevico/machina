use std::sync::{Arc, Mutex};

use crate::ram::RamBlock;
use crate::region::{MemoryRegion, MmioOps, RegionType};

// ----- FlatRange: one contiguous span in the flat view -----

/// Discriminant carried by each flat range so the address
/// space can dispatch reads/writes without revisiting the
/// region tree.
pub enum FlatRangeKind {
    Ram { block: Arc<RamBlock> },
    Io { ops: Arc<Mutex<Box<dyn MmioOps>>> },
}

/// A single non-overlapping range in the flattened address
/// map.  `offset_in_region` is the byte offset from the
/// start of the owning leaf region that corresponds to
/// `addr`.
pub struct FlatRange {
    pub addr: u64,
    pub size: u64,
    pub kind: FlatRangeKind,
    pub offset_in_region: u64,
}

impl FlatRange {
    pub fn is_io(&self) -> bool {
        matches!(self.kind, FlatRangeKind::Io { .. })
    }

    fn end(&self) -> u64 {
        self.addr + self.size
    }
}

// ----- FlatView -----

pub struct FlatView {
    pub ranges: Vec<FlatRange>,
}

/// Intermediate record produced by the tree walk before
/// overlap resolution.
struct RawRange {
    addr: u64,
    size: u64,
    priority: i32,
    kind: FlatRangeKind,
    offset_in_region: u64,
}

impl FlatView {
    /// Flatten a `MemoryRegion` tree into a sorted,
    /// non-overlapping list of `FlatRange`s.  Higher-priority
    /// regions win when ranges overlap.
    pub fn from_region(root: &MemoryRegion) -> Self {
        let mut raw: Vec<RawRange> = Vec::new();
        Self::collect(root, 0, 0, &mut raw);

        // Higher priority first; ties broken by address.
        raw.sort_by(|a, b| {
            b.priority.cmp(&a.priority).then(a.addr.cmp(&b.addr))
        });

        let mut resolved: Vec<FlatRange> = Vec::new();
        for r in raw {
            Self::insert_range(&mut resolved, r);
        }

        // Final sort by address.
        resolved.sort_by_key(|r| r.addr);
        Self { ranges: resolved }
    }

    /// Binary-search lookup.  Returns the range containing
    /// `addr`, if any.
    pub fn lookup(&self, addr: u64) -> Option<&FlatRange> {
        let idx = self.ranges.partition_point(|r| r.addr <= addr);
        if idx == 0 {
            return None;
        }
        let r = &self.ranges[idx - 1];
        if addr < r.end() {
            Some(r)
        } else {
            None
        }
    }

    // -- private helpers --

    /// Recursively collect leaf regions with their absolute
    /// addresses and inherited priorities.
    fn collect(
        region: &MemoryRegion,
        base: u64,
        inherited_prio: i32,
        out: &mut Vec<RawRange>,
    ) {
        if !region.enabled {
            return;
        }
        let prio = region.priority.max(inherited_prio);

        match &region.region_type {
            RegionType::Ram { block } => {
                out.push(RawRange {
                    addr: base,
                    size: region.size,
                    priority: prio,
                    kind: FlatRangeKind::Ram {
                        block: Arc::clone(block),
                    },
                    offset_in_region: 0,
                });
            }
            RegionType::Io { ops } => {
                out.push(RawRange {
                    addr: base,
                    size: region.size,
                    priority: prio,
                    kind: FlatRangeKind::Io {
                        ops: Arc::clone(ops),
                    },
                    offset_in_region: 0,
                });
            }
            RegionType::Container => {}
        }

        for sub in &region.subregions {
            Self::collect(&sub.region, base + sub.offset, prio, out);
        }
    }

    /// Insert `raw` into `resolved`, skipping any portions
    /// already covered by a previously inserted (i.e. higher-
    /// priority) range.
    fn insert_range(resolved: &mut Vec<FlatRange>, raw: RawRange) {
        let mut cur = raw.addr;
        let end = raw.addr + raw.size;

        // Collect existing ranges that overlap [cur, end).
        let mut overlaps: Vec<(u64, u64)> = resolved
            .iter()
            .filter(|r| r.addr < end && r.end() > cur)
            .map(|r| (r.addr, r.end()))
            .collect();
        overlaps.sort_by_key(|&(a, _)| a);

        for (oa, ob) in &overlaps {
            if cur < *oa {
                // Gap before this overlap — fill it.
                let gap_end = (*oa).min(end);
                Self::push_fragment(resolved, &raw, cur, gap_end);
            }
            cur = cur.max(*ob);
            if cur >= end {
                break;
            }
        }
        if cur < end {
            Self::push_fragment(resolved, &raw, cur, end);
        }
    }

    fn push_fragment(
        resolved: &mut Vec<FlatRange>,
        raw: &RawRange,
        start: u64,
        end: u64,
    ) {
        let offset_delta = start - raw.addr;
        let kind = match &raw.kind {
            FlatRangeKind::Ram { block } => FlatRangeKind::Ram {
                block: Arc::clone(block),
            },
            FlatRangeKind::Io { ops } => FlatRangeKind::Io {
                ops: Arc::clone(ops),
            },
        };
        resolved.push(FlatRange {
            addr: start,
            size: end - start,
            kind,
            offset_in_region: raw.offset_in_region + offset_delta,
        });
    }
}

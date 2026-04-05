use std::sync::Arc;

use machina_core::address::GPA;

use crate::ram::RamBlock;
use crate::region::{MemoryRegion, MmioOps, RegionType};

// ----- FlatRange: one contiguous span in the flat view -----

/// Discriminant carried by each flat range so the address
/// space can dispatch reads/writes without revisiting the
/// region tree.
pub enum FlatRangeKind {
    Ram { block: Arc<RamBlock> },
    Rom { block: Arc<RamBlock> },
    Io { ops: Arc<dyn MmioOps> },
}

/// A single non-overlapping range in the flattened address
/// map.  `offset_in_region` is the byte offset from the
/// start of the owning leaf region that corresponds to
/// `addr`.
pub struct FlatRange {
    pub addr: GPA,
    pub size: u64,
    pub kind: FlatRangeKind,
    pub offset_in_region: u64,
}

impl FlatRange {
    pub fn is_io(&self) -> bool {
        matches!(self.kind, FlatRangeKind::Io { .. })
    }

    fn end(&self) -> u64 {
        self.addr.0 + self.size
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
        resolved.sort_by_key(|r| r.addr.0);
        Self { ranges: resolved }
    }

    /// Binary-search lookup.  Returns the range containing
    /// `addr`, if any.
    pub fn lookup(&self, addr: GPA) -> Option<&FlatRange> {
        let a = addr.0;
        let idx = self.ranges.partition_point(|r| r.addr.0 <= a);
        if idx == 0 {
            return None;
        }
        let r = &self.ranges[idx - 1];
        if a < r.end() {
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
            RegionType::Rom { block } => {
                out.push(RawRange {
                    addr: base,
                    size: region.size,
                    priority: prio,
                    kind: FlatRangeKind::Rom {
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
            RegionType::Alias {
                target,
                offset: alias_off,
            } => {
                Self::collect_alias(
                    target,
                    base,
                    *alias_off,
                    region.size,
                    prio,
                    out,
                );
            }
            RegionType::Container => {}
        }

        for sub in &region.subregions {
            Self::collect(&sub.region, base + sub.offset.0, prio, out);
        }
    }

    /// Collect a leaf through an alias indirection.  For leaf
    /// targets (Ram/Rom/Io) the alias offset is forwarded as
    /// `offset_in_region`.  Container and nested-alias targets
    /// are recursed into with clipped bounds.
    fn collect_alias(
        target: &MemoryRegion,
        base: u64,
        alias_off: u64,
        alias_size: u64,
        prio: i32,
        out: &mut Vec<RawRange>,
    ) {
        if !target.enabled {
            return;
        }
        match &target.region_type {
            RegionType::Ram { block } => {
                out.push(RawRange {
                    addr: base,
                    size: alias_size,
                    priority: prio,
                    kind: FlatRangeKind::Ram {
                        block: Arc::clone(block),
                    },
                    offset_in_region: alias_off,
                });
            }
            RegionType::Rom { block } => {
                out.push(RawRange {
                    addr: base,
                    size: alias_size,
                    priority: prio,
                    kind: FlatRangeKind::Rom {
                        block: Arc::clone(block),
                    },
                    offset_in_region: alias_off,
                });
            }
            RegionType::Io { ops } => {
                out.push(RawRange {
                    addr: base,
                    size: alias_size,
                    priority: prio,
                    kind: FlatRangeKind::Io {
                        ops: Arc::clone(ops),
                    },
                    offset_in_region: alias_off,
                });
            }
            RegionType::Alias {
                target: inner,
                offset: inner_off,
            } => {
                // Nested alias: compose offsets.
                Self::collect_alias(
                    inner,
                    base,
                    alias_off + inner_off,
                    alias_size,
                    prio,
                    out,
                );
            }
            RegionType::Container => {
                // Recurse into subregions, clipping to the
                // alias window [alias_off, alias_off+size).
                for sub in &target.subregions {
                    let sub_start = sub.offset.0;
                    let sub_end = sub_start + sub.region.size;
                    let win_start = alias_off;
                    let win_end = alias_off + alias_size;
                    if sub_end <= win_start || sub_start >= win_end {
                        continue;
                    }
                    let clip_start = sub_start.max(win_start);
                    let clip_end = sub_end.min(win_end);
                    let new_base = base + (clip_start - win_start);
                    let new_size = clip_end - clip_start;
                    let off_in_sub = clip_start - sub_start;
                    Self::collect_alias(
                        &sub.region,
                        new_base,
                        off_in_sub,
                        new_size,
                        prio,
                        out,
                    );
                }
            }
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
            .filter(|r| r.addr.0 < end && r.end() > cur)
            .map(|r| (r.addr.0, r.end()))
            .collect();
        overlaps.sort_by_key(|&(a, _)| a);

        for (oa, ob) in &overlaps {
            if cur < *oa {
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
            FlatRangeKind::Rom { block } => FlatRangeKind::Rom {
                block: Arc::clone(block),
            },
            FlatRangeKind::Io { ops } => FlatRangeKind::Io {
                ops: Arc::clone(ops),
            },
        };
        resolved.push(FlatRange {
            addr: GPA::new(start),
            size: end - start,
            kind,
            offset_in_region: raw.offset_in_region + offset_delta,
        });
    }
}

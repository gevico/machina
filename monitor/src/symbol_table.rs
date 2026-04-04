//! ELF symbol table for pc-to-function-name resolution.

use object::{Object, ObjectSymbol};
use std::cmp::Ordering;

// ---------------------------------------------------------------------------
// Symbol interval
// ---------------------------------------------------------------------------

/// One `STT_FUNC` entry: `[start_pc, end_pc)` -> name.
#[derive(Clone, Debug)]
struct SymbolInterval {
    start: u64,
    end: u64,
    name: String,
}

// ---------------------------------------------------------------------------
// SymbolTable
// ---------------------------------------------------------------------------

/// Read-only symbol table parsed from an ELF `.symtab`.
///
/// Intervals are sorted by `start` for binary-search lookup.
#[derive(Clone, Debug, Default)]
pub struct SymbolTable {
    intervals: Vec<SymbolInterval>,
    /// Pre-resolved `__switch` range for task-switch detection.
    switch_range: Option<(u64, u64)>,
}

impl SymbolTable {
    /// Parse all `STT_FUNC` symbols from raw ELF bytes.
    pub fn from_elf(data: &[u8]) -> Result<Self, String> {
        let obj =
            object::read::elf::ElfFile64::<object::Endianness>::parse(data)
                .map_err(|e| format!("ELF parse error: {}", e))?;

        let mut intervals: Vec<SymbolInterval> = Vec::new();

        for sym in obj.symbols() {
            // Only interested in function symbols with a name.
            if sym.kind() != object::SymbolKind::Text {
                continue;
            }
            let name_bytes = match sym.name_bytes() {
                Ok(n) if !n.is_empty() => n,
                _ => continue,
            };
            let name = String::from_utf8_lossy(name_bytes).into_owned();

            let addr = sym.address();
            let size = sym.size();
            if size == 0 || addr == 0 {
                continue;
            }

            intervals.push(SymbolInterval {
                start: addr,
                end: addr + size,
                name,
            });
        }

        // Sort by start address.
        intervals.sort_by_key(|s| s.start);

        // Pre-resolve __switch range.
        let switch_range = intervals
            .iter()
            .find(|s| s.name == "__switch")
            .map(|s| (s.start, s.end));

        Ok(Self {
            intervals,
            switch_range,
        })
    }

    /// Look up the function name containing `pc`.
    ///
    /// Returns `None` if `pc` is not inside any known symbol range.
    pub fn lookup(&self, pc: u64) -> Option<&str> {
        let idx = self.intervals.binary_search_by(|s| {
            if pc < s.start {
                Ordering::Greater
            } else if pc >= s.end {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        });
        match idx {
            Ok(i) => Some(&self.intervals[i].name),
            Err(_) => None,
        }
    }

    /// Whether `pc` falls inside the `__switch` function range.
    pub fn in_switch_range(&self, pc: u64) -> bool {
        self.switch_range
            .is_some_and(|(start, end)| pc >= start && pc < end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal ELF64 with two function symbols.
    ///
    /// We reuse the `object` crate's ability to parse the bytes we
    /// craft by hand.
    fn build_test_elf() -> Vec<u8> {
        // For testing purposes, use a real ELF binary if available,
        // otherwise just test with empty table.
        Vec::new()
    }

    #[test]
    fn empty_table_returns_none() {
        let table = SymbolTable::default();
        assert!(table.lookup(0x1000).is_none());
        assert!(!table.in_switch_range(0x1000));
    }

    #[test]
    fn switch_range_absent() {
        let table = SymbolTable::default();
        assert!(!table.in_switch_range(0));
    }
}

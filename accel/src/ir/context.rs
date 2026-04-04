use std::collections::HashMap;

use super::label::Label;
use super::op::{Op, OpIdx};
use super::temp::{Temp, TempIdx};
use super::types::{RegSet, Type, TYPE_COUNT};

/// Maximum number of temps per translation context.
pub const MAX_TEMPS: usize = 512;
/// Maximum number of guest instructions per TB.
pub const MAX_INSNS: usize = 512;

/// Per-thread TCG translation context.
///
/// Maps to QEMU's `TCGContext`. Holds all state needed during translation
/// of a single translation block: temporaries, IR ops, labels, and
/// register allocation metadata.
pub struct Context {
    temps: Vec<Temp>,
    ops: Vec<Op>,
    labels: Vec<Label>,

    /// Number of global temps (always at the front of `temps`).
    nb_globals: u32,

    // -- Stack frame for spilling --
    /// Host register used as the frame pointer for spill slots.
    pub frame_reg: Option<u8>,
    /// Start offset of the spill area in the stack frame.
    pub frame_start: i64,
    /// End offset (next free byte) of the spill area.
    pub frame_end: i64,
    /// Next free byte in the spill area (grows from frame_start).
    pub frame_alloc_end: i64,

    // -- Register allocation state --
    /// Registers reserved by the backend (not available for allocation).
    pub reserved_regs: RegSet,

    // -- Constant deduplication --
    /// Per-type hash map from constant value to TempIdx,
    /// avoiding duplicate const temps.
    const_table: [HashMap<u64, TempIdx>; TYPE_COUNT],

    // -- Guest instruction tracking --
    /// End offset in host code for each guest instruction
    /// (indexed by guest insn number).
    pub gen_insn_end_off: Vec<u16>,

    // -- TB identification --
    /// Index of the TB being translated. Used by the backend to
    /// encode the source TB in exit_tb return values for direct
    /// chaining.
    pub tb_idx: u32,
    pub contains_atomic: bool,
}

impl Context {
    pub fn new() -> Self {
        Self {
            temps: Vec::with_capacity(256),
            ops: Vec::with_capacity(512),
            labels: Vec::with_capacity(32),
            nb_globals: 0,
            frame_reg: None,
            frame_start: 0,
            frame_end: 0,
            frame_alloc_end: 0,
            reserved_regs: RegSet::EMPTY,
            const_table: Default::default(),
            gen_insn_end_off: Vec::with_capacity(MAX_INSNS),
            tb_idx: 0,
            contains_atomic: false,
        }
    }

    /// Reset context for translating a new TB. Preserves globals
    /// but resets their register allocation state so the next
    /// codegen pass starts with all globals in memory.
    pub fn reset(&mut self) {
        self.temps.truncate(self.nb_globals as usize);
        // Reset regalloc state on surviving globals
        for t in &mut self.temps {
            match t.kind {
                super::temp::TempKind::Fixed => {
                    // Fixed temps stay in their register
                    t.mem_coherent = false;
                }
                super::temp::TempKind::Global => {
                    t.val_type = super::types::TempVal::Mem;
                    t.reg = None;
                    t.mem_coherent = true;
                }
                _ => {}
            }
        }
        self.ops.clear();
        self.labels.clear();
        for table in &mut self.const_table {
            table.clear();
        }
        self.gen_insn_end_off.clear();
        self.frame_alloc_end = self.frame_start;
        self.contains_atomic = false;
    }

    // -- Temp allocation --

    pub fn nb_globals(&self) -> u32 {
        self.nb_globals
    }

    pub fn nb_temps(&self) -> u32 {
        self.temps.len() as u32
    }

    /// Allocate a new EBB-scoped temporary.
    pub fn new_temp(&mut self, ty: Type) -> TempIdx {
        let idx = TempIdx(self.temps.len() as u32);
        self.temps.push(Temp::new_ebb(idx, ty));
        idx
    }

    /// Allocate a new TB-scoped temporary.
    pub fn new_temp_tb(&mut self, ty: Type) -> TempIdx {
        let idx = TempIdx(self.temps.len() as u32);
        self.temps.push(Temp::new_tb(idx, ty));
        idx
    }

    /// Get or create a constant temp (deduplicated per type).
    pub fn new_const(&mut self, ty: Type, val: u64) -> TempIdx {
        let type_idx = ty as usize;
        if let Some(&existing) = self.const_table[type_idx].get(&val) {
            return existing;
        }
        let idx = TempIdx(self.temps.len() as u32);
        self.temps.push(Temp::new_const(idx, ty, val));
        self.const_table[type_idx].insert(val, idx);
        idx
    }

    /// Register a global temp (must be called before any
    /// non-global allocation).
    /// The `base` is the TempIdx of the env pointer (a fixed temp).
    pub fn new_global(
        &mut self,
        ty: Type,
        base: TempIdx,
        offset: i64,
        name: &'static str,
    ) -> TempIdx {
        assert_eq!(
            self.temps.len() as u32,
            self.nb_globals,
            "globals must be registered before locals"
        );
        let idx = TempIdx(self.temps.len() as u32);
        self.temps
            .push(Temp::new_global(idx, ty, base, offset, name));
        self.nb_globals += 1;
        idx
    }

    /// Register a fixed-register temp (must be called
    /// before any non-global allocation).
    pub fn new_fixed(
        &mut self,
        ty: Type,
        reg: u8,
        name: &'static str,
    ) -> TempIdx {
        assert_eq!(
            self.temps.len() as u32,
            self.nb_globals,
            "fixed temps must be registered before locals"
        );
        let idx = TempIdx(self.temps.len() as u32);
        self.temps.push(Temp::new_fixed(idx, ty, reg, name));
        self.nb_globals += 1;
        idx
    }

    pub fn temp(&self, idx: TempIdx) -> &Temp {
        &self.temps[idx.0 as usize]
    }

    pub fn temp_mut(&mut self, idx: TempIdx) -> &mut Temp {
        &mut self.temps[idx.0 as usize]
    }

    pub fn temps(&self) -> &[Temp] {
        &self.temps
    }

    /// Iterate over global temps only.
    pub fn globals(&self) -> &[Temp] {
        &self.temps[..self.nb_globals as usize]
    }

    // -- Op emission --

    pub fn emit_op(&mut self, op: Op) -> OpIdx {
        let idx = op.idx;
        self.ops.push(op);
        idx
    }

    pub fn next_op_idx(&self) -> OpIdx {
        OpIdx(self.ops.len() as u32)
    }

    pub fn op(&self, idx: OpIdx) -> &Op {
        &self.ops[idx.0 as usize]
    }

    pub fn op_mut(&mut self, idx: OpIdx) -> &mut Op {
        &mut self.ops[idx.0 as usize]
    }

    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    pub fn num_ops(&self) -> usize {
        self.ops.len()
    }

    // -- Labels --

    pub fn new_label(&mut self) -> u32 {
        let id = self.labels.len() as u32;
        self.labels.push(Label::new(id));
        id
    }

    pub fn label(&self, id: u32) -> &Label {
        &self.labels[id as usize]
    }

    pub fn label_mut(&mut self, id: u32) -> &mut Label {
        &mut self.labels[id as usize]
    }

    pub fn labels(&self) -> &[Label] {
        &self.labels
    }

    // -- Frame management --

    /// Configure the stack frame for spilling.
    pub fn set_frame(&mut self, reg: u8, start: i64, size: i64) {
        self.frame_reg = Some(reg);
        self.frame_start = start;
        self.frame_end = start + size;
        self.frame_alloc_end = start;
    }

    /// Allocate a stack slot for a local temp that needs spilling.
    /// Returns the offset from frame_reg.
    pub fn alloc_temp_frame(&mut self, tidx: TempIdx) -> i64 {
        let t = self.temp(tidx);
        if t.mem_allocated {
            return t.mem_offset;
        }
        let size = t.ty.size_bytes() as i64;
        // Align to natural size
        self.frame_alloc_end = (self.frame_alloc_end + size - 1) & !(size - 1);
        let offset = self.frame_alloc_end;
        self.frame_alloc_end += size;
        assert!(
            self.frame_alloc_end <= self.frame_end,
            "spill area overflow"
        );
        let t = self.temp_mut(tidx);
        t.mem_allocated = true;
        t.mem_offset = offset;
        offset
    }

    /// Construct a Context from pre-built parts (deserialization).
    pub fn from_raw_parts(
        temps: Vec<Temp>,
        ops: Vec<Op>,
        labels: Vec<Label>,
        nb_globals: u32,
    ) -> Self {
        Self {
            temps,
            ops,
            labels,
            nb_globals,
            frame_reg: None,
            frame_start: 0,
            frame_end: 0,
            frame_alloc_end: 0,
            reserved_regs: RegSet::EMPTY,
            const_table: Default::default(),
            gen_insn_end_off: Vec::new(),
            tb_idx: 0,
            contains_atomic: false,
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::new()
    }
}

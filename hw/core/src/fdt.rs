// Minimal Flattened Devicetree (FDT) blob builder.

use std::collections::HashMap;

const FDT_MAGIC: u32 = 0xd00d_feed;
const FDT_BEGIN_NODE: u32 = 0x0000_0001;
const FDT_END_NODE: u32 = 0x0000_0002;
const FDT_PROP: u32 = 0x0000_0003;
const FDT_END: u32 = 0x0000_0009;

const FDT_VERSION: u32 = 17;
const FDT_LAST_COMP_VERSION: u32 = 16;
const FDT_HEADER_SIZE: u32 = 40;
// One empty reservation entry: address(8) + size(8) = 16 bytes.
const FDT_RSVMAP_SIZE: u32 = 16;

/// Builds a DTB (device tree blob) in memory.
///
/// Usage:
/// ```ignore
/// let mut fdt = FdtBuilder::new();
/// fdt.begin_node("");          // root node
/// fdt.property_string("compatible", "riscv-virtio");
/// fdt.property_u32("#address-cells", 2);
/// fdt.end_node();
/// let dtb = fdt.finish();
/// ```
pub struct FdtBuilder {
    strings: Vec<u8>,
    struct_buf: Vec<u8>,
    string_offsets: HashMap<String, u32>,
}

impl FdtBuilder {
    pub fn new() -> Self {
        Self {
            strings: Vec::new(),
            struct_buf: Vec::new(),
            string_offsets: HashMap::new(),
        }
    }

    /// Start a new node.  Use `""` for the root node.
    pub fn begin_node(&mut self, name: &str) {
        self.push_u32(FDT_BEGIN_NODE);
        self.push_str(name);
    }

    /// Close the current node.
    pub fn end_node(&mut self) {
        self.push_u32(FDT_END_NODE);
    }

    /// Add a `u32` property.
    pub fn property_u32(&mut self, name: &str, val: u32) {
        let data = val.to_be_bytes();
        self.property_bytes(name, &data);
    }

    /// Add a `u64` property (stored as two big-endian u32).
    pub fn property_u64(&mut self, name: &str, val: u64) {
        let data = val.to_be_bytes();
        self.property_bytes(name, &data);
    }

    /// Add a null-terminated string property.
    pub fn property_string(&mut self, name: &str, val: &str) {
        let mut data = val.as_bytes().to_vec();
        data.push(0); // null terminator
        self.property_bytes(name, &data);
    }

    /// Add a list of big-endian `u32` values as a property.
    pub fn property_u32_list(&mut self, name: &str, vals: &[u32]) {
        let mut data = Vec::with_capacity(vals.len() * 4);
        for &v in vals {
            data.extend_from_slice(&v.to_be_bytes());
        }
        self.property_bytes(name, &data);
    }

    /// Add a raw byte-array property.
    pub fn property_bytes(&mut self, name: &str, data: &[u8]) {
        let nameoff = self.intern_string(name);
        self.push_u32(FDT_PROP);
        self.push_u32(data.len() as u32);
        self.push_u32(nameoff);
        self.struct_buf.extend_from_slice(data);
        self.align4();
    }

    /// Consume the builder and produce a complete DTB blob.
    pub fn finish(mut self) -> Vec<u8> {
        self.push_u32(FDT_END);

        let off_mem_rsvmap = FDT_HEADER_SIZE;
        let off_dt_struct = off_mem_rsvmap + FDT_RSVMAP_SIZE;
        let off_dt_strings = off_dt_struct + self.struct_buf.len() as u32;
        let totalsize = off_dt_strings + self.strings.len() as u32;

        let mut blob = Vec::with_capacity(totalsize as usize);

        // -- header (10 × u32 = 40 bytes) --
        blob.extend_from_slice(&FDT_MAGIC.to_be_bytes());
        blob.extend_from_slice(&totalsize.to_be_bytes());
        blob.extend_from_slice(&off_dt_struct.to_be_bytes());
        blob.extend_from_slice(&off_dt_strings.to_be_bytes());
        blob.extend_from_slice(&off_mem_rsvmap.to_be_bytes());
        blob.extend_from_slice(&FDT_VERSION.to_be_bytes());
        blob.extend_from_slice(&FDT_LAST_COMP_VERSION.to_be_bytes());
        // boot_cpuid_phys
        blob.extend_from_slice(&0u32.to_be_bytes());
        // size_dt_strings
        blob.extend_from_slice(&(self.strings.len() as u32).to_be_bytes());
        // size_dt_struct
        blob.extend_from_slice(&(self.struct_buf.len() as u32).to_be_bytes());

        // -- memory reservation map (one empty entry) --
        blob.extend_from_slice(&[0u8; 16]);

        // -- structure block --
        blob.extend_from_slice(&self.struct_buf);

        // -- strings block --
        blob.extend_from_slice(&self.strings);

        blob
    }

    fn push_u32(&mut self, val: u32) {
        self.struct_buf.extend_from_slice(&val.to_be_bytes());
    }

    /// Push a null-terminated string and pad to 4-byte
    /// alignment.
    fn push_str(&mut self, s: &str) {
        self.struct_buf.extend_from_slice(s.as_bytes());
        self.struct_buf.push(0); // null terminator
        self.align4();
    }

    /// Pad `struct_buf` to the next 4-byte boundary.
    fn align4(&mut self) {
        let rem = self.struct_buf.len() % 4;
        if rem != 0 {
            let pad = 4 - rem;
            self.struct_buf.extend(std::iter::repeat_n(0u8, pad));
        }
    }

    /// Deduplicate property name strings and return the
    /// offset into the strings block.
    fn intern_string(&mut self, name: &str) -> u32 {
        if let Some(&off) = self.string_offsets.get(name) {
            return off;
        }
        let off = self.strings.len() as u32;
        self.strings.extend_from_slice(name.as_bytes());
        self.strings.push(0);
        self.string_offsets.insert(name.to_string(), off);
        off
    }
}

impl Default for FdtBuilder {
    fn default() -> Self {
        Self::new()
    }
}

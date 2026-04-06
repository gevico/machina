// GDB CSR register definitions and dynamic XML.
//
// Defines the subset of RISC-V CSRs exposed to GDB
// and generates the target XML dynamically.

/// CSR entry for GDB register mapping.
pub struct CsrEntry {
    pub name: &'static str,
    pub addr: u16,
}

/// Common RISC-V CSRs exposed to GDB (register
/// numbers 66+, in order).
pub const GDB_CSRS: &[CsrEntry] = &[
    CsrEntry {
        name: "mstatus",
        addr: 0x300,
    },
    CsrEntry {
        name: "misa",
        addr: 0x301,
    },
    CsrEntry {
        name: "medeleg",
        addr: 0x302,
    },
    CsrEntry {
        name: "mideleg",
        addr: 0x303,
    },
    CsrEntry {
        name: "mie",
        addr: 0x304,
    },
    CsrEntry {
        name: "mtvec",
        addr: 0x305,
    },
    CsrEntry {
        name: "mcounteren",
        addr: 0x306,
    },
    CsrEntry {
        name: "mscratch",
        addr: 0x340,
    },
    CsrEntry {
        name: "mepc",
        addr: 0x341,
    },
    CsrEntry {
        name: "mcause",
        addr: 0x342,
    },
    CsrEntry {
        name: "mtval",
        addr: 0x343,
    },
    CsrEntry {
        name: "mip",
        addr: 0x344,
    },
    CsrEntry {
        name: "sstatus",
        addr: 0x100,
    },
    CsrEntry {
        name: "sie",
        addr: 0x104,
    },
    CsrEntry {
        name: "stvec",
        addr: 0x105,
    },
    CsrEntry {
        name: "scounteren",
        addr: 0x106,
    },
    CsrEntry {
        name: "sscratch",
        addr: 0x140,
    },
    CsrEntry {
        name: "sepc",
        addr: 0x141,
    },
    CsrEntry {
        name: "scause",
        addr: 0x142,
    },
    CsrEntry {
        name: "stval",
        addr: 0x143,
    },
    CsrEntry {
        name: "sip",
        addr: 0x144,
    },
    CsrEntry {
        name: "satp",
        addr: 0x180,
    },
    CsrEntry {
        name: "pmpcfg0",
        addr: 0x3A0,
    },
    CsrEntry {
        name: "pmpcfg2",
        addr: 0x3A2,
    },
    CsrEntry {
        name: "pmpaddr0",
        addr: 0x3B0,
    },
    CsrEntry {
        name: "pmpaddr1",
        addr: 0x3B1,
    },
    CsrEntry {
        name: "pmpaddr2",
        addr: 0x3B2,
    },
    CsrEntry {
        name: "pmpaddr3",
        addr: 0x3B3,
    },
    CsrEntry {
        name: "mcycle",
        addr: 0xB00,
    },
    CsrEntry {
        name: "minstret",
        addr: 0xB02,
    },
];

/// Number of CSR registers exposed to GDB.
pub const GDB_CSR_COUNT: usize = 30;

/// First GDB register number for CSRs.
pub const GDB_CSR_BASE: usize = 66;

/// GDB register number for the virtual/priv register.
pub const GDB_PRIV_REG: usize = 65;

/// Look up a CSR entry by GDB register number.
pub fn csr_by_gdb_reg(reg: usize) -> Option<&'static CsrEntry> {
    if reg < GDB_CSR_BASE {
        return None;
    }
    GDB_CSRS.get(reg - GDB_CSR_BASE)
}

/// Build the complete target XML with CPU, FPU,
/// virtual, and CSR features.
pub fn build_target_xml() -> String {
    let mut xml = String::with_capacity(8192);
    xml.push_str(
        "<?xml version=\"1.0\"?>\n\
         <!DOCTYPE target SYSTEM \
         \"gdb-target.dtd\">\n\
         <target version=\"1.0\">\n  \
         <architecture>riscv:rv64</architecture>\n",
    );

    // CPU feature (GPR + PC) — inline.
    xml.push_str(&machina_gdbstub::target::RISCV64_CPU_XML.replace(
        "<?xml version=\"1.0\"?>\n\
                 <!DOCTYPE feature SYSTEM \
                 \"gdb-target.dtd\">\n",
        "",
    ));

    // FPU feature — inline.
    xml.push_str(&machina_gdbstub::target::RISCV64_FPU_XML.replace(
        "<?xml version=\"1.0\"?>\n\
                 <!DOCTYPE feature SYSTEM \
                 \"gdb-target.dtd\">\n",
        "",
    ));

    // Virtual/privilege register.
    xml.push_str(
        "  <feature name=\"\
         org.gnu.gdb.riscv.virtual\">\n\
             <reg name=\"priv\" bitsize=\"64\" \
         type=\"int\"/>\n\
         </feature>\n",
    );

    // CSR feature.
    xml.push_str(
        "  <feature name=\"\
         org.gnu.gdb.riscv.csr\">\n",
    );
    for entry in GDB_CSRS {
        xml.push_str(&format!(
            "    <reg name=\"{}\" \
             bitsize=\"64\" type=\"int\"/>\n",
            entry.name,
        ));
    }
    xml.push_str("  </feature>\n");

    xml.push_str("</target>\n");
    xml
}

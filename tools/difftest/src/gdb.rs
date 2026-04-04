// GDB Remote Serial Protocol client for difftest.
//
// Implements the minimal subset of GDB RSP needed to
// drive QEMU as a reference model: connect, single-step,
// read/write registers, write memory.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Number of RISC-V GPRs + PC in GDB register file.
const GDB_NUM_REGS: usize = 33;
/// Bytes per 64-bit register in the `g` response.
const REG_BYTES: usize = 8;

/// GDB RSP client connected to a QEMU gdbstub.
pub struct GdbClient {
    stream: TcpStream,
    no_ack: bool,
    buf: Vec<u8>,
}

/// RISC-V register state as seen by GDB.
#[derive(Clone, Debug)]
pub struct RegState {
    /// x0..x31 (32 GPRs) + pc.
    pub regs: [u64; GDB_NUM_REGS],
}

impl RegState {
    pub fn pc(&self) -> u64 {
        self.regs[32]
    }

    pub fn gpr(&self, i: usize) -> u64 {
        self.regs[i]
    }
}

impl GdbClient {
    /// Connect to QEMU's GDB stub at the given address.
    /// Retries up to `retries` times with 200ms delay.
    pub fn connect(addr: &str, retries: u32) -> io::Result<Self> {
        let mut last_err = io::Error::other("no attempts");
        for i in 0..retries {
            match TcpStream::connect(addr) {
                Ok(stream) => {
                    stream.set_nodelay(true)?;
                    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
                    let mut client = Self {
                        stream,
                        no_ack: false,
                        buf: Vec::with_capacity(4096),
                    };
                    client.negotiate()?;
                    return Ok(client);
                }
                Err(e) => {
                    last_err = e;
                    if i + 1 < retries {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
        }
        Err(last_err)
    }

    /// Negotiate features and enable no-ack mode.
    fn negotiate(&mut self) -> io::Result<()> {
        let resp = self.command("qSupported")?;
        if resp.contains("QStartNoAckMode+") {
            let ack_resp = self.command("QStartNoAckMode")?;
            if ack_resp == "OK" {
                self.no_ack = true;
            }
        }
        Ok(())
    }

    /// Send a GDB RSP command and return the response.
    pub fn command(&mut self, cmd: &str) -> io::Result<String> {
        self.send_packet(cmd)?;
        self.recv_packet()
    }

    /// Send a raw RSP packet: $<data>#<checksum>.
    fn send_packet(&mut self, data: &str) -> io::Result<()> {
        let cksum = data
            .as_bytes()
            .iter()
            .fold(0u8, |acc, &b| acc.wrapping_add(b));
        write!(self.stream, "${}#{:02x}", data, cksum)?;
        self.stream.flush()?;
        if !self.no_ack {
            self.wait_ack()?;
        }
        Ok(())
    }

    /// Wait for '+' ACK from the stub.
    fn wait_ack(&mut self) -> io::Result<()> {
        let mut b = [0u8; 1];
        self.stream.read_exact(&mut b)?;
        if b[0] != b'+' {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected '+', got {:?}", b[0] as char),
            ));
        }
        Ok(())
    }

    /// Receive one RSP packet, stripping framing.
    fn recv_packet(&mut self) -> io::Result<String> {
        self.buf.clear();
        let mut b = [0u8; 1];

        // Skip until '$'.
        loop {
            self.stream.read_exact(&mut b)?;
            if b[0] == b'$' {
                break;
            }
        }

        // Read until '#'.
        loop {
            self.stream.read_exact(&mut b)?;
            if b[0] == b'#' {
                break;
            }
            self.buf.push(b[0]);
        }

        // Read 2-byte checksum (we don't verify it).
        let mut ck = [0u8; 2];
        self.stream.read_exact(&mut ck)?;

        // Send ACK if not in no-ack mode.
        if !self.no_ack {
            self.stream.write_all(b"+")?;
            self.stream.flush()?;
        }

        String::from_utf8(self.buf.clone())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Single-step one instruction on thread 1.
    pub fn step(&mut self) -> io::Result<()> {
        let resp = self.command("vCont;s:1")?;
        // Expect a stop reply like "T05..." or "S05".
        if !resp.starts_with('T') && !resp.starts_with('S') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected step reply: {}", resp),
            ));
        }
        Ok(())
    }

    /// Read all registers (gpr[0..31] + pc).
    pub fn read_regs(&mut self) -> io::Result<RegState> {
        let hex = self.command("g")?;
        parse_regs_hex(&hex)
    }

    /// Write gpr[0..31] + pc, preserving other registers
    /// (FP, CSRs) from the current QEMU state.
    pub fn write_regs(&mut self, state: &RegState) -> io::Result<()> {
        // Read current full register hex from QEMU.
        let full_hex = self.command("g")?;
        if full_hex.len() < GDB_NUM_REGS * REG_BYTES * 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "g response too short for write_regs",
            ));
        }
        // Overwrite GPR + PC portion (first 33 regs).
        let mut new_hex = encode_regs_hex(state);
        new_hex.push_str(&full_hex[GDB_NUM_REGS * REG_BYTES * 2..]);
        let resp = self.command(&format!("G{}", new_hex))?;
        if resp != "OK" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("write_regs failed: {}", resp),
            ));
        }
        Ok(())
    }

    /// Write memory at `addr` with `data`.
    pub fn write_mem(&mut self, addr: u64, data: &[u8]) -> io::Result<()> {
        let hex: String = data.iter().map(|b| format!("{:02x}", b)).collect();
        let cmd = format!("M{:x},{:x}:{}", addr, data.len(), hex);
        let resp = self.command(&cmd)?;
        if resp != "OK" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("write_mem failed: {}", resp),
            ));
        }
        Ok(())
    }

    /// Send continue command.
    pub fn cont(&mut self) -> io::Result<String> {
        self.command("c")
    }

    /// Step N instructions on REF. Uses vCont;s in a loop.
    pub fn step_n(&mut self, n: u64) -> io::Result<()> {
        for _ in 0..n {
            self.step()?;
        }
        Ok(())
    }

    /// Set a software breakpoint at `addr`.
    pub fn set_breakpoint(&mut self, addr: u64) -> io::Result<()> {
        let resp = self.command(&format!("Z0,{:x},4", addr))?;
        if resp != "OK" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("set_breakpoint failed: {}", resp),
            ));
        }
        Ok(())
    }

    /// Remove a software breakpoint at `addr`.
    pub fn remove_breakpoint(&mut self, addr: u64) -> io::Result<()> {
        let resp = self.command(&format!("z0,{:x},4", addr))?;
        if resp != "OK" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("remove_breakpoint failed: {}", resp),
            ));
        }
        Ok(())
    }
}

/// Parse hex register dump from `g` command.
/// RISC-V 64-bit: 33 regs × 8 bytes = 264 bytes = 528 hex
/// chars (minimum). QEMU may send more (FP regs etc.).
fn parse_regs_hex(hex: &str) -> io::Result<RegState> {
    let min_len = GDB_NUM_REGS * REG_BYTES * 2;
    if hex.len() < min_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("g response too short: {} < {}", hex.len(), min_len),
        ));
    }
    let mut state = RegState {
        regs: [0u64; GDB_NUM_REGS],
    };
    for i in 0..GDB_NUM_REGS {
        let off = i * REG_BYTES * 2;
        let s = &hex[off..off + REG_BYTES * 2];
        state.regs[i] = parse_le_hex_u64(s)?;
    }
    Ok(state)
}

/// Parse a 16-char little-endian hex string to u64.
fn parse_le_hex_u64(s: &str) -> io::Result<u64> {
    if s.len() != 16 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected 16 hex chars, got {}", s.len()),
        ));
    }
    let mut bytes = [0u8; 8];
    for i in 0..8 {
        bytes[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    }
    Ok(u64::from_le_bytes(bytes))
}

/// Encode register state as hex for `G` command.
fn encode_regs_hex(state: &RegState) -> String {
    let mut hex = String::with_capacity(GDB_NUM_REGS * REG_BYTES * 2);
    for &val in &state.regs {
        for b in val.to_le_bytes() {
            hex.push_str(&format!("{:02x}", b));
        }
    }
    hex
}

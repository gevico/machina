// GDB stub unit tests.
//
// Tests protocol encoding/decoding, GdbHandler command
// dispatch, and GdbState breakpoint management.

use machina_gdbstub::handler::{GdbHandler, GdbTarget, StopReason};
use machina_gdbstub::protocol;
use machina_system::gdb::{GdbRunState, GdbState};

fn tcp_bind_available() -> bool {
    std::net::TcpListener::bind("127.0.0.1:0").is_ok()
}

// ── Protocol tests ──────────────────────────────────

#[test]
fn test_encode_hex_bytes() {
    let data = [0x00, 0x01, 0xfe, 0xff];
    assert_eq!(protocol::encode_hex_bytes(&data), "0001feff",);
}

#[test]
fn test_decode_hex_bytes() {
    let decoded = protocol::decode_hex_bytes("0001feff").unwrap();
    assert_eq!(decoded, vec![0x00, 0x01, 0xfe, 0xff]);
}

#[test]
fn test_decode_hex_bytes_odd_len() {
    assert!(protocol::decode_hex_bytes("abc").is_err());
}

#[test]
fn test_parse_hex() {
    assert_eq!(protocol::parse_hex("0"), 0);
    assert_eq!(protocol::parse_hex("ff"), 255);
    assert_eq!(protocol::parse_hex("deadbeef"), 0xdeadbeef,);
    assert_eq!(
        protocol::parse_hex("123456789abcdef0"),
        0x1234_5678_9abc_def0,
    );
}

#[test]
fn test_encode_reg_hex() {
    assert_eq!(protocol::encode_reg_hex(1u64), "0100000000000000",);
    assert_eq!(protocol::encode_reg_hex(0xdead_beef), "efbeadde00000000",);
}

#[test]
fn test_decode_reg_hex() {
    assert_eq!(protocol::decode_reg_hex("0100000000000000"), 1,);
    assert_eq!(protocol::decode_reg_hex("efbeadde00000000"), 0xdead_beef,);
}

#[test]
fn test_send_packet_format() {
    let mut buf = std::io::Cursor::new(Vec::new());
    protocol::send_packet(&mut buf, "OK").unwrap();
    assert_eq!(&buf.into_inner(), b"$OK#9a");
}

#[test]
fn test_recv_packet_valid() {
    let data = b"+$OK#9a+".to_vec();
    let mut cursor = std::io::Cursor::new(data);
    let pkt = protocol::recv_packet(&mut cursor).unwrap();
    assert_eq!(pkt, "OK");
}

#[test]
fn test_recv_packet_checksum_mismatch() {
    let data = b"$OK#00+".to_vec();
    let mut cursor = std::io::Cursor::new(data);
    assert!(protocol::recv_packet(&mut cursor).is_err());
}

#[test]
fn test_recv_packet_ctrl_c() {
    // 0x03 byte should return "\x03".
    let data = vec![0x03];
    let mut cursor = std::io::Cursor::new(data);
    // recv_packet will block waiting for '$' after the
    // interrupt byte. Test the 0x03 path by providing it
    // followed by a valid packet.
    let data = b"\x03$S05#b8+".to_vec();
    let mut cursor = std::io::Cursor::new(data);
    let pkt = protocol::recv_packet(&mut cursor).unwrap();
    assert_eq!(pkt, "\x03");
}

// ── GdbHandler tests ────────────────────────────────

/// A minimal mock GdbTarget for testing command dispatch.
struct MockTarget {
    regs: Vec<u8>,
    mem: Vec<u8>,
    pc: u64,
}

impl MockTarget {
    fn new() -> Self {
        let mut regs = vec![0u8; 65 * 8];
        let pc_bytes = 0x8020_0000u64.to_le_bytes();
        regs[256..264].copy_from_slice(&pc_bytes);
        Self {
            regs,
            mem: vec![0; 4096],
            pc: 0x8020_0000,
        }
    }
}

impl GdbTarget for MockTarget {
    fn read_registers(&self) -> Vec<u8> {
        self.regs.clone()
    }

    fn write_registers(&mut self, data: &[u8]) -> bool {
        if data.len() == self.regs.len() {
            self.regs.copy_from_slice(data);
            true
        } else {
            false
        }
    }

    fn read_register(&self, reg: usize) -> Vec<u8> {
        let off = match reg {
            0..=31 => reg * 8,
            32 => 256,
            33..=64 => (reg - 33) * 8 + 264,
            _ => return Vec::new(),
        };
        if off + 8 <= self.regs.len() {
            self.regs[off..off + 8].to_vec()
        } else {
            Vec::new()
        }
    }

    fn write_register(&mut self, reg: usize, val: &[u8]) -> bool {
        if val.len() != 8 {
            return false;
        }
        let off = match reg {
            0..=31 => reg * 8,
            32 => 256,
            33..=64 => (reg - 33) * 8 + 264,
            _ => return false,
        };
        if off + 8 <= self.regs.len() {
            self.regs[off..off + 8].copy_from_slice(val);
            true
        } else {
            false
        }
    }

    fn read_memory(&self, addr: u64, len: usize) -> Vec<u8> {
        let off = addr as usize;
        if off + len <= self.mem.len() {
            self.mem[off..off + len].to_vec()
        } else {
            vec![0; len]
        }
    }

    fn write_memory(&mut self, addr: u64, data: &[u8]) -> bool {
        let off = addr as usize;
        if off + data.len() <= self.mem.len() {
            self.mem[off..off + data.len()].copy_from_slice(data);
            true
        } else {
            false
        }
    }

    fn set_breakpoint(&mut self, _type_: u8, _addr: u64, _kind: u32) -> bool {
        true
    }

    fn remove_breakpoint(
        &mut self,
        _type_: u8,
        _addr: u64,
        _kind: u32,
    ) -> bool {
        true
    }

    fn resume(&mut self) {}
    fn step(&mut self) {}
    fn get_pc(&self) -> u64 {
        self.pc
    }
    fn get_stop_reason(&self) -> StopReason {
        StopReason::Breakpoint
    }
}

#[test]
fn test_handler_stop_reason() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert_eq!(h.handle("?", &mut t).unwrap(), "T05thread:01;swbreak:;");
}

#[test]
fn test_handler_read_registers() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    let resp = h.handle("g", &mut t).unwrap();
    // 65 regs * 8 bytes * 2 hex chars = 1040.
    assert_eq!(resp.len(), 1040);
}

#[test]
fn test_handler_write_registers() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    let zeros = "0".repeat(1040);
    assert_eq!(h.handle(&format!("G{}", zeros), &mut t).unwrap(), "OK",);
    assert!(t.regs.iter().all(|&b| b == 0));
}

#[test]
fn test_handler_write_registers_bad_len() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert_eq!(h.handle("G00", &mut t).unwrap(), "E01",);
}

#[test]
fn test_handler_read_register_pc() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    // p20 = register 32 (0x20) = PC.
    assert_eq!(h.handle("p20", &mut t).unwrap(), "0000208000000000",);
}

#[test]
fn test_handler_write_register() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    // P20=<new_pc_hex>.
    assert_eq!(h.handle("P20=0000408000000000", &mut t).unwrap(), "OK",);
    let pc = u64::from_le_bytes(t.regs[256..264].try_into().unwrap());
    assert_eq!(pc, 0x8040_0000);
}

#[test]
fn test_handler_read_memory() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    t.mem[0..4].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    assert_eq!(h.handle("m0,4", &mut t).unwrap(), "deadbeef",);
}

#[test]
fn test_handler_write_memory() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert_eq!(h.handle("M0,4:cafebabe", &mut t).unwrap(), "OK",);
    assert_eq!(&t.mem[0..4], &[0xca, 0xfe, 0xba, 0xbe]);
}

#[test]
fn test_handler_breakpoint_add_remove() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert_eq!(h.handle("Z0,80200000,4", &mut t).unwrap(), "OK",);
    assert_eq!(h.handle("z0,80200000,4", &mut t).unwrap(), "OK",);
}

#[test]
fn test_handler_query_supported() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    let resp = h.handle("qSupported:foo+", &mut t).unwrap();
    assert!(resp.contains("multiprocess+"));
    assert!(resp.contains("vContSupported+"));
    assert!(resp.contains("PacketSize=4000"));
    assert!(resp.contains("qXfer:features:read+"));
}

#[test]
fn test_handler_query_attached() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert_eq!(h.handle("qAttached", &mut t).unwrap(), "1",);
}

#[test]
fn test_handler_query_thread_info() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert_eq!(h.handle("qfThreadInfo", &mut t).unwrap(), "m01",);
    assert_eq!(h.handle("qsThreadInfo", &mut t).unwrap(), "l",);
}

#[test]
fn test_handler_set_thread() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert_eq!(h.handle("Hg0", &mut t).unwrap(), "OK");
    assert_eq!(h.handle("Hc1", &mut t).unwrap(), "OK");
}

#[test]
fn test_handler_thread_alive() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert_eq!(h.handle("T1", &mut t).unwrap(), "OK");
}

#[test]
fn test_handler_unknown() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    // 'w' is not a recognized command.
    assert_eq!(h.handle("w", &mut t).unwrap(), "");
}

#[test]
fn test_handler_detach() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert!(h.handle("D", &mut t).is_none());
}

#[test]
fn test_handler_kill() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert!(h.handle("k", &mut t).is_none());
}

#[test]
fn test_handler_vcont_query() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    let resp = h.handle("vCont?", &mut t).unwrap();
    assert!(resp.contains("c"));
    assert!(resp.contains("s"));
}

#[test]
fn test_handler_vcont_step() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert_eq!(h.handle("vCont;s:1", &mut t).unwrap(), "T05thread:01;",);
}

#[test]
fn test_handler_no_ack() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert_eq!(h.handle("QStartNoAckMode", &mut t).unwrap(), "OK",);
    assert!(h.no_ack());
}

#[test]
fn test_handler_ctrl_c() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    assert_eq!(h.handle("\x03", &mut t).unwrap(), "T02thread:01;",);
}

// ── GdbState tests ──────────────────────────────────

#[test]
fn test_gdb_state_initial() {
    let gs = GdbState::new();
    assert_eq!(gs.run_state(), GdbRunState::Paused);
    assert!(!gs.is_connected());
    assert!(!gs.is_stepping());
    assert!(!gs.has_breakpoints());
}

#[test]
fn test_gdb_state_connected() {
    let gs = GdbState::new();
    gs.set_connected(true);
    assert!(gs.is_connected());
    gs.set_connected(false);
    assert!(!gs.is_connected());
}

#[test]
fn test_gdb_state_breakpoints() {
    let gs = GdbState::new();
    assert!(!gs.has_breakpoints());
    assert!(!gs.hit_breakpoint(0x8020_0000));

    gs.set_breakpoint(0x8020_0000);
    assert!(gs.has_breakpoints());
    assert!(gs.hit_breakpoint(0x8020_0000));
    assert!(!gs.hit_breakpoint(0x8020_0004));

    gs.remove_breakpoint(0x8020_0000);
    assert!(!gs.has_breakpoints());
    assert!(!gs.hit_breakpoint(0x8020_0000));
}

#[test]
fn test_gdb_state_hw_breakpoints() {
    let gs = GdbState::new();
    gs.set_hw_breakpoint(0x1000);
    assert!(gs.has_breakpoints());
    assert!(gs.hit_breakpoint(0x1000));

    gs.remove_hw_breakpoint(0x1000);
    assert!(!gs.has_breakpoints());
}

#[test]
fn test_gdb_state_multiple_breakpoints() {
    let gs = GdbState::new();
    gs.set_breakpoint(0x1000);
    gs.set_breakpoint(0x2000);
    gs.set_breakpoint(0x3000);
    assert!(gs.hit_breakpoint(0x1000));
    assert!(gs.hit_breakpoint(0x2000));
    assert!(gs.hit_breakpoint(0x3000));
    assert!(!gs.hit_breakpoint(0x4000));

    gs.remove_breakpoint(0x2000);
    assert!(gs.hit_breakpoint(0x1000));
    assert!(!gs.hit_breakpoint(0x2000));
    assert!(gs.hit_breakpoint(0x3000));
}

#[test]
fn test_gdb_state_set_duplicate_breakpoint() {
    let gs = GdbState::new();
    gs.set_breakpoint(0x1000);
    gs.set_breakpoint(0x1000); // duplicate, no panic.
    assert!(gs.hit_breakpoint(0x1000));
}

#[test]
fn test_gdb_state_remove_nonexistent() {
    let gs = GdbState::new();
    gs.remove_breakpoint(0x9999); // no panic.
    assert!(!gs.has_breakpoints());
}

#[test]
fn test_gdb_state_detach() {
    let gs = GdbState::new();
    gs.set_connected(true);
    gs.detach();
    assert!(!gs.is_connected());
    assert!(gs.is_detached());
    assert_eq!(gs.run_state(), GdbRunState::Running);
}

#[test]
fn test_gdb_state_request_resume() {
    let gs = GdbState::new();
    assert_eq!(gs.run_state(), GdbRunState::Paused);
    gs.request_resume();
    assert_eq!(gs.run_state(), GdbRunState::Running);
}

#[test]
fn test_gdb_state_request_step() {
    let gs = GdbState::new();
    gs.request_resume();
    gs.request_step();
    assert!(gs.is_stepping());
    assert_eq!(gs.run_state(), GdbRunState::Stepping);
}

#[test]
fn test_gdb_state_request_pause() {
    let gs = GdbState::new();
    gs.request_resume();
    assert_eq!(gs.run_state(), GdbRunState::Running);
    gs.request_pause();
    assert_eq!(gs.run_state(), GdbRunState::PauseRequested);
}

// ── Integration: handler + mock target round-trip ──

#[test]
fn test_register_write_read_roundtrip() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    // Write x5 = 0x4242424242424242.
    let val = "4242424242424242";
    assert_eq!(h.handle(&format!("P5={}", val), &mut t).unwrap(), "OK",);
    // Read it back.
    assert_eq!(h.handle("p5", &mut t).unwrap(), val,);
}

#[test]
fn test_memory_write_read_roundtrip() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    // Write 8 bytes at addr 0x100.
    assert_eq!(h.handle("M100,8:0102030405060708", &mut t).unwrap(), "OK",);
    // Read back.
    assert_eq!(h.handle("m100,8", &mut t).unwrap(), "0102030405060708",);
}

// ── qXfer target XML tests ─────────────────────────────

#[test]
fn test_handler_qxfer_target_xml() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    // Read first chunk of target.xml.
    let resp = h
        .handle("qXfer:features:read:target.xml:0,fff", &mut t)
        .unwrap();
    assert!(resp.starts_with('m') || resp.starts_with('l'));
    let xml = &resp[1..];
    assert!(xml.contains("org.gnu.gdb.riscv.cpu"));
    assert!(xml.contains("org.gnu.gdb.riscv.fpu"));
    assert!(xml.contains("riscv:rv64"));
}

#[test]
fn test_handler_qxfer_nonexistent() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    let resp = h
        .handle("qXfer:features:read:nonexistent.xml:0,fff", &mut t)
        .unwrap();
    assert!(resp.is_empty());
}

#[test]
fn test_handler_qxfer_offset_beyond_end() {
    let mut h = GdbHandler::new();
    let mut t = MockTarget::new();
    // Offset beyond XML length returns 'l' (end).
    let resp = h
        .handle("qXfer:features:read:target.xml:fffff,fff", &mut t)
        .unwrap();
    assert_eq!(resp, "l");
}

// ── Round 1: stop_reason tracking tests ───────────

#[test]
fn test_gdb_state_stop_reason_default() {
    let gs = GdbState::new();
    assert_eq!(gs.get_stop_reason(), StopReason::Pause);
}

#[test]
fn test_gdb_state_stop_reason_breakpoint() {
    let gs = GdbState::new();
    gs.set_stop_reason(StopReason::Breakpoint);
    assert_eq!(gs.get_stop_reason(), StopReason::Breakpoint);
}

#[test]
fn test_gdb_state_stop_reason_step() {
    let gs = GdbState::new();
    gs.set_stop_reason(StopReason::Step);
    assert_eq!(gs.get_stop_reason(), StopReason::Step);
}

#[test]
fn test_gdb_state_stop_reason_pause() {
    let gs = GdbState::new();
    gs.set_stop_reason(StopReason::Pause);
    assert_eq!(gs.get_stop_reason(), StopReason::Pause);
}

#[test]
fn test_gdb_state_snapshot_on_step() {
    // Verify that save_snapshot updates the snapshot and
    // the stop_reason is tracked independently.
    let gs = GdbState::new();
    let gpr = [0u64; 32];
    let mut fpr = [0u64; 32];
    fpr[0] = 0xcafe;
    gs.save_snapshot(0, &gpr, &fpr, 0x8020_0000, 3, &[]);
    gs.set_stop_reason(StopReason::Step);

    let snap = gs.read_snapshot();
    assert_eq!(snap.pc, 0x8020_0000);
    assert_eq!(snap.fpr[0], 0xcafe);
    assert_eq!(gs.get_stop_reason(), StopReason::Step);
}

#[test]
fn test_gdb_state_wait_paused_timeout() {
    let gs = GdbState::new();
    // Already paused -> should return true immediately.
    assert!(gs.wait_paused_timeout(std::time::Duration::from_millis(10)));

    // Resume -> not paused, timeout should return false.
    gs.request_resume();
    assert!(!gs.wait_paused_timeout(std::time::Duration::from_millis(10)));
}

#[test]
fn test_gdb_state_breakpoint_triggers_pause() {
    let gs = GdbState::new();
    gs.set_connected(true);
    gs.request_resume();
    assert_eq!(gs.run_state(), GdbRunState::Running);

    // Simulate breakpoint hit: set stop reason + request
    // pause.
    gs.set_stop_reason(StopReason::Breakpoint);
    gs.request_pause();
    assert_eq!(gs.run_state(), GdbRunState::PauseRequested);
    assert_eq!(gs.get_stop_reason(), StopReason::Breakpoint,);
}

// ── Round 1: resume/step flow tests ──────────────

// check_resume_packet is private; tested indirectly
// through the serve() flow. The GdbState methods it
// relies on (request_resume, request_step, wait_paused,
// get_stop_reason) are tested via public API below.

#[test]
fn test_gdb_step_saves_snapshot() {
    let gs = GdbState::new();
    gs.set_connected(true);

    // Simulate exec loop: save snapshot, set stop reason,
    // then the state would transition via complete_step().
    let gpr = {
        let mut g = [0u64; 32];
        g[1] = 0x1234;
        g
    };
    let fpr = [0u64; 32];
    gs.save_snapshot(0, &gpr, &fpr, 0x8030_0000, 3, &[]);
    gs.set_stop_reason(StopReason::Step);

    // Verify the snapshot reflects post-step state.
    let snap = gs.read_snapshot();
    assert_eq!(snap.pc, 0x8030_0000);
    assert_eq!(snap.gpr[1], 0x1234);
    assert_eq!(gs.get_stop_reason(), StopReason::Step);
}

// ── Round 2: Integration test against real serve() path ──

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

/// Mock exec loop thread that simulates the CPU side of
/// the GDB coordination. Advances a virtual PC, checks
/// breakpoints, and responds to resume/step/pause.
fn mock_exec_loop(gs: &GdbState) {
    let mut gpr = [0u64; 32];
    gpr[2] = 0x8080_0000; // sp
    gpr[3] = 0x8020_0000; // gp
    let mut fpr = [0u64; 32];
    let mut pc: u64 = 0x8020_0000;

    // Initial pause: save snapshot and park.
    gs.save_snapshot(0, &gpr, &fpr, pc, 3, &[]);
    if gs.check_and_wait() {
        return;
    }

    loop {
        if !gs.is_connected() || gs.is_detached() {
            break;
        }

        match gs.run_state() {
            GdbRunState::Stepping => {
                // Apply dirty register writes from GDB.
                if let Some(snap) = gs.take_dirty_snapshot(0) {
                    for i in 1..32 {
                        gpr[i] = snap.gpr[i];
                    }
                    pc = snap.pc;
                    for i in 0..32 {
                        fpr[i] = snap.fpr[i];
                    }
                }
                pc += 4;
                gpr[1] = pc; // ra = stepped addr
                gs.save_snapshot(0, &gpr, &fpr, pc, 3, &[]);
                gs.set_stop_reason(StopReason::Step);
                gs.complete_step();
            }
            GdbRunState::Running => {
                // Apply dirty register writes.
                if let Some(snap) = gs.take_dirty_snapshot(0) {
                    for i in 1..32 {
                        gpr[i] = snap.gpr[i];
                    }
                    pc = snap.pc;
                }
                // Simulate execution: advance PC, check
                // breakpoints and pause requests.
                loop {
                    pc += 4;
                    if gs.has_breakpoints() && gs.hit_breakpoint(pc) {
                        gpr[1] = pc;
                        gs.save_snapshot(0, &gpr, &fpr, pc, 3, &[]);
                        gs.set_stop_reason(StopReason::Breakpoint);
                        gs.request_pause();
                        break;
                    }
                    let state = gs.run_state();
                    if state == GdbRunState::PauseRequested
                        || state == GdbRunState::Paused
                    {
                        gpr[1] = pc;
                        gs.save_snapshot(0, &gpr, &fpr, pc, 3, &[]);
                        gs.set_stop_reason(StopReason::Pause);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                // Park until resumed.
                if gs.check_and_wait() {
                    break;
                }
            }
            _ => {
                // Paused: just wait.
                if gs.check_and_wait() {
                    break;
                }
            }
        }
    }
}

// RSP helpers for the test client side.

fn rsp_send(stream: &mut TcpStream, packet: &str) {
    let checksum: u8 = packet.bytes().fold(0u8, |a, b| a.wrapping_add(b));
    let msg = format!("${}#{:02x}", packet, checksum);
    stream.write_all(msg.as_bytes()).unwrap();
    stream.flush().unwrap();
    // Consume ACK ('+') if present.
    let mut ack = [0u8; 1];
    let _ = stream.read(&mut ack);
}

fn rsp_recv(stream: &mut TcpStream) -> String {
    // Skip '+' ACK bytes.
    let mut buf = [0u8; 1];
    loop {
        stream.read_exact(&mut buf).unwrap();
        if buf[0] == b'$' {
            break;
        }
    }
    let mut data = Vec::new();
    loop {
        stream.read_exact(&mut buf).unwrap();
        if buf[0] == b'#' {
            break;
        }
        data.push(buf[0]);
    }
    // Read 2-char checksum.
    stream.read_exact(&mut buf).unwrap();
    stream.read_exact(&mut buf).unwrap();
    String::from_utf8(data).unwrap()
}

/// Helper: decode LE hex bytes to u64.
fn u8hex_to_u64(hex: &str) -> u64 {
    let bytes: Vec<u8> = (0..8)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
        .collect();
    u64::from_le_bytes(bytes.try_into().unwrap())
}

#[test]
fn test_serve_integration_full_path() {
    if !tcp_bind_available() {
        eprintln!("skipping: TCP bind not permitted");
        return;
    }
    // Allocate RAM for memory tests.
    let ram_size: u64 = 1024 * 1024; // 1 MiB
    let mut ram = vec![0u8; ram_size as usize];
    // Write a known pattern at offset 0.
    ram[0..4].copy_from_slice(&[0xde, 0xad, 0xbe, 0xef]);

    // Create GdbState and configure memory access.
    let gs = Arc::new(GdbState::new());
    gs.set_mem_access(ram.as_ptr(), ram_size, 0x8000_0000, 0);
    gs.set_connected(true);

    // Start mock exec loop thread.
    let gs_exec = gs.clone();
    let exec_handle = std::thread::Builder::new()
        .name("mock-exec".into())
        .spawn(move || {
            mock_exec_loop(&gs_exec);
        })
        .unwrap();

    // TCP listener on random port.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    // Start serve() in a thread.
    let gs_serve = gs.clone();
    let serve_handle = std::thread::Builder::new()
        .name("gdb-serve".into())
        .spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            machina_system::gdb::serve(stream, &gs_serve).unwrap();
        })
        .unwrap();

    // Connect as GDB client.
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    // ── 1. Initial stop reply ──
    let init = rsp_recv(&mut stream);
    assert_eq!(init, "T05thread:01;", "initial stop: no swbreak claim");

    // ── 2. Enable no-ack mode ──
    rsp_send(&mut stream, "QStartNoAckMode");
    assert_eq!(rsp_recv(&mut stream), "OK");

    // ── 3. qXfer target XML ──
    rsp_send(&mut stream, "qXfer:features:read:target.xml:0,fff");
    let xml_resp = rsp_recv(&mut stream);
    assert!(
        xml_resp.starts_with('m') || xml_resp.starts_with('l'),
        "qXfer prefix"
    );
    let xml = &xml_resp[1..];
    assert!(xml.contains("org.gnu.gdb.riscv.cpu"));
    assert!(xml.contains("org.gnu.gdb.riscv.fpu"));

    // ── 4. Read all registers (g) ──
    rsp_send(&mut stream, "g");
    let regs = rsp_recv(&mut stream);
    assert_eq!(regs.len(), 1040, "65 regs * 8 bytes * 2 hex");

    // ── 5. Read PC (p20 = reg 32) ──
    rsp_send(&mut stream, "p20");
    let pc_hex = rsp_recv(&mut stream);
    assert_ne!(pc_hex, "0000000000000000", "PC not zero");

    // ── 6. Write + read x5 (P5/p5) ──
    rsp_send(&mut stream, "P5=4242424242424242");
    assert_eq!(rsp_recv(&mut stream), "OK");
    rsp_send(&mut stream, "p5");
    assert_eq!(rsp_recv(&mut stream), "4242424242424242", "P5/p5 roundtrip");

    // ── 7. Memory read (m) ──
    rsp_send(&mut stream, "m80000000,4");
    assert_eq!(rsp_recv(&mut stream), "deadbeef", "RAM read");

    // ── 8. Memory write + read (M/m) ──
    rsp_send(&mut stream, "M80000004,4:cafebabe");
    assert_eq!(rsp_recv(&mut stream), "OK");
    rsp_send(&mut stream, "m80000004,4");
    assert_eq!(rsp_recv(&mut stream), "cafebabe", "RAM write/read");

    // ── 9. Step (s) ──
    rsp_send(&mut stream, "s");
    assert_eq!(rsp_recv(&mut stream), "T05thread:01;", "step stop reply");
    rsp_send(&mut stream, "p20");
    let pc_after = rsp_recv(&mut stream);
    assert_ne!(pc_after, pc_hex, "PC changed after step");

    // ── 10. Set breakpoint + continue ──
    let cur_pc = u8hex_to_u64(&pc_after);
    let bp_addr = cur_pc + 4;
    rsp_send(&mut stream, &format!("Z0,{:x},4", bp_addr));
    assert_eq!(rsp_recv(&mut stream), "OK");
    rsp_send(&mut stream, "c");
    let bp_reply = rsp_recv(&mut stream);
    assert!(bp_reply.contains("swbreak"), "bp hit: {}", bp_reply,);

    // ── 11. Remove breakpoint + continue + Ctrl-C ──
    rsp_send(&mut stream, &format!("z0,{:x},4", bp_addr));
    assert_eq!(rsp_recv(&mut stream), "OK");
    rsp_send(&mut stream, "c");
    // Brief delay then Ctrl-C.
    std::thread::sleep(Duration::from_millis(100));
    stream.write_all(&[0x03]).unwrap();
    stream.flush().unwrap();
    let ctrl_reply = rsp_recv(&mut stream);
    assert!(ctrl_reply.starts_with("T02"), "Ctrl-C: {}", ctrl_reply,);

    // ── 12. Detach ──
    rsp_send(&mut stream, "D");
    let _ = rsp_recv(&mut stream);

    // Cleanup: wait for threads.
    stream.shutdown(std::net::Shutdown::Both).ok();
    let _ = serve_handle.join();
    let _ = exec_handle.join();
}

// ── Production path: subprocess GDB integration test ──

use std::process::{Child, Command, Stdio};

fn project_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn bin_path(name: &str) -> std::path::PathBuf {
    let base = project_root().join("target").join("debug").join(name);
    if cfg!(windows) {
        base.with_extension("exe")
    } else {
        base
    }
}

fn sifive_pass_bin() -> std::path::PathBuf {
    project_root().join("tests/firmware/sifive_pass.bin")
}

fn ensure_machina_built() {
    // Serialise concurrent builds: on Windows multiple linkers
    // cannot write the same .exe simultaneously.
    use std::sync::{Mutex, OnceLock};
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = LOCK.get_or_init(Mutex::default).lock().unwrap();

    let status = Command::new("cargo")
        .args(["build", "-p", "machina-emu"])
        .current_dir(project_root())
        .status()
        .expect("cargo build machina-emu failed");
    assert!(status.success(), "cargo build machina-emu failed");
}

/// RSP recv that tolerates connection close (process exit).
fn rsp_recv_or_eof(stream: &mut TcpStream) -> Option<String> {
    let mut buf = [0u8; 1];
    // Skip '+' ACK bytes.
    loop {
        match stream.read_exact(&mut buf) {
            Ok(()) if buf[0] == b'$' => break,
            Ok(()) => continue,
            Err(_) => return None,
        }
    }
    let mut data = Vec::new();
    loop {
        match stream.read_exact(&mut buf) {
            Ok(()) if buf[0] == b'#' => break,
            Ok(()) => data.push(buf[0]),
            Err(_) => return None,
        }
    }
    // Read 2-char checksum (ignore).
    let _ = stream.read_exact(&mut buf);
    let _ = stream.read_exact(&mut buf);
    Some(String::from_utf8(data).unwrap())
}

/// Wait for machina process to exit, with timeout.
fn wait_process(child: &mut Child, timeout: Duration) {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                assert!(status.success(), "machina exited: {:?}", status,);
                return;
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    panic!("machina timed out");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("wait error: {}", e),
        }
    }
}

/// End-to-end GDB integration test against the production
/// machina binary. Spawns machina with -S -gdb, connects
/// via TCP, and drives the full RSP sequence over the real
/// CPU/system execution path.
///
/// Covers AC-1 (registers), AC-2 (memory + MMIO + negative),
/// AC-3 (breakpoints), AC-6 (snapshot after stop), AC-7
/// (stop replies).
#[test]
fn test_gdb_production_integration() {
    if !tcp_bind_available() {
        eprintln!("skipping: TCP bind not permitted");
        return;
    }
    ensure_machina_built();

    // Allocate a random port for GDB.
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let gdb_port = probe.local_addr().unwrap().port();
    drop(probe);

    let gdb_addr = format!("tcp:127.0.0.1:{}", gdb_port);
    let kernel = sifive_pass_bin();
    let kernel_str = kernel.to_str().unwrap();

    // Spawn machina: -S (freeze) -gdb tcp::PORT
    // -bios none -kernel sifive_pass.bin -nographic
    let mut child = Command::new(bin_path("machina"))
        .args([
            "-M",
            "riscv64-ref",
            "-m",
            "128",
            "-bios",
            "none",
            "-kernel",
            kernel_str,
            "-nographic",
            "-S",
            "-gdb",
            &gdb_addr,
        ])
        .current_dir(project_root())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("machina failed to start");

    // Give machina time to start and bind the GDB port.
    // Retry connection with backoff.
    let mut stream = None;
    for attempt in 0..20 {
        std::thread::sleep(Duration::from_millis(200));
        match TcpStream::connect(format!("127.0.0.1:{}", gdb_port,)) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) => {
                // Check if process crashed.
                if let Ok(Some(status)) = child.try_wait() {
                    panic!("machina exited early: {:?}", status,);
                }
                if attempt == 19 {
                    // Last attempt, clean up.
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("GDB port not available after 4s");
                }
            }
        }
    }
    let mut stream = stream.unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();

    // ── 1. Initial stop reply (AC-7) ──
    let init = match rsp_recv_or_eof(&mut stream) {
        Some(s) => s,
        None => {
            // Connection closed or timed out.
            // Note: don't read stderr here, it blocks if
            // the child is still alive.
            let status = child.try_wait();
            panic!(
                "initial stop: no reply from machina\n\
                 process status: {:?}",
                status,
            );
        }
    };
    assert_eq!(init, "T05thread:01;", "initial stop: got {}", init,);

    // ── 2. No-ack mode ──
    rsp_send(&mut stream, "QStartNoAckMode");
    assert_eq!(rsp_recv(&mut stream), "OK");

    // ── 3. qXfer target XML (AC-4) ──
    rsp_send(&mut stream, "qXfer:features:read:target.xml:0,fff");
    let xml_resp = rsp_recv(&mut stream);
    assert!(
        xml_resp.starts_with('m') || xml_resp.starts_with('l'),
        "qXfer prefix: {}",
        xml_resp,
    );
    let xml = &xml_resp[1..];
    assert!(xml.contains("org.gnu.gdb.riscv.cpu"));
    assert!(xml.contains("org.gnu.gdb.riscv.fpu"));

    // ── 4. Read all registers g (AC-1) ──
    rsp_send(&mut stream, "g");
    let regs = rsp_recv(&mut stream);
    assert_eq!(regs.len(), 1040, "65 regs * 8 bytes * 2 hex");

    // ── 5. Read PC p20 (AC-1) ──
    // With -S, CPU freezes before executing. PC may be
    // at MROM reset vector (0x1000 area) or kernel entry
    // depending on startup timing. Just verify non-zero.
    rsp_send(&mut stream, "p20");
    let pc_hex = rsp_recv(&mut stream);
    let pc_val = u8hex_to_u64(&pc_hex);
    assert_ne!(pc_val, 0, "PC non-zero after initial stop",);

    // ── 6. Write + read s1 (x9) (AC-1) ──
    // Use s1 (x9) instead of t0 (x5) because the
    // MROM reset vector uses t0 for address computation.
    // Writing t0 would corrupt the jump target.
    rsp_send(&mut stream, "P9=4242424242424242");
    assert_eq!(rsp_recv(&mut stream), "OK");
    rsp_send(&mut stream, "p9");
    assert_eq!(rsp_recv(&mut stream), "4242424242424242", "P9/p9 roundtrip",);

    // ── 7. RAM read (AC-2) ──
    // Read kernel code at 0x80000000 (first instruction).
    rsp_send(&mut stream, "m80000000,4");
    let ram_read = rsp_recv(&mut stream);
    assert_ne!(ram_read, "", "RAM read non-empty",);

    // ── 8. RAM write + read (AC-2) ──
    // Write at 0x80001000 (above kernel, safe RAM).
    rsp_send(&mut stream, "M80001000,4:cafebabe");
    assert_eq!(rsp_recv(&mut stream), "OK");
    rsp_send(&mut stream, "m80001000,4");
    assert_eq!(rsp_recv(&mut stream), "cafebabe", "RAM write/read",);

    // ── 9. MMIO read via AddressSpace (AC-2) ──
    // UART at 0x10000000 in riscv64-ref.
    rsp_send(&mut stream, "m10000000,4");
    let mmio_read = rsp_recv(&mut stream);
    // Should return something (no crash, no empty).
    assert!(!mmio_read.is_empty(), "MMIO read: got empty",);

    // ── 10. Negative: unmapped address (AC-2) ──
    // Address 0 is not in RAM range. Goes through
    // AddressSpace fallback; should return zeros.
    rsp_send(&mut stream, "m0,4");
    let neg_read = rsp_recv(&mut stream);
    assert_eq!(neg_read, "00000000", "unmapped read: got {}", neg_read,);

    // ── 11. Negative: out-of-range write (AC-2) ──
    // On production path with full AddressSpace,
    // writes to unmapped addresses may silently
    // succeed. Accept OK or E01.
    rsp_send(&mut stream, "Mffffffff,4:deadbeef");
    let neg_write = rsp_recv(&mut stream);
    assert!(
        neg_write == "OK" || neg_write == "E01",
        "out-of-range write: got {}",
        neg_write,
    );

    // ── 12. Breakpoint at kernel + continue (AC-3, AC-7) ──
    // Set breakpoint at kernel entry (0x80000000) BEFORE
    // any step/continue so the CPU is still at MROM and
    // must hit the breakpoint when it reaches the kernel
    // via the reset vector.  With -bios none, raw binaries
    // load at RAM_BASE (0x80000000). Breakpoints are only
    // checked at TB boundaries (start of each TB).
    rsp_send(&mut stream, "Z0,80000000,4");
    let z0_reply = rsp_recv(&mut stream);
    assert_eq!(z0_reply, "OK");
    rsp_send(&mut stream, "c");
    let bp_reply = match rsp_recv_or_eof(&mut stream) {
        Some(s) => s,
        None => {
            let status = child.try_wait();
            panic!(
                "bp continue: no reply from machina\n\
                 process status: {:?}",
                status,
            );
        }
    };
    assert!(bp_reply.contains("swbreak"), "bp hit: {}", bp_reply,);

    // ── 13. g after breakpoint stop (AC-1, AC-6) ──
    // Verify register snapshot reflects post-execution state.
    rsp_send(&mut stream, "g");
    let bp_regs = rsp_recv(&mut stream);
    assert_eq!(bp_regs.len(), 1040);
    // Extract PC from g response (reg 32, offset 32*16).
    let pc_off = 32 * 8 * 2;
    let bp_pc = u8hex_to_u64(&bp_regs[pc_off..pc_off + 16]);
    assert_eq!(bp_pc, 0x8000_0000, "PC at breakpoint: got {:#x}", bp_pc,);

    // ── 14. p20 after execution (AC-6) ──
    rsp_send(&mut stream, "p20");
    let bp_pc_hex = rsp_recv(&mut stream);
    let bp_pc_val = u8hex_to_u64(&bp_pc_hex);
    assert_eq!(
        bp_pc_val, 0x8000_0000,
        "p20 after bp hit: got {:#x}",
        bp_pc_val,
    );

    // ── 15. Step reply (AC-7) ──
    // Step from the kernel entry. The CPU is at
    // 0x80000000, stepping advances to 0x80000004.
    rsp_send(&mut stream, "s");
    let step_reply = rsp_recv(&mut stream);
    assert!(
        step_reply.starts_with("T05thread:01;"),
        "step stop reply: got {}",
        step_reply,
    );
    // Verify PC advanced after step.
    rsp_send(&mut stream, "p20");
    let step_pc_hex = rsp_recv(&mut stream);
    let step_pc = u8hex_to_u64(&step_pc_hex);
    assert_ne!(
        step_pc, bp_pc_val,
        "PC changed after step: before {:#x}, after {:#x}",
        bp_pc_val, step_pc,
    );

    // ── 16. Remove breakpoint (AC-3 negative) ──
    rsp_send(&mut stream, "z0,80000000,4");
    assert_eq!(rsp_recv(&mut stream), "OK");

    // ── 17. Detach -> process exits cleanly ──
    rsp_send(&mut stream, "D");
    let _ = rsp_recv_or_eof(&mut stream);

    // Cleanup.
    stream.shutdown(std::net::Shutdown::Both).ok();
    wait_process(&mut child, Duration::from_secs(5));
}

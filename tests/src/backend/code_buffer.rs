use machina_backend::code_buffer::CodeBuffer;

#[test]
fn test_emit_and_read() {
    let mut buf = CodeBuffer::new(4096).unwrap();
    buf.emit_u8(0x90); // NOP
    buf.emit_u32(0xDEADBEEF);
    assert_eq!(buf.offset(), 5);
    assert_eq!(buf.as_slice()[0], 0x90);
    assert_eq!(buf.read_u32(1), 0xDEADBEEF);
}

#[test]
fn test_patch() {
    let mut buf = CodeBuffer::new(4096).unwrap();
    buf.emit_u32(0);
    buf.patch_u32(0, 0x12345678);
    assert_eq!(buf.read_u32(0), 0x12345678);
}

#[test]
fn test_permissions() {
    let buf = CodeBuffer::new(4096).unwrap();
    buf.set_executable().unwrap();
    buf.set_writable().unwrap();
}

use machina_hw_core::chardev::{Chardev, NullChardev};

#[test]
fn test_null_chardev_write_discard() {
    let mut c = NullChardev;
    // Writing to null must not panic.
    c.write(0x41);
    c.write(0xff);
}

#[test]
fn test_null_chardev_read_none() {
    let mut c = NullChardev;
    assert_eq!(c.read(), None);
}

#[test]
fn test_null_chardev_can_read_false() {
    let c = NullChardev;
    assert!(!c.can_read());
}

use std::sync::{Arc, Mutex};

#[cfg(unix)]
use machina_hw_core::chardev::SocketChardev;
use machina_hw_core::chardev::{
    CharFrontend, Chardev, NullChardev, StdioChardev,
};

#[test]
fn test_null_chardev_write_discard() {
    let mut c = NullChardev;
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

// -- Helper: in-memory chardev for frontend tests --

struct MemChardev {
    tx_buf: Arc<Mutex<Vec<u8>>>,
}

impl MemChardev {
    fn new(tx_sink: Arc<Mutex<Vec<u8>>>) -> Self {
        Self { tx_buf: tx_sink }
    }
}

impl Chardev for MemChardev {
    fn read(&mut self) -> Option<u8> {
        None
    }

    fn write(&mut self, data: u8) {
        self.tx_buf.lock().unwrap().push(data);
    }

    fn can_read(&self) -> bool {
        false
    }
}

#[test]
fn test_char_frontend_write_through() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let backend = MemChardev::new(Arc::clone(&sink));
    let mut fe = CharFrontend::new(Box::new(backend));

    fe.write(b"hello");
    assert_eq!(*sink.lock().unwrap(), b"hello".to_vec());
}

#[test]
fn test_char_frontend_start_input() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let backend = MemChardev::new(Arc::clone(&sink));
    let mut fe = CharFrontend::new(Box::new(backend));

    let received = Arc::new(Mutex::new(Vec::new()));
    let recv_clone = Arc::clone(&received);
    let cb: Arc<Mutex<dyn FnMut(u8) + Send>> =
        Arc::new(Mutex::new(move |byte: u8| {
            recv_clone.lock().unwrap().push(byte);
        }));
    // start_input on MemChardev is a no-op (default
    // impl), so callback won't fire -- just verify no
    // panic.
    fe.start_input(cb);
}

#[test]
fn test_stdio_chardev_write() {
    let mut c = StdioChardev::new();
    c.write(b'X');
}

#[cfg(unix)]
#[test]
fn test_socket_chardev_not_connected() {
    let mut c = SocketChardev::new();
    assert_eq!(c.read(), None);
}

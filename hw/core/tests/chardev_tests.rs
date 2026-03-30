use std::sync::{Arc, Mutex};

use machina_hw_core::chardev::{
    CharFrontend, Chardev, NullChardev, SocketChardev, StdioChardev,
};

// -- Existing NullChardev tests -----------------------------------

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

// -- Helper: in-memory chardev for frontend tests -----------------

struct MemChardev {
    rx_buf: Vec<u8>,
    tx_buf: Arc<Mutex<Vec<u8>>>,
}

impl MemChardev {
    fn new(rx_data: &[u8], tx_sink: Arc<Mutex<Vec<u8>>>) -> Self {
        Self {
            rx_buf: rx_data.to_vec(),
            tx_buf: tx_sink,
        }
    }
}

impl Chardev for MemChardev {
    fn read(&mut self) -> Option<u8> {
        if self.rx_buf.is_empty() {
            None
        } else {
            Some(self.rx_buf.remove(0))
        }
    }

    fn write(&mut self, data: u8) {
        self.tx_buf.lock().unwrap().push(data);
    }

    fn can_read(&self) -> bool {
        !self.rx_buf.is_empty()
    }

    fn set_handler(&mut self, _handler: Option<Box<dyn FnMut(u8) + Send>>) {}
}

// -- CharFrontend tests -------------------------------------------

#[test]
fn test_char_frontend_write_through() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let backend = MemChardev::new(&[], Arc::clone(&sink));
    let mut fe = CharFrontend::new(Box::new(backend));

    fe.write(b"hello");
    assert_eq!(*sink.lock().unwrap(), b"hello".to_vec());
}

#[test]
fn test_char_frontend_receive_callback() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let backend = MemChardev::new(b"AB", Arc::clone(&sink));

    let received = Arc::new(Mutex::new(Vec::new()));
    let recv_clone = Arc::clone(&received);

    let mut fe = CharFrontend::new(Box::new(backend));
    fe.set_handlers(
        Box::new(move |data: &[u8]| {
            recv_clone.lock().unwrap().extend_from_slice(data);
        }),
        Box::new(|_event| {}),
    );

    fe.poll();
    assert_eq!(*received.lock().unwrap(), b"AB".to_vec());
}

// -- StdioChardev tests -------------------------------------------

#[test]
fn test_stdio_chardev_write() {
    let mut c = StdioChardev::new();
    // Must not panic — output goes to real stdout.
    c.write(b'X');
}

// -- SocketChardev tests ------------------------------------------

#[test]
fn test_socket_chardev_not_connected() {
    let mut c = SocketChardev::new();
    // No connection → read returns None.
    assert_eq!(c.read(), None);
}

#[test]
fn test_char_frontend_poll_receives() {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let backend = MemChardev::new(b"XYZ", Arc::clone(&sink));

    let received = Arc::new(Mutex::new(Vec::new()));
    let recv_clone = Arc::clone(&received);

    let mut fe = CharFrontend::new(Box::new(backend));
    fe.set_handlers(
        Box::new(move |data: &[u8]| {
            recv_clone.lock().unwrap().extend_from_slice(data);
        }),
        Box::new(|_event| {}),
    );

    // Poll with data available: handler should be called.
    fe.poll();
    let got = received.lock().unwrap().clone();
    assert_eq!(
        got,
        b"XYZ".to_vec(),
        "poll should forward all available bytes"
    );

    // Second poll: no more data.
    fe.poll();
    let got2 = received.lock().unwrap().clone();
    assert_eq!(got2, b"XYZ".to_vec(), "second poll should add nothing");
}

// Character device backend framework.

use std::sync::Mutex;

type CharHandler = Mutex<Option<Box<dyn FnMut(u8) + Send>>>;

/// Trait for character device backends.
///
/// A chardev provides byte-level I/O used by serial ports,
/// consoles, and similar devices.
pub trait Chardev: Send + Sync {
    /// Read one byte if available.
    fn read(&mut self) -> Option<u8>;

    /// Write one byte to the backend.
    fn write(&mut self, data: u8);

    /// Returns `true` if data is available to read.
    fn can_read(&self) -> bool;

    /// Install (or clear) an input handler invoked when the
    /// backend receives data from the outside world.
    fn set_handler(&mut self, handler: Option<Box<dyn FnMut(u8) + Send>>);
}

// -- NullChardev ---------------------------------------------------

/// Discards all output and never produces input.
pub struct NullChardev;

impl Chardev for NullChardev {
    fn read(&mut self) -> Option<u8> {
        None
    }

    fn write(&mut self, _data: u8) {
        // Discard silently.
    }

    fn can_read(&self) -> bool {
        false
    }

    fn set_handler(&mut self, _handler: Option<Box<dyn FnMut(u8) + Send>>) {
        // Nothing to do — null backend never delivers input.
    }
}

// -- StdioChardev --------------------------------------------------

/// Wraps host stdin/stdout.
pub struct StdioChardev {
    handler: CharHandler,
}

impl StdioChardev {
    pub fn new() -> Self {
        Self {
            handler: Mutex::new(None),
        }
    }
}

impl Default for StdioChardev {
    fn default() -> Self {
        Self::new()
    }
}

impl Chardev for StdioChardev {
    fn read(&mut self) -> Option<u8> {
        // TODO: non-blocking read from stdin
        None
    }

    fn write(&mut self, _data: u8) {
        // TODO: write byte to stdout
    }

    fn can_read(&self) -> bool {
        // TODO: poll stdin
        false
    }

    fn set_handler(&mut self, handler: Option<Box<dyn FnMut(u8) + Send>>) {
        *self.handler.lock().unwrap() = handler;
    }
}

// -- SocketChardev -------------------------------------------------

/// Unix-socket backed chardev (for integration testing).
pub struct SocketChardev {
    handler: CharHandler,
}

impl SocketChardev {
    pub fn new() -> Self {
        Self {
            handler: Mutex::new(None),
        }
    }
}

impl Default for SocketChardev {
    fn default() -> Self {
        Self::new()
    }
}

impl Chardev for SocketChardev {
    fn read(&mut self) -> Option<u8> {
        // TODO: read from connected socket
        None
    }

    fn write(&mut self, _data: u8) {
        // TODO: write to connected socket
    }

    fn can_read(&self) -> bool {
        // TODO: poll socket
        false
    }

    fn set_handler(&mut self, handler: Option<Box<dyn FnMut(u8) + Send>>) {
        *self.handler.lock().unwrap() = handler;
    }
}

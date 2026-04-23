// Character device backend framework.

use std::io::Write as _;
use std::sync::{Arc, Mutex};

use machina_core::mobject::{MObject, MObjectState};

/// Shared mutable byte callback.
pub type ByteCb = Arc<Mutex<dyn FnMut(u8) + Send>>;

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

    /// Start delivering input bytes via the callback.
    /// The backend is responsible for how (thread, poll,
    /// etc.). The callback is invoked with each byte.
    fn start_input(&mut self, _cb: ByteCb) {}
}

/// Bridges a device (frontend) to a chardev backend.
pub struct CharFrontend {
    backend: Box<dyn Chardev>,
}

impl CharFrontend {
    pub fn new(backend: Box<dyn Chardev>) -> Self {
        Self { backend }
    }

    /// Write a byte slice to the backend.
    pub fn write(&mut self, data: &[u8]) {
        for &b in data {
            self.backend.write(b);
        }
    }

    /// Start receiving input from the backend. The
    /// callback is invoked for each byte received.
    pub fn start_input(&mut self, cb: ByteCb) {
        self.backend.start_input(cb);
    }
}

#[derive(Debug)]
pub enum ChardevResolveError {
    UnknownPath(String),
    BackendUnavailable(String),
    BackendAlreadyInstalled(String),
}

impl std::fmt::Display for ChardevResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownPath(path) => {
                write!(f, "unknown chardev object '{path}'")
            }
            Self::BackendUnavailable(path) => {
                write!(f, "chardev object '{path}' has no available frontend")
            }
            Self::BackendAlreadyInstalled(path) => {
                write!(
                    f,
                    "chardev object '{path}' already has an installed frontend"
                )
            }
        }
    }
}

impl std::error::Error for ChardevResolveError {}

pub trait ChardevResolver: Send + Sync {
    fn take_frontend(
        &self,
        path: &str,
    ) -> Result<CharFrontend, ChardevResolveError>;

    fn put_frontend(
        &self,
        path: &str,
        frontend: CharFrontend,
    ) -> Result<(), ChardevResolveError>;
}

pub struct ChardevObject {
    object: MObjectState,
    frontend: Option<CharFrontend>,
}

impl ChardevObject {
    pub fn new_detached(local_id: &str, frontend: CharFrontend) -> Self {
        Self {
            object: MObjectState::new_detached(local_id)
                .expect("chardev local_id must be valid"),
            frontend: Some(frontend),
        }
    }

    pub fn frontend_available(&self) -> bool {
        self.frontend.is_some()
    }

    pub fn take_frontend(&mut self) -> Option<CharFrontend> {
        self.frontend.take()
    }

    pub fn put_frontend(
        &mut self,
        frontend: CharFrontend,
    ) -> Result<(), ChardevResolveError> {
        let Some(path) = self.object.object_path() else {
            return Err(ChardevResolveError::UnknownPath(
                self.object.local_id().to_string(),
            ));
        };
        if self.frontend.is_some() {
            return Err(ChardevResolveError::BackendAlreadyInstalled(
                path.to_string(),
            ));
        }
        self.frontend = Some(frontend);
        Ok(())
    }
}

impl MObject for ChardevObject {
    fn mobject_state(&self) -> &MObjectState {
        &self.object
    }

    fn mobject_state_mut(&mut self) -> &mut MObjectState {
        &mut self.object
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// Discards all output and never produces input.
pub struct NullChardev;

impl Chardev for NullChardev {
    fn read(&mut self) -> Option<u8> {
        None
    }

    fn write(&mut self, _data: u8) {}

    fn can_read(&self) -> bool {
        false
    }
}

/// Wraps host stdin/stdout with QEMU-compatible escape
/// sequences (Ctrl+A prefix):
///   Ctrl+A, X — exit emulator
///   Ctrl+A, C — toggle monitor console
///   Ctrl+A, H — show help
///   Ctrl+A, Ctrl+A — send literal Ctrl+A
pub struct StdioChardev {
    _thread: Option<std::thread::JoinHandle<()>>,
    /// Saved terminal state for raw-mode restore on drop.
    /// Unix only: Windows has no termios.
    #[cfg(unix)]
    saved_termios: Option<libc::termios>,
    /// Monitor line callback for Ctrl+A C toggle.
    monitor_cb: Option<ByteCb>,
    /// Quit callback for Ctrl+A X.
    quit_cb: Option<Arc<dyn Fn() + Send + Sync>>,
}

const ESCAPE_CHAR: u8 = 0x01; // Ctrl+A

impl StdioChardev {
    pub fn new() -> Self {
        #[cfg(unix)]
        let saved = enable_raw_mode();
        #[cfg(unix)]
        if saved.is_some() {
            eprintln!("machina: Ctrl+A H for help");
        }
        #[cfg(not(unix))]
        eprintln!("machina: Ctrl+A H for help");
        Self {
            _thread: None,
            #[cfg(unix)]
            saved_termios: saved,
            monitor_cb: None,
            quit_cb: None,
        }
    }

    /// Set a callback invoked when Ctrl+A X is pressed
    /// instead of calling process::exit().
    pub fn set_quit_cb(&mut self, cb: Arc<dyn Fn() + Send + Sync>) {
        self.quit_cb = Some(cb);
    }

    /// Set a monitor line callback for Ctrl+A C toggle.
    pub fn set_monitor_cb(&mut self, cb: ByteCb) {
        self.monitor_cb = Some(cb);
    }
}

impl Default for StdioChardev {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for StdioChardev {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(ref t) = self.saved_termios {
            restore_termios(t);
        }
    }
}

impl Chardev for StdioChardev {
    fn read(&mut self) -> Option<u8> {
        None
    }

    fn write(&mut self, data: u8) {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(&[data]);
        let _ = out.flush();
    }

    fn can_read(&self) -> bool {
        false
    }

    fn start_input(&mut self, cb: ByteCb) {
        let quit_cb = self.quit_cb.clone();
        let mon_cb = self.monitor_cb.clone();
        let handle = std::thread::spawn(move || {
            use std::io::Read;
            let stdin = std::io::stdin();
            let mut buf = [0u8; 1];
            let mut escape = false;
            let mut in_monitor = false;
            loop {
                match stdin.lock().read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => {
                        let ch = buf[0];
                        if escape {
                            escape = false;
                            match ch {
                                b'x' | b'X' => {
                                    if let Some(ref q) = quit_cb {
                                        q();
                                    } else {
                                        std::process::exit(0);
                                    }
                                    break;
                                }
                                b'c' | b'C' => {
                                    in_monitor = !in_monitor;
                                    if in_monitor {
                                        eprint!(
                                            "\r\n\
                                             (machina) "
                                        );
                                    } else {
                                        eprint!("\r\n");
                                    }
                                }
                                b'h' | b'H' => {
                                    eprintln!(
                                        "\nCtrl+A H  \
                                         help\n\
                                         Ctrl+A X  \
                                         exit\n\
                                         Ctrl+A C  \
                                         monitor\n\
                                         Ctrl+A \
                                         Ctrl+A  \
                                         send Ctrl+A"
                                    );
                                }
                                ESCAPE_CHAR => {
                                    send_to(
                                        &cb,
                                        &mon_cb,
                                        in_monitor,
                                        ESCAPE_CHAR,
                                    );
                                }
                                _ => {
                                    send_to(&cb, &mon_cb, in_monitor, ch);
                                }
                            }
                        } else if ch == ESCAPE_CHAR {
                            escape = true;
                        } else {
                            send_to(&cb, &mon_cb, in_monitor, ch);
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        self._thread = Some(handle);
    }
}

fn send_to(
    serial_cb: &ByteCb,
    monitor_cb: &Option<ByteCb>,
    in_monitor: bool,
    ch: u8,
) {
    if in_monitor {
        if let Some(ref m) = monitor_cb {
            if let Ok(mut f) = m.lock() {
                f(ch);
            }
        }
    } else {
        if let Ok(mut f) = serial_cb.lock() {
            f(ch);
        }
    }
}

/// Global saved termios for atexit restore.
#[cfg(unix)]
static SAVED_TERMIOS: std::sync::Mutex<Option<libc::termios>> =
    std::sync::Mutex::new(None);

/// Restore terminal from raw mode. Safe to call
/// multiple times or from signal handlers.
pub fn restore_terminal() {
    #[cfg(unix)]
    if let Ok(guard) = SAVED_TERMIOS.lock() {
        if let Some(ref t) = *guard {
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, t);
            }
        }
    }
}

#[cfg(unix)]
fn enable_raw_mode() -> Option<libc::termios> {
    unsafe {
        let mut orig: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(0, &mut orig) != 0 {
            return None;
        }
        // Save globally for atexit restore.
        if let Ok(mut g) = SAVED_TERMIOS.lock() {
            *g = Some(orig);
        }
        let mut raw = orig;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ISIG);
        raw.c_iflag &= !(libc::IXON | libc::ICRNL | libc::INLCR | libc::IGNCR);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(0, libc::TCSANOW, &raw) != 0 {
            return None;
        }
        Some(orig)
    }
}

#[cfg(unix)]
fn restore_termios(orig: &libc::termios) {
    unsafe {
        libc::tcsetattr(0, libc::TCSANOW, orig);
    }
}

/// Unix-socket backed chardev (for integration testing).
#[cfg(unix)]
pub struct SocketChardev {
    stream: Option<std::os::unix::net::UnixStream>,
}

#[cfg(unix)]
impl SocketChardev {
    pub fn new() -> Self {
        Self { stream: None }
    }

    pub fn connect(&mut self, path: &str) -> std::io::Result<()> {
        let s = std::os::unix::net::UnixStream::connect(path)?;
        s.set_nonblocking(true)?;
        self.stream = Some(s);
        Ok(())
    }
}

#[cfg(unix)]
impl Default for SocketChardev {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
impl Chardev for SocketChardev {
    fn read(&mut self) -> Option<u8> {
        use std::io::Read;
        let stream = self.stream.as_mut()?;
        let mut buf = [0u8; 1];
        match stream.read(&mut buf) {
            Ok(1) => Some(buf[0]),
            _ => None,
        }
    }

    fn write(&mut self, data: u8) {
        if let Some(ref mut stream) = self.stream {
            let _ = stream.write_all(&[data]);
        }
    }

    fn can_read(&self) -> bool {
        self.stream.is_some()
    }
}

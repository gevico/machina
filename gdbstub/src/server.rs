// GDB RSP TCP server.
//
// Listens for a single GDB client connection and
// processes the RSP command loop. The server assumes
// the target is initially paused.

use std::io;
use std::net::TcpListener;

use crate::handler::{GdbHandler, GdbTarget};
use crate::protocol;

/// GDB remote debug server.
pub struct GdbServer {
    addr: String,
}

impl GdbServer {
    pub fn new(addr: &str) -> Self {
        Self {
            addr: addr.to_string(),
        }
    }

    /// Listen, accept one client, and run the RSP loop.
    ///
    /// The target must be in a paused state. Commands like
    /// resume/step are blocking: they run the CPU and
    /// return only after the CPU stops again.
    pub fn serve(&self, target: &mut dyn GdbTarget) -> io::Result<()> {
        let listener = TcpListener::bind(&self.addr)?;
        eprintln!("machina: gdbstub waiting on {}", self.addr);

        let (mut stream, _) = listener.accept()?;
        eprintln!("machina: gdb client connected");
        stream.set_nodelay(true)?;

        let mut handler = GdbHandler::new();

        loop {
            let packet = match protocol::recv_packet(&mut stream) {
                Ok(p) => p,
                Err(e) => {
                    if e.kind() == io::ErrorKind::UnexpectedEof {
                        eprintln!(
                            "machina: gdb \
                                 client disconnected"
                        );
                        break;
                    }
                    continue;
                }
            };

            let response = match handler.handle(&packet, target) {
                Some(resp) => resp,
                None => {
                    let _ = protocol::send_packet(&mut stream, "OK");
                    break;
                }
            };

            if let Err(e) = protocol::send_packet(&mut stream, &response) {
                eprintln!("machina: gdb send error: {}", e);
                break;
            }
        }

        Ok(())
    }
}

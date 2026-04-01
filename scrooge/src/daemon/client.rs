use crate::protocol::{Request, Response};
use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

pub struct Client {
    socket_path: String,
}

impl Client {
    pub fn new(socket_path: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
        }
    }

    /// Send a request and wait for a response.
    ///
    /// Uses a 30-second read timeout so hooks never block Claude's prompt
    /// pipeline if the daemon is busy or unresponsive. Increase this for
    /// operations that are known to be slow (e.g. first-run model download).
    pub fn send(&self, req: Request) -> Result<Response> {
        self.send_timeout(req, Duration::from_secs(30))
    }

    pub fn send_timeout(&self, req: Request, timeout: Duration) -> Result<Response> {
        let mut stream = UnixStream::connect(&self.socket_path)?;
        stream.set_read_timeout(Some(timeout))?;

        // Write request as a single newline-terminated JSON line.
        serde_json::to_writer(&mut stream, &req)?;
        stream.write_all(b"\n")?;
        stream.flush()?;

        // Read one newline-terminated response line.
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;

        let resp: Response = serde_json::from_str(line.trim())?;
        Ok(resp)
    }

    /// Returns true if the daemon is reachable. Does not send any data —
    /// a clean EOF from the server side is expected and handled gracefully.
    pub fn is_running(&self) -> bool {
        UnixStream::connect(&self.socket_path).is_ok()
    }
}

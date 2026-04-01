use crate::protocol::{Request, Response};
use anyhow::Result;
use std::os::unix::net::UnixStream;
use std::io::Write;

pub struct Client {
    socket_path: String,
}

impl Client {
    pub fn new(socket_path: &str) -> Self {
        Self {
            socket_path: socket_path.to_string(),
        }
    }

    pub fn send(&self, req: Request) -> Result<Response> {
        let mut stream = UnixStream::connect(&self.socket_path)?;
        
        serde_json::to_writer(&mut stream, &req)?;
        stream.write_all(b"\n")?;
        stream.flush()?;

        // Read response
        let resp: Response = serde_json::from_reader(&mut stream)?;
        Ok(resp)
    }
    
    pub fn is_running(&self) -> bool {
        UnixStream::connect(&self.socket_path).is_ok()
    }
}

use crate::models::slm::Slm;
use crate::protocol::{ExtractedFact, Request, Response};
use anyhow::Result;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

pub struct Server {
    slm: Arc<Mutex<Slm>>,
    socket_path: String,
}

impl Server {
    pub fn new(slm: Slm, socket_path: &str) -> Self {
        Self {
            slm: Arc::new(Mutex::new(slm)),
            socket_path: socket_path.to_string(),
        }
    }

    pub fn start(&self) -> Result<()> {
        if Path::new(&self.socket_path).exists() {
            std::fs::remove_file(&self.socket_path)?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        eprintln!("[scrooge] Daemon listening on {}", self.socket_path);

        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    let slm = Arc::clone(&self.slm);
                    thread::spawn(move || handle_connection(&mut stream, slm));
                }
                Err(e) => eprintln!("[scrooge] accept error: {}", e),
            }
        }
        Ok(())
    }
}

/// Handle a single connection: read one newline-terminated JSON request,
/// write one newline-terminated JSON response, then close.
fn handle_connection(stream: &mut std::os::unix::net::UnixStream, slm: Arc<Mutex<Slm>>) {
    // Read exactly one line — the request JSON.
    let mut line = String::new();
    match BufReader::new(&*stream).read_line(&mut line) {
        Ok(0) => return, // client disconnected without sending anything (e.g. is_running probe)
        Ok(_) => {}
        Err(e) => {
            eprintln!("[scrooge] read error: {}", e);
            return;
        }
    }

    let req: Request = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[scrooge] parse error: {} (input: {:?})", e, line.trim());
            return;
        }
    };

    let is_shutdown = matches!(req, Request::Shutdown);

    let resp = dispatch(req, &slm);

    // Write response as a single newline-terminated JSON line.
    if let Err(e) = (|| -> Result<()> {
        serde_json::to_writer(&mut *stream, &resp)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
        Ok(())
    })() {
        eprintln!("[scrooge] write error: {}", e);
    }

    if is_shutdown {
        eprintln!("[scrooge] Shutdown complete.");
        std::process::exit(0);
    }
}

fn dispatch(req: Request, slm: &Arc<Mutex<Slm>>) -> Response {
    match req {
        Request::Ping => Response::Pong,
        Request::Shutdown => Response::Pong, // response already sent before exit in caller

        Request::Librarian { prompt, max_tokens } => {
            eprintln!("[scrooge] Librarian: reconciling...");
            let mut model = slm.lock().unwrap();
            match model.generate(&prompt, max_tokens) {
                Ok(summary) => {
                    eprintln!("[scrooge] Librarian: done.");
                    Response::Librarian { summary }
                }
                Err(e) => {
                    eprintln!("[scrooge] Librarian error: {}", e);
                    Response::Error(e.to_string())
                }
            }
        }

        Request::Gatekeeper { transcript } => {
            eprintln!("[scrooge] Gatekeeper: extracting facts...");
            let mut model = slm.lock().unwrap();
            let prompt = format!(
                "Extract technical decisions, conventions, and fixes from this transcript.\n\
                 Output ONLY a JSON array. Each element: {{\"category\": \"decision|convention|fix\", \"content\": \"...\", \"priority\": 1-10}}.\n\
                 If nothing is worth storing, output []. No explanation, only JSON.\n\n\
                 Transcript:\n{}",
                transcript
            );
            match model.generate(&prompt, 512) {
                Ok(raw) => {
                    eprintln!("[scrooge] Gatekeeper: done.");
                    // Extract the JSON array from the response — the model may emit
                    // surrounding prose even when instructed not to.
                    let facts = parse_facts_from_output(&raw);
                    Response::Gatekeeper { facts }
                }
                Err(e) => {
                    eprintln!("[scrooge] Gatekeeper error: {}", e);
                    Response::Error(e.to_string())
                }
            }
        }
    }
}

/// Attempt to extract a JSON array from the model's raw output.
/// The model often emits surrounding prose despite instructions; this
/// finds the first `[...]` substring and tries to parse it.
fn parse_facts_from_output(raw: &str) -> Vec<ExtractedFact> {
    // Try the whole response first.
    if let Ok(facts) = serde_json::from_str::<Vec<ExtractedFact>>(raw.trim()) {
        return facts;
    }
    // Find first '[' and last ']' and try the slice between them.
    if let (Some(start), Some(end)) = (raw.find('['), raw.rfind(']')) {
        if start < end {
            if let Ok(facts) = serde_json::from_str::<Vec<ExtractedFact>>(&raw[start..=end]) {
                return facts;
            }
        }
    }
    eprintln!("[scrooge] Gatekeeper: could not parse JSON from output: {:?}", &raw[..raw.len().min(200)]);
    vec![]
}

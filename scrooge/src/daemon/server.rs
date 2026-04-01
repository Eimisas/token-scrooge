use crate::models::slm::Slm;
use crate::protocol::{Request, Response};
use anyhow::Result;
use std::io::Write;
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
        println!("[scrooge] Daemon listening on {}", self.socket_path);

        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    let slm = Arc::clone(&self.slm);
                    thread::spawn(move || {
                        if let Err(e) = handle_client(&mut stream, slm) {
                            eprintln!("[scrooge] client error: {}", e);
                        }
                    });
                }
                Err(e) => eprintln!("[scrooge] accept error: {}", e),
            }
        }
        Ok(())
    }
}

fn handle_client(stream: &mut std::os::unix::net::UnixStream, slm: Arc<Mutex<Slm>>) -> Result<()> {
    loop {
        let req: Request = match serde_json::from_reader(stream.try_clone()?) {
            Ok(r) => r,
            Err(_) => break, // Connection closed or malformed
        };

        let resp = match req {
            Request::Ping => Response::Pong,
            Request::Shutdown => {
                let _ = serde_json::to_writer(&mut *stream, &Response::Pong);
                std::process::exit(0);
            }
            Request::Librarian { prompt, max_tokens } => {
                let mut model = slm.lock().unwrap();
                let system_prompt = "You are a project memory librarian. Reconcile the following potentially conflicting facts into a single, current truth based on their timestamps. If they don't conflict, simply summarize them briefly. Be concise.";
                let combined_prompt = format!("{}\n\nTask: Reconcile these facts for the query: {}\n\nFacts:\n{}", system_prompt, prompt, prompt);
                match model.generate(&combined_prompt, max_tokens) {
                    Ok(summary) => Response::Librarian { summary },
                    Err(e) => Response::Error(e.to_string()),
                }
            }
            Request::Gatekeeper { transcript } => {
                let mut model = slm.lock().unwrap();
                let system_prompt = "You are a project historian. Extract key technical decisions, conventions, and fixes from the following transcript. Output ONLY valid JSON as an array of objects with keys: category (decision|convention|fix), content (string), priority (1-10).";
                let combined_prompt = format!("{}\n\nTranscript:\n{}", system_prompt, transcript);
                match model.generate(&combined_prompt, 500) {
                    Ok(json_raw) => {
                        // Parse JSON from model output
                        // For PoC, we return error if model didn't return perfect JSON
                        match serde_json::from_str(&json_raw) {
                            Ok(facts) => Response::Gatekeeper { facts },
                            Err(_) => Response::Error(format!("Model returned invalid JSON: {}", json_raw)),
                        }
                    }
                    Err(e) => Response::Error(e.to_string()),
                }
            }
        };

        serde_json::to_writer(&mut *stream, &resp)?;
        stream.write_all(b"\n")?;
        stream.flush()?;
    }
    
    Ok(())
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub enum Request {
    Librarian {
        prompt: String,
        max_tokens: usize,
    },
    Gatekeeper {
        transcript: String,
    },
    Ping,
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    Librarian {
        summary: String,
    },
    Gatekeeper {
        facts: Vec<ExtractedFact>,
    },
    Pong,
    Error(String),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExtractedFact {
    pub content: String,
    pub category: String,
    pub priority: u8,
}

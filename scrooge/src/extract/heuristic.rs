use super::transcript::TranscriptMessage;
use crate::db::facts::FactCategory;
use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone)]
pub struct ExtractedFact {
    pub content:  String,
    pub category: FactCategory,
    pub priority: u8,
}

// ─── Compiled patterns (initialised once) ────────────────────────────────────

static RE_USER_REMEMBER: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|\b)(?:remember|note|important)[:\s]+(.+)").unwrap()
});
static RE_DECISION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(?:we (?:decided|chose|opted|agreed)|(?:going|decided) to use|let's use|use\s+\w+\s+(?:instead|for))[:\s]+(.{10,200})",
    ).unwrap()
});
static RE_CONVENTION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:from now on|always|never|convention|pattern|rule)[:\s]+(.{10,150})").unwrap()
});
static RE_ASSISTANT_FIXED: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(?:I(?:'ve)?\s+fixed?|I(?:'ve)?\s+resolved?|The (?:bug|issue|error) (?:was|is) (?:fixed?|resolved?))[:\s]*(.{10,200})",
    ).unwrap()
});
static RE_CREATED_COMPONENT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(?:created?|added?|implemented?|built)\s+(?:a\s+)?(?:new\s+)?(.{5,80}?)\s+(?:component|function|module|service|hook|endpoint|class)",
    ).unwrap()
});
static RE_FILE_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:^|[\s(])([a-zA-Z0-9_.\-/]+\.[a-z]{1,5})(?::(\d+))?").unwrap()
});

const MAX_FACTS_PER_SESSION: usize = 20;
const MIN_CONTENT_LEN: usize = 15;

// ─── Public entry point ───────────────────────────────────────────────────────

pub fn extract(messages: &[TranscriptMessage]) -> Vec<ExtractedFact> {
    let mut facts: Vec<ExtractedFact> = Vec::new();

    for msg in messages {
        match msg {
            TranscriptMessage::User      { content }     => facts.extend(from_user(content)),
            TranscriptMessage::Assistant { content, .. } => facts.extend(from_assistant(content)),
            TranscriptMessage::Summary   { summary }     => facts.extend(from_summary(summary)),
            TranscriptMessage::FileWrite { path }        => facts.extend(file_op(path, "Created")),
            TranscriptMessage::FileEdit  { path }        => facts.extend(file_op(path, "Modified")),
            TranscriptMessage::ToolResult { .. }         => {} // too noisy
        }
    }

    // Highest priority first, dedup near-duplicates, then cap
    facts.sort_by(|a, b| b.priority.cmp(&a.priority));
    facts.dedup_by(|a, b| jaccard(&a.content, &b.content) > 0.8);
    facts.truncate(MAX_FACTS_PER_SESSION);
    facts
}

// ─── Per-message extractors ───────────────────────────────────────────────────

fn from_user(content: &str) -> Vec<ExtractedFact> {
    let mut out = Vec::new();

    for cap in RE_USER_REMEMBER.captures_iter(content) {
        let text = cap[1].trim().to_string();
        if text.len() >= MIN_CONTENT_LEN {
            out.push(ExtractedFact { content: text, category: FactCategory::User, priority: 10 });
        }
    }
    for cap in RE_CONVENTION.captures_iter(content) {
        let text = cap[1].trim().to_string();
        if text.len() >= MIN_CONTENT_LEN {
            out.push(ExtractedFact { content: text, category: FactCategory::Convention, priority: 8 });
        }
    }
    for cap in RE_DECISION.captures_iter(content) {
        let text = cap[1].trim().to_string();
        if text.len() >= MIN_CONTENT_LEN {
            out.push(ExtractedFact {
                content: format!("Decision: {}", text),
                category: FactCategory::Decision,
                priority: 7,
            });
        }
    }
    out
}

fn from_assistant(content: &str) -> Vec<ExtractedFact> {
    let mut out = Vec::new();

    for cap in RE_ASSISTANT_FIXED.captures_iter(content) {
        let text = cap[1].trim().to_string();
        if text.len() >= MIN_CONTENT_LEN {
            let enriched = enrich_with_file_ref(&text, content);
            out.push(ExtractedFact {
                content: format!("Fixed: {}", enriched),
                category: FactCategory::Fix,
                priority: 9,
            });
        }
    }
    for cap in RE_CREATED_COMPONENT.captures_iter(content) {
        let text = cap[1].trim().to_string();
        if text.len() >= MIN_CONTENT_LEN {
            out.push(ExtractedFact {
                content: format!("Created: {} component/module", text),
                category: FactCategory::File,
                priority: 5,
            });
        }
    }
    for cap in RE_DECISION.captures_iter(content) {
        let text = cap[1].trim().to_string();
        if text.len() >= MIN_CONTENT_LEN {
            out.push(ExtractedFact {
                content: format!("Decision: {}", text),
                category: FactCategory::Decision,
                priority: 7,
            });
        }
    }
    out
}

fn from_summary(summary: &str) -> Vec<ExtractedFact> {
    summary
        .lines()
        .map(|l| l.trim().trim_start_matches(['•', '-', '*', '·']).trim())
        .filter(|l| l.len() >= MIN_CONTENT_LEN && l.len() <= 300)
        .map(|l| {
            let lc = l.to_lowercase();
            let category = if lc.contains("fixed") || lc.contains("resolved") {
                FactCategory::Fix
            } else if lc.contains("decision") || lc.contains("chose") {
                FactCategory::Decision
            } else if lc.contains("convention") || lc.contains("pattern") {
                FactCategory::Convention
            } else {
                FactCategory::Context
            };
            ExtractedFact { content: l.to_string(), category, priority: 6 }
        })
        .collect()
}

fn file_op(path: &str, action: &str) -> Vec<ExtractedFact> {
    if path.contains("node_modules")
        || path.contains(".git/")
        || path.starts_with("/tmp")
        || path.ends_with(".tmp")
    {
        return vec![];
    }
    vec![ExtractedFact {
        content:  format!("{}: {}", action, path),
        category: FactCategory::File,
        priority: 2,
    }]
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

fn jaccard(a: &str, b: &str) -> f64 {
    let wa: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let wb: std::collections::HashSet<&str> = b.split_whitespace().collect();
    if wa.is_empty() && wb.is_empty() { return 1.0; }
    let inter = wa.intersection(&wb).count();
    let union = wa.union(&wb).count();
    if union == 0 { 0.0 } else { inter as f64 / union as f64 }
}

fn enrich_with_file_ref(text: &str, surrounding: &str) -> String {
    for cap in RE_FILE_PATH.captures_iter(surrounding) {
        let file = &cap[1];
        if file.contains('/')
            || matches!(
                file.rsplit('.').next().unwrap_or(""),
                "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "go" | "rb" | "java"
            )
        {
            return if let Some(line) = cap.get(2) {
                format!("{} ({}:{})", text, file, line.as_str())
            } else {
                format!("{} ({})", text, file)
            };
        }
    }
    text.to_string()
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_remember() {
        let msgs = vec![TranscriptMessage::User {
            content: "Remember: we use JWT in httpOnly cookies, not localStorage".to_string(),
        }];
        let facts = extract(&msgs);
        assert!(!facts.is_empty());
        assert_eq!(facts[0].category, FactCategory::User);
        assert_eq!(facts[0].priority, 10);
    }

    #[test]
    fn extracts_fix_from_assistant() {
        let msgs = vec![TranscriptMessage::Assistant {
            content: "I've fixed the null pointer in auth/refresh.ts line 47 that broke token refresh".to_string(),
            thinking: None,
        }];
        let facts = extract(&msgs);
        assert!(facts.iter().any(|f| f.category == FactCategory::Fix));
    }

    #[test]
    fn caps_at_max() {
        let msgs: Vec<_> = (0..50)
            .map(|i| TranscriptMessage::User {
                content: format!("Remember fact {} which is important and must not be forgotten ever", i),
            })
            .collect();
        assert!(extract(&msgs).len() <= MAX_FACTS_PER_SESSION);
    }

    #[test]
    fn jaccard_similarity() {
        assert!(jaccard("login bug fix here", "login bug fix here") > 0.9);
        assert!(jaccard("login bug", "database migration error") < 0.2);
    }
}

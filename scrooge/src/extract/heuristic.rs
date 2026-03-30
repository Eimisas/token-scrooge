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
// Natural phrasings developers use when choosing a tool/approach
static RE_LETS_USE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:let'?s|let us)\s+(?:use|go with|stick with|keep using)\s+(.{5,150})").unwrap()
});
// "we're using / we use / we'll use X for Y"
static RE_WE_USE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:we(?:'re| are| will|'ll)\s+using|we\s+use|we(?:'ll| will)\s+use)\s+(.{5,150})").unwrap()
});
// "the approach/convention/pattern is X"
static RE_THE_APPROACH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)the\s+(?:approach|convention|pattern|rule|architecture|strategy)\s+(?:is|should be|will be)[:\s]+(.{10,150})",
    ).unwrap()
});
// "don't use X / never mutate Y / avoid Z"
static RE_PROHIBITION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:don'?t|do not|never|avoid)\s+(.{15,150})").unwrap()
});
static RE_CONVENTION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:from now on|always|convention|pattern|rule)[:\s]+(.{10,150})").unwrap()
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
    let mut last_user: Option<&str> = None;

    for msg in messages {
        match msg {
            TranscriptMessage::User      { content }     => {
                facts.extend(from_user(content));
                last_user = Some(content);
            }
            TranscriptMessage::Assistant { content, .. } => {
                facts.extend(from_assistant(content, last_user));
                // Assistant turn ends the pair; next assistant gets fresh context
                last_user = None;
            }
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
    for cap in RE_THE_APPROACH.captures_iter(content) {
        let text = cap[1].trim().to_string();
        if text.len() >= MIN_CONTENT_LEN {
            out.push(ExtractedFact { content: text, category: FactCategory::Convention, priority: 8 });
        }
    }
    for cap in RE_PROHIBITION.captures_iter(content) {
        let text = cap[1].trim().to_string();
        if text.len() >= MIN_CONTENT_LEN {
            out.push(ExtractedFact {
                content: format!("Convention: {}", text),
                category: FactCategory::Convention,
                priority: 8,
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
    for cap in RE_LETS_USE.captures_iter(content) {
        let text = cap[1].trim().to_string();
        if text.len() >= MIN_CONTENT_LEN {
            out.push(ExtractedFact {
                content: format!("Decision: use {}", text),
                category: FactCategory::Decision,
                priority: 7,
            });
        }
    }
    for cap in RE_WE_USE.captures_iter(content) {
        let text = cap[1].trim().to_string();
        if text.len() >= MIN_CONTENT_LEN {
            out.push(ExtractedFact {
                content: format!("Decision: we use {}", text),
                category: FactCategory::Decision,
                priority: 7,
            });
        }
    }
    out
}

fn from_assistant(content: &str, user_ctx: Option<&str>) -> Vec<ExtractedFact> {
    let mut out = Vec::new();

    for cap in RE_ASSISTANT_FIXED.captures_iter(content) {
        let text = cap[1].trim().to_string();
        if text.len() >= MIN_CONTENT_LEN {
            let enriched = enrich_with_file_ref(&text, content);
            // If the captured fix is vague (no specific noun, just pronouns/short phrases),
            // prepend the user's question so the fact is searchable by what was being worked on.
            let fact_text = if is_vague(&enriched) {
                if let Some(ctx) = user_ctx {
                    format!("Fixed: {} (re: {})", enriched, truncate(ctx, 80))
                } else {
                    format!("Fixed: {}", enriched)
                }
            } else {
                format!("Fixed: {}", enriched)
            };
            out.push(ExtractedFact {
                content: fact_text,
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
    let mut out = Vec::new();
    for line in summary.lines() {
        let line = line.trim().trim_start_matches(['•', '-', '*', '·']).trim();
        if line.len() < MIN_CONTENT_LEN || line.len() > 300 {
            continue;
        }
        // Try full heuristic extraction on this bullet — captures decisions,
        // conventions, and fixes embedded in summary text with correct priority.
        let pattern_facts: Vec<_> = from_user(line)
            .into_iter()
            .chain(from_assistant(line, None))
            .collect();
        if !pattern_facts.is_empty() {
            out.extend(pattern_facts);
        } else {
            // Keyword fallback: at least categorise correctly even if no pattern fires
            let lc = line.to_lowercase();
            let category = if lc.contains("fixed") || lc.contains("resolved") {
                FactCategory::Fix
            } else if lc.contains("decision") || lc.contains("chose") {
                FactCategory::Decision
            } else if lc.contains("convention") || lc.contains("pattern") {
                FactCategory::Convention
            } else {
                FactCategory::Context
            };
            out.push(ExtractedFact { content: line.to_string(), category, priority: 6 });
        }
    }
    out
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

/// Returns true when a fix capture is too vague to be useful without context.
/// Triggers on short text or text that's only pronouns / generic nouns.
fn is_vague(text: &str) -> bool {
    const VAGUE_WORDS: &[&str] = &[
        "it", "this", "that", "the issue", "the bug", "the error",
        "the problem", "the crash", "the failure", "them", "these",
    ];
    let lower = text.to_lowercase();
    let trimmed = lower.trim();
    if text.len() < 25 { return true; }
    VAGUE_WORDS.iter().any(|w| trimmed == *w || trimmed.starts_with(&format!("{} ", w)))
}

/// Truncate to `max_chars` at a word boundary and append "…" if cut.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars { return s.to_string(); }
    let cut = s[..max_chars].rfind(char::is_whitespace).unwrap_or(max_chars);
    format!("{}…", s[..cut].trim_end())
}

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

    #[test]
    fn extracts_lets_use_pattern() {
        let msgs = vec![TranscriptMessage::User {
            content: "let's use Zod for all schema validation going forward".to_string(),
        }];
        let facts = extract(&msgs);
        assert!(facts.iter().any(|f| f.category == FactCategory::Decision && f.content.contains("Zod")),
            "expected Decision fact containing 'Zod', got: {:?}", facts);
    }

    #[test]
    fn extracts_we_use_pattern() {
        let msgs = vec![TranscriptMessage::User {
            content: "we're using the repository pattern for all data access".to_string(),
        }];
        let facts = extract(&msgs);
        assert!(facts.iter().any(|f| f.category == FactCategory::Decision && f.content.contains("repository")),
            "expected Decision fact containing 'repository', got: {:?}", facts);
    }

    #[test]
    fn extracts_the_approach_pattern() {
        let msgs = vec![TranscriptMessage::User {
            content: "the convention is to return Result types from all service functions".to_string(),
        }];
        let facts = extract(&msgs);
        assert!(facts.iter().any(|f| f.category == FactCategory::Convention && f.content.contains("Result")),
            "expected Convention fact containing 'Result', got: {:?}", facts);
    }

    #[test]
    fn extracts_prohibition_pattern() {
        let msgs = vec![TranscriptMessage::User {
            content: "don't store tokens in localStorage, use httpOnly cookies instead".to_string(),
        }];
        let facts = extract(&msgs);
        assert!(facts.iter().any(|f| f.category == FactCategory::Convention && f.content.contains("localStorage")),
            "expected Convention fact containing 'localStorage', got: {:?}", facts);
    }

    #[test]
    fn summary_bullet_with_pattern_gets_correct_category() {
        // A summary bullet that contains a decision phrase should be captured as Decision,
        // not fall through to the Context/6 default
        let msgs = vec![TranscriptMessage::Summary {
            summary: "• let's use postgres as the primary database for persistence".to_string(),
        }];
        let facts = extract(&msgs);
        assert!(facts.iter().any(|f| f.category == FactCategory::Decision),
            "expected Decision fact from summary, got: {:?}", facts);
    }

    #[test]
    fn vague_fix_gets_user_context_prepended() {
        let msgs = vec![
            TranscriptMessage::User {
                content: "the login redirect is broken after the last deploy".to_string(),
            },
            TranscriptMessage::Assistant {
                content: "I've fixed it now, should be working".to_string(),
                thinking: None,
            },
        ];
        let facts = extract(&msgs);
        let fix = facts.iter().find(|f| f.category == FactCategory::Fix).expect("expected a fix fact");
        assert!(fix.content.contains("login redirect"), "vague fix should include user context, got: {}", fix.content);
    }

    #[test]
    fn specific_fix_does_not_get_context_appended() {
        let msgs = vec![
            TranscriptMessage::User {
                content: "something is wrong".to_string(),
            },
            TranscriptMessage::Assistant {
                content: "I've fixed the null pointer dereference in auth/session.rs line 42 causing the crash".to_string(),
                thinking: None,
            },
        ];
        let facts = extract(&msgs);
        let fix = facts.iter().find(|f| f.category == FactCategory::Fix).expect("expected a fix fact");
        assert!(!fix.content.contains("re:"), "specific fix should not include context, got: {}", fix.content);
    }

    #[test]
    fn is_vague_detects_short_and_pronoun_captures() {
        assert!(is_vague("it"));
        assert!(is_vague("the bug"));
        assert!(is_vague("the issue"));
        assert!(is_vague("short"));
    }

    #[test]
    fn is_vague_passes_specific_captures() {
        assert!(!is_vague("the null pointer dereference in auth/session.rs at line 42"));
        assert!(!is_vague("the race condition in the connection pool causing deadlocks"));
    }

    #[test]
    fn truncate_cuts_at_word_boundary() {
        let s = "the quick brown fox jumps over the lazy dog";
        let t = truncate(s, 20);
        assert!(t.ends_with('…'), "should end with ellipsis, got: {}", t);
        assert!(!t.contains("jumps"), "should cut before 'jumps', got: {}", t);
        assert!(t.contains("fox"), "should include 'fox', got: {}", t);
    }

    #[test]
    fn summary_bullet_without_pattern_falls_back_to_keyword() {
        let msgs = vec![TranscriptMessage::Summary {
            summary: "• the team fixed the authentication middleware issue".to_string(),
        }];
        let facts = extract(&msgs);
        // Should be categorised as Fix by keyword fallback
        assert!(facts.iter().any(|f| f.category == FactCategory::Fix),
            "expected Fix fact from keyword fallback, got: {:?}", facts);
    }
}

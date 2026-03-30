use crate::db::facts::{Fact, FactCategory};
use crate::db::stats::GainSummary;
use colored::Colorize;

/// Format facts as the additionalContext string sent to Claude.
pub fn memory_context(facts: &[&Fact]) -> String {
    if facts.is_empty() { return String::new(); }
    let n = facts.len();
    let mut lines = vec![format!(
        "[MEMORY: {} relevant fact{} from past sessions]",
        n,
        if n == 1 { "" } else { "s" }
    )];
    for f in facts {
        lines.push(format!(
            "• [{}] {} {}",
            category_label(&f.category),
            f.content.trim(),
            age_label(&f.created_at),
        ));
    }
    lines.join("\n")
}

pub fn print_gain_report(s: &GainSummary) {
    println!("{}", "Token Scrooge — Memory Stats".bold().cyan());
    println!("{}", "─".repeat(42).dimmed());
    println!("  Facts stored:     {}", s.total_facts_stored.to_string().bold());
    println!("  Sessions tracked: {}", s.total_sessions.to_string().bold());
    println!("  Injections:       {}", s.total_injections.to_string().bold());
    if s.total_tokens_saved > 0 {
        println!(
            "  Tokens saved:     {}",
            format!("~{}", s.total_tokens_saved).green().bold()
        );
    }
    println!("{}", "─".repeat(42).dimmed());
}

pub fn print_fact(fact: &Fact, index: usize) {
    println!(
        "{:>3}. [{}] {} {}",
        (index + 1).to_string().dimmed(),
        category_label(&fact.category).yellow(),
        fact.content.trim(),
        age_label(&fact.created_at).dimmed(),
    );
    println!("     {}", fact.id.dimmed());
}

fn category_label(cat: &FactCategory) -> &'static str {
    match cat {
        FactCategory::Decision   => "decision",
        FactCategory::Fix        => "fix",
        FactCategory::File       => "file",
        FactCategory::Convention => "convention",
        FactCategory::User       => "note",
        FactCategory::Context    => "context",
    }
}

fn age_label(dt: &chrono::DateTime<chrono::Utc>) -> String {
    let days = (chrono::Utc::now() - *dt).num_days();
    match days {
        0     => String::new(),
        1     => "(yesterday)".to_string(),
        2..=6 => format!("({} days ago)", days),
        7..=29 => format!("({} week{} ago)", days / 7, if days / 7 == 1 { "" } else { "s" }),
        _     => format!("({} month{} ago)", days / 30, if days / 30 == 1 { "" } else { "s" }),
    }
}

use crate::config::resolve_scrooge_dir;
use crate::db::{self, facts, stats};
use crate::format;
use crate::hooks::{self, HookInput};
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::io::Read;
use std::path::Path;

// ─── CLI definition ───────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "scrooge", version, about = "Zero-setup persistent memory for Claude Code")]
pub struct Cli {
    /// Show token savings and memory stats, then exit
    #[arg(long = "savings", short = 's')]
    pub savings: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Wrap `claude` with memory management — use this instead of `claude`
    Claude {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Internal: handle a Claude Code hook event (called by hooks, not users)
    #[command(hide = true)]
    Hook { event: String },
    /// Save a fact to memory
    Remember {
        text: String,
        /// Category: decision | fix | file | convention | context (default: note)
        #[arg(short, long)]
        tag: Option<String>,
    },
    /// Delete a fact by ID
    Forget { id: String },
    /// Search memory for relevant facts
    Recall {
        query: String,
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Show token savings analytics (also available as `scrooge --savings`)
    Gain,
    /// Install hooks and initialise the memory DB
    Setup,
}

// ─── Command handlers ─────────────────────────────────────────────────────────

pub fn cmd_claude(args: Vec<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let scrooge_dir = resolve_scrooge_dir(&cwd);

    if !crate::config::db_path(&scrooge_dir).exists() {
        eprintln!("[scrooge] First run — initialising memory store at {}", scrooge_dir.display());
        db::open(&scrooge_dir)?;
        crate::inject::inject_hooks()?;
        maybe_gitignore(&cwd)?;
        eprintln!("[scrooge] Ready. Hooks installed in ~/.claude/settings.json");
    } else {
        // Keep hook path up-to-date (handles reinstalls)
        crate::inject::inject_hooks()?;
    }

    exec_claude(&args)
}

pub fn cmd_hook(event: String) -> Result<()> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;

    let input: HookInput = serde_json::from_str(&raw).unwrap_or_else(|_| HookInput {
        session_id: "unknown".to_string(),
        cwd: std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().to_string()),
        hook_event_name: Some(event.clone()),
        ..Default::default()
    });

    let output = match event.as_str() {
        "prompt" => hooks::prompt::handle(&input)?,
        "stop"   => hooks::stop::handle(&input)?,
        other    => bail!("Unknown hook event: {}", other),
    };

    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

pub fn cmd_remember(text: String, tag: Option<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let scrooge_dir = resolve_scrooge_dir(&cwd);
    let conn = db::open(&scrooge_dir)?;
    let category = tag
        .as_deref()
        .map(facts::FactCategory::from_str)
        .unwrap_or(facts::FactCategory::User);
    let id = facts::insert(&conn, "manual", &cwd, &text, category)?;
    println!("Saved [{}]: {}", id, text);
    Ok(())
}

pub fn cmd_forget(id: String) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let conn = db::open(&resolve_scrooge_dir(&cwd))?;
    if facts::delete(&conn, &id)? {
        println!("Deleted: {}", id);
    } else {
        bail!("Fact not found: {}", id);
    }
    Ok(())
}

pub fn cmd_recall(query: String, limit: usize) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let conn = db::open(&resolve_scrooge_dir(&cwd))?;
    let results = facts::search(&conn, &cwd, &query, limit)?;
    if results.is_empty() {
        println!("No facts found for: {}", query);
        return Ok(());
    }
    println!("Found {} fact{}:", results.len(), if results.len() == 1 { "" } else { "s" });
    for (i, r) in results.iter().enumerate() {
        format::print_fact(&r.fact, i);
    }
    Ok(())
}

pub fn cmd_gain() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let scrooge_dir = resolve_scrooge_dir(&cwd);
    let conn = db::open(&scrooge_dir)?;
    let summary = stats::gain_summary(&conn)?;
    format::print_gain_report(&summary);
    Ok(())
}

pub fn cmd_setup() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let scrooge_dir = resolve_scrooge_dir(&cwd);
    db::open(&scrooge_dir)?;
    crate::inject::inject_hooks()?;
    maybe_gitignore(&cwd)?;
    println!("Setup complete.");
    println!("  DB:    {}", crate::config::db_path(&scrooge_dir).display());
    println!("  Hooks: {}", crate::config::settings_json_path()?.display());
    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn exec_claude(args: &[String]) -> Result<()> {
    let claude = which_claude()?;

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = std::process::Command::new(&claude).args(args).exec();
        Err(anyhow::anyhow!("Failed to exec claude: {}", err))
    }

    #[cfg(not(unix))]
    {
        let status = std::process::Command::new(&claude).args(args).status()?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn which_claude() -> Result<std::path::PathBuf> {
    let self_path = std::env::current_exe()?;
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join("claude");
        if candidate.exists() && candidate != self_path {
            return Ok(candidate);
        }
    }
    bail!("claude binary not found in PATH")
}

fn maybe_gitignore(cwd: &Path) -> Result<()> {
    let root = match crate::config::find_project_root(cwd) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };
    let gitignore = root.join(".gitignore");
    let entry = ".scrooge/";
    if gitignore.exists() {
        let content = std::fs::read_to_string(&gitignore)?;
        if content.lines().any(|l| l.trim() == entry) {
            return Ok(());
        }
        let sep = if content.ends_with('\n') { "" } else { "\n" };
        std::fs::write(&gitignore, format!("{}{}{}\n", content, sep, entry))?;
    } else {
        std::fs::write(&gitignore, format!("{}\n", entry))?;
    }
    Ok(())
}

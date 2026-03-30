use crate::config::{self, resolve_scrooge_dir};
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
        /// Include archived facts in results
        #[arg(long)]
        include_archived: bool,
    },
    /// Archive facts that have not been accessed within N days
    Expire {
        /// Days of inactivity before a fact is archived (default: 180)
        #[arg(long, default_value = "180")]
        days: i64,
        /// Show what would be archived without actually archiving
        #[arg(long)]
        dry_run: bool,
    },
    /// Show token savings analytics (also available as `scrooge --savings`)
    Gain,
    /// Install hooks and initialise the memory DB
    Setup,
    /// Initialise memory DB for this project without launching Claude
    Init,
    /// Remove scrooge hooks and delete the local memory DB
    Uninstall {
        /// Also remove the global hooks from ~/.claude/settings.json
        #[arg(long)]
        global: bool,
    },
    /// Manage per-project configuration
    Config {
        #[command(subcommand)]
        action: ConfigCommands,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show current effective configuration
    Show,
    /// Write a default config.toml to .scrooge/config.toml
    Init {
        /// Overwrite if file already exists
        #[arg(long)]
        force: bool,
    },
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

pub fn cmd_recall(query: String, limit: usize, include_archived: bool) -> Result<()> {
    let cwd  = std::env::current_dir()?;
    let conn = db::open(&resolve_scrooge_dir(&cwd))?;
    let results = if include_archived {
        facts::search_including_archived(&conn, &cwd, &query, limit)?
    } else {
        facts::search(&conn, &cwd, &query, limit, &facts::SearchOptions::default())?
    };
    if results.is_empty() {
        println!("No facts found for: {}", query);
        return Ok(());
    }
    println!("Found {} fact{}:", results.len(), if results.len() == 1 { "" } else { "s" });
    for (i, r) in results.iter().enumerate() {
        format::print_fact(&r.fact, i);
        if r.fact.archived_at.is_some() {
            println!("     [ARCHIVED]");
        }
    }
    Ok(())
}

pub fn cmd_expire(days: i64, dry_run: bool) -> Result<()> {
    if days <= 0 {
        bail!("--days must be a positive integer, got {}", days);
    }

    let cwd          = std::env::current_dir()?;
    let scrooge_dir  = resolve_scrooge_dir(&cwd);
    let conn         = db::open(&scrooge_dir)?;
    let project_path = crate::config::canonical_project_path(&cwd);
    let cutoff       = chrono::Utc::now().timestamp() - days * 86_400;

    // Collect candidates without archiving so we can show a preview first.
    let mut stmt = conn.prepare(
        "SELECT id, category, COALESCE(last_accessed, created_at), content
         FROM facts
         WHERE project_path = ?1
           AND archived_at  IS NULL
           AND COALESCE(last_accessed, created_at) < ?2
         ORDER BY COALESCE(last_accessed, created_at) ASC",
    )?;

    struct Row { id: String, category: String, last_active: i64, content: String }
    let candidates: Vec<Row> = stmt
        .query_map(rusqlite::params![project_path, cutoff], |row| {
            Ok(Row {
                id:          row.get(0)?,
                category:    row.get(1)?,
                last_active: row.get(2)?,
                content:     row.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    if candidates.is_empty() {
        println!("No facts eligible for archival (threshold: {} days).", days);
        return Ok(());
    }

    for c in &candidates {
        let date = chrono::DateTime::from_timestamp(c.last_active, 0)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "unknown".into());
        let preview: String = c.content.chars().take(60).collect();
        // Machine-readable output: tab-separated, one fact per line
        println!("WOULD_ARCHIVE\t{}\t{}\t{}\t{}", c.id, c.category, date, preview);
    }

    if dry_run {
        println!("\n(dry-run) {} fact(s) would be archived. Re-run without --dry-run to apply.", candidates.len());
        return Ok(());
    }

    print!("\nArchive {} fact(s)? [y/N] ", candidates.len());
    use std::io::Write;
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer.trim().to_lowercase() != "y" {
        println!("Aborted.");
        return Ok(());
    }

    let archived = facts::archive_facts_older_than(&conn, &cwd, days)?;
    println!("Archived {} fact(s).", archived.len());
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

pub fn cmd_init() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let scrooge_dir = resolve_scrooge_dir(&cwd);
    let db = crate::config::db_path(&scrooge_dir);
    if db.exists() {
        println!("Already initialised: {}", db.display());
        return Ok(());
    }
    db::open(&scrooge_dir)?;
    maybe_gitignore(&cwd)?;
    println!("Initialised: {}", db.display());
    println!("Run `scrooge setup` once to install the hooks globally.");
    Ok(())
}

pub fn cmd_uninstall(global: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let scrooge_dir = resolve_scrooge_dir(&cwd);

    // Remove local .scrooge/ directory
    if scrooge_dir.exists() {
        std::fs::remove_dir_all(&scrooge_dir)?;
        println!("Removed: {}", scrooge_dir.display());
    } else {
        println!("Nothing to remove: {} does not exist", scrooge_dir.display());
    }

    if global {
        crate::inject::remove_hooks()?;
        println!("Removed scrooge hooks from ~/.claude/settings.json");
    } else {
        println!("Hooks left intact. Run `scrooge uninstall --global` to remove them too.");
    }

    Ok(())
}

pub fn cmd_config(action: ConfigCommands) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let scrooge_dir = resolve_scrooge_dir(&cwd);

    match action {
        ConfigCommands::Show => {
            let cfg = config::load_config(&scrooge_dir)?;
            print!("{}", config::config_to_toml(&cfg));
            Ok(())
        }
        ConfigCommands::Init { force } => {
            let path = config::config_file_path(&scrooge_dir);
            if path.exists() && !force {
                bail!(
                    "Config file already exists: {}\nUse --force to overwrite.",
                    path.display()
                );
            }
            std::fs::create_dir_all(&scrooge_dir)?;
            let toml = config::config_to_toml(&config::ScroogeConfig::default());
            std::fs::write(&path, &toml)?;
            println!("Wrote config: {}", path.display());
            Ok(())
        }
    }
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

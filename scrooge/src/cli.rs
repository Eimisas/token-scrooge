use crate::config::{self, resolve_scrooge_dir};
use crate::db::{self, facts, stats};
use crate::format;
use crate::hooks::{self, HookInput, HookOutput};
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
    /// Manage the background scrooge daemon (required for SLM features)
    Daemon {
        #[command(subcommand)]
        action: DaemonCommands,
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

#[derive(Subcommand)]
pub enum DaemonCommands {
    /// Start the scrooge daemon in the background
    Start {
        /// Run in foreground for debugging
        #[arg(long)]
        foreground: bool,
    },
    /// Stop the running scrooge daemon
    Stop,
    /// Check if the daemon is running
    Status,
}

// ─── Command handlers ─────────────────────────────────────────────────────────

pub fn cmd_daemon(action: DaemonCommands) -> Result<()> {
    let home = dirs::home_dir().expect("no home dir");
    let scrooge_dir = home.join(".scrooge");
    let socket_path = scrooge_dir.join("daemon.sock").to_string_lossy().to_string();
    let client = crate::daemon::Client::new(&socket_path);

    match action {
        DaemonCommands::Status => {
            if client.is_running() {
                println!("Daemon is running.");
            } else {
                println!("Daemon is NOT running.");
            }
        }
        DaemonCommands::Stop => {
            if client.is_running() {
                println!("Stopping daemon...");
                let _ = client.send(crate::protocol::Request::Shutdown);
            } else {
                println!("Daemon is not running.");
            }
        }
        DaemonCommands::Start { foreground } => {
            if client.is_running() {
                println!("Daemon is already running.");
                return Ok(());
            }

            if foreground {
                run_daemon(&socket_path)?;
            } else {
                let exe = std::env::current_exe()?;
                std::process::Command::new(exe)
                    .arg("daemon")
                    .arg("start")
                    .arg("--foreground")
                    .spawn()?;
                println!("Daemon started in background.");
            }
        }
    }
    Ok(())
}

fn run_daemon(socket_path: &str) -> Result<()> {
    let home = dirs::home_dir().expect("no home dir");
    let cache_dir = home.join(".scrooge").join("models");
    std::fs::create_dir_all(&cache_dir)?;

    println!("[scrooge] Loading model Qwen2.5-0.5B-Instruct...");
    let slm = crate::models::slm::Slm::load("Qwen/Qwen2.5-0.5B-Instruct", cache_dir)?;
    let server = crate::daemon::Server::new(slm, socket_path);
    server.start()?;
    Ok(())
}

pub fn cmd_claude(args: Vec<String>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let scrooge_dir = resolve_scrooge_dir(&cwd);

    if !crate::config::db_path(&scrooge_dir).exists() {
        eprintln!("[scrooge] First run — initialising memory store at {}", scrooge_dir.display());
        db::open(&scrooge_dir)?;
        crate::inject::inject_hooks()?;
        maybe_gitignore(&cwd)?;

        eprintln!("[scrooge] Preparing local embedding model (first-time download)...");
        let _ = crate::embeddings::EmbeddingModel::load();

        eprintln!("[scrooge] Ready. Hooks installed in ~/.claude/settings.json");
    } else {
        // Keep hook path up-to-date (handles reinstalls)
        crate::inject::inject_hooks()?;
    }

    // Ensure daemon is running before handing off to claude.
    // scrooge claude always owns the stop: when this session exits the daemon
    // stops too. Users who want a persistent daemon across multiple sessions
    // should use `scrooge daemon start` directly.
    let _ = ensure_daemon_running().map_err(|e| {
        eprintln!("[scrooge] Warning: Could not start memory assistant: {}", e);
    });

    exec_claude(&args)
}

/// Start the daemon if it is not already running.
/// Returns `true` if this call started the daemon, `false` if it was already running.
fn ensure_daemon_running() -> Result<bool> {
    let home = dirs::home_dir().expect("no home dir");
    let scrooge_dir = home.join(".scrooge");
    let socket_path = scrooge_dir.join("daemon.sock").to_string_lossy().to_string();
    let client = crate::daemon::Client::new(&socket_path);

    if client.is_running() {
        return Ok(false); // already up — nothing to do
    }

    eprintln!("[scrooge] Starting memory assistant (first prompt may be slower)...");

    // Pre-download the SLM so the user sees progress here rather than silence
    // while the daemon loads in the background.
    let cache_dir = scrooge_dir.join("models");
    std::fs::create_dir_all(&cache_dir)?;
    let _ = crate::models::slm::Slm::load("Qwen/Qwen2.5-0.5B-Instruct", cache_dir)?;

    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg("daemon")
        .arg("start")
        .arg("--foreground")
        // Redirect daemon stdio so its internal logs don't appear in the user's
        // terminal while Claude is running.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    // Poll until the socket is ready (model load can take a few seconds).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if client.is_running() {
            eprintln!("[scrooge] Memory assistant ready.");
            return Ok(true);
        }
    }

    // Timed out — proceed without SLM features rather than blocking the user.
    eprintln!("[scrooge] Warning: memory assistant did not start in time; SLM features disabled for this session.");
    Ok(true) // we did spawn it, so we should attempt cleanup on exit
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
        "prompt" => hooks::prompt::handle(&input).unwrap_or_else(|e| {
            eprintln!("scrooge hook error: {e}");
            HookOutput::allow()
        }),
        "stop"   => hooks::stop::handle(&input).unwrap_or_else(|e| {
            eprintln!("scrooge hook error: {e}");
            HookOutput::allow()
        }),
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

    let model = crate::embeddings::EmbeddingModel::load().ok();
    let embedding = model.as_ref().and_then(|m| m.embed(&text).ok());

    let id = facts::insert(&conn, "manual", &cwd, &text, category, embedding.as_deref())?;
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

    let model = crate::embeddings::EmbeddingModel::load().ok();
    let query_embedding = model.and_then(|m| m.embed(&query).ok());

    let results = if include_archived {
        facts::search_including_archived(&conn, &cwd, &query, limit)?
    } else {
        let opts = facts::SearchOptions {
            query_embedding,
            ..facts::SearchOptions::default()
        };
        facts::search(&conn, &cwd, &query, limit, &opts)?
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

    eprintln!("[scrooge] Downloading embedding model (this may take a minute)...");
    match crate::embeddings::EmbeddingModel::load() {
        Ok(_)  => eprintln!("[scrooge] Embedding model ready."),
        Err(e) => eprintln!("[scrooge] Warning: Embedding model download failed: {}. (Will retry on first use)", e),
    }

    eprintln!("[scrooge] Downloading SLM (Qwen2.5-0.5B, ~350MB)...");
    let home = dirs::home_dir().expect("no home dir");
    let cache_dir = home.join(".scrooge").join("models");
    std::fs::create_dir_all(&cache_dir)?;
    match crate::models::slm::Slm::load("Qwen/Qwen2.5-0.5B-Instruct", cache_dir) {
        Ok(_)  => eprintln!("[scrooge] SLM ready."),
        Err(e) => eprintln!("[scrooge] Warning: SLM download failed: {}. (Will retry on first `scrooge claude`)", e),
    }

    println!("Setup complete: {}", crate::config::db_path(&scrooge_dir).display());
    println!("  Memory is now automatic for all sessions.");
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

    // Remove local project .scrooge/ directory
    if scrooge_dir.exists() {
        std::fs::remove_dir_all(&scrooge_dir)?;
        println!("Removed project memory: {}", scrooge_dir.display());
    }

    if global {
        crate::inject::remove_hooks()?;
        println!("Removed scrooge hooks from ~/.claude/settings.json");

        let global_dir = config::global_scrooge_dir()?;
        if global_dir.exists() {
            std::fs::remove_dir_all(&global_dir)?;
            println!("Removed global memory and models: {}", global_dir.display());
        }

        let bin = config::scrooge_binary_path()?;
        if bin.exists() {
            // Attempt to remove the binary itself. 
            // On some OSs this might fail if the process is running, 
            // but usually it works or can be renamed.
            let _ = std::fs::remove_file(&bin);
            println!("Removed scrooge binary: {}", bin.display());
        }
        
        println!("Uninstall complete.");
    } else {
        println!("Project memory removed. Run `scrooge uninstall --global` to remove hooks, models, and binary.");
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
    let status = std::process::Command::new(&claude).args(args).status()?;

    // Always stop the daemon when this session ends. scrooge claude is the
    // daemon's lifecycle owner. Users who want a persistent daemon use
    // `scrooge daemon start` directly and manage it themselves.
    let home = dirs::home_dir().expect("no home dir");
    let socket = home.join(".scrooge").join("daemon.sock");
    let client = crate::daemon::Client::new(&socket.to_string_lossy());
    if client.is_running() {
        let _ = client.send(crate::protocol::Request::Shutdown);
    }

    std::process::exit(status.code().unwrap_or(1));
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

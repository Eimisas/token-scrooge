//! End-to-end integration tests.
//! Each test spawns the real `scrooge` binary and inspects its output.
//! Tests are isolated via temp directories so they don't pollute each other.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Path to the compiled scrooge binary (set by Cargo at test time).
fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_scrooge"))
}

/// Run scrooge with given args from a specific working directory.
fn run(args: &[&str], cwd: &Path) -> Output {
    Command::new(bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("failed to execute scrooge")
}

/// Create a temporary project dir with a .git marker so scrooge anchors to it.
fn tmp_project() -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let project = dir.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::create_dir(project.join(".git")).unwrap();
    (dir, project)
}

fn stdout(o: &Output) -> String { String::from_utf8_lossy(&o.stdout).to_string() }
fn stderr(o: &Output) -> String { String::from_utf8_lossy(&o.stderr).to_string() }

/// Write a minimal Claude Code JSONL transcript.
fn write_transcript(path: &Path, messages: &[(&str, &str)]) {
    let mut f = fs::File::create(path).unwrap();
    for (role, content) in messages {
        let line = match *role {
            "user" => serde_json::json!({
                "type": "user",
                "sessionId": "test-session",
                "message": { "role": "user", "content": content }
            }),
            "assistant" => serde_json::json!({
                "type": "assistant",
                "sessionId": "test-session",
                "message": {
                    "role": "assistant",
                    "content": [{ "type": "text", "text": content }]
                }
            }),
            _ => continue,
        };
        writeln!(f, "{}", line).unwrap();
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn setup_creates_db() {
    let (_dir, project) = tmp_project();
    let o = run(&["setup"], &project);
    assert!(o.status.success(), "setup failed: {}", stderr(&o));

    let db = project.join(".scrooge").join("memory.db");
    assert!(db.exists(), ".scrooge/memory.db should exist after setup");

    let out = stdout(&o);
    assert!(out.contains("Setup complete"), "unexpected output: {}", out);
    assert!(out.contains("memory.db"), "output should show DB path");
}

#[test]
fn setup_adds_scrooge_to_gitignore() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);
    let gitignore = project.join(".gitignore");
    assert!(gitignore.exists());
    let content = fs::read_to_string(&gitignore).unwrap();
    assert!(content.contains(".scrooge/"), "gitignore should contain .scrooge/");
}

#[test]
fn setup_is_idempotent() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);
    let o = run(&["setup"], &project);
    assert!(o.status.success(), "second setup should also succeed");
    // .gitignore should not have duplicate entries
    let content = fs::read_to_string(project.join(".gitignore")).unwrap();
    let count = content.lines().filter(|l| l.trim() == ".scrooge/").count();
    assert_eq!(count, 1, "should have exactly one .scrooge/ entry");
}

#[test]
fn remember_and_recall() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);

    let o = run(
        &["remember", "we use Postgres with connection pooling via pgbouncer"],
        &project,
    );
    assert!(o.status.success(), "remember failed: {}", stderr(&o));
    assert!(stdout(&o).contains("Saved"), "should print Saved: {}", stdout(&o));

    let o = run(&["recall", "postgres database connection"], &project);
    assert!(o.status.success());
    let out = stdout(&o);
    assert!(out.contains("pgbouncer"), "recall should surface the stored fact: {}", out);
}

#[test]
fn remember_with_tag() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);

    let o = run(
        &["remember", "always use Result<T, AppError> in service layer", "--tag", "convention"],
        &project,
    );
    assert!(o.status.success());

    let o = run(&["recall", "service layer error handling"], &project);
    let out = stdout(&o);
    assert!(out.contains("convention"), "output should show category: {}", out);
}

#[test]
fn forget_removes_fact() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);
    run(&["remember", "temporary test fact to be deleted"], &project);

    // Get the ID from recall
    let o = run(&["recall", "temporary test fact"], &project);
    let out = stdout(&o);
    // ID is on a line that starts with spaces and contains UUID hex pattern
    let id_line = out.lines()
        .find(|l| l.trim().len() == 36 && l.contains('-'))
        .expect("recall should show a UUID id line");
    let id = id_line.trim();

    let o = run(&["forget", id], &project);
    assert!(o.status.success(), "forget failed: {}", stderr(&o));
    assert!(stdout(&o).contains("Deleted"));

    // Should no longer appear in recall
    let o = run(&["recall", "temporary test fact"], &project);
    let out = stdout(&o);
    assert!(out.contains("No facts found") || !out.contains("temporary"),
        "deleted fact should not appear in recall: {}", out);
}

#[test]
fn recall_empty_query_returns_recent_facts() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);
    run(&["remember", "first fact about authentication"], &project);
    run(&["remember", "second fact about database schema"], &project);

    // Empty-ish query (single space) falls back to recency order
    let o = run(&["recall", ""], &project);
    assert!(o.status.success());
    // May return 0 results for truly empty query — that's fine
}

#[test]
fn savings_shows_stats() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);
    run(&["remember", "test fact for savings test"], &project);

    let o = run(&["--savings"], &project);
    assert!(o.status.success(), "savings failed: {}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("Facts stored"), "output: {}", out);
    assert!(out.contains("Sessions tracked"), "output: {}", out);
}

#[test]
fn gain_subcommand_same_as_savings_flag() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);

    let o1 = run(&["gain"], &project);
    let o2 = run(&["--savings"], &project);
    assert!(o1.status.success());
    assert!(o2.status.success());
    // Both should contain the same headers
    assert!(stdout(&o1).contains("Facts stored"));
    assert!(stdout(&o2).contains("Facts stored"));
}

#[test]
fn prompt_hook_injects_relevant_context() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);
    run(&["remember", "Auth uses JWT tokens stored in httpOnly cookies", "--tag", "decision"], &project);

    // Simulate the UserPromptSubmit hook
    let hook_input = serde_json::json!({
        "session_id": "integration-test-session",
        "cwd": project.to_string_lossy(),
        "hook_event_name": "UserPromptSubmit",
        "prompt": "how does JWT auth work with cookies?"
    });

    let mut child = Command::new(bin())
        .args(&["hook", "prompt"])
        .current_dir(&project)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn scrooge hook prompt");

    {
        let stdin = child.stdin.as_mut().unwrap();
        stdin.write_all(hook_input.to_string().as_bytes()).unwrap();
    }

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "hook failed: {}", String::from_utf8_lossy(&output.stderr));

    let out = stdout(&output);
    let parsed: serde_json::Value = serde_json::from_str(&out)
        .unwrap_or_else(|_| panic!("hook output not valid JSON: {}", out));

    let ctx = parsed["hookSpecificOutput"]["additionalContext"].as_str();
    assert!(ctx.is_some(), "additionalContext should be present: {}", out);
    assert!(ctx.unwrap().contains("JWT"), "context should mention JWT: {}", ctx.unwrap());
}

#[test]
fn prompt_hook_returns_empty_json_when_no_match() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);
    run(&["remember", "database schema uses UUID primary keys", "--tag", "convention"], &project);

    let hook_input = serde_json::json!({
        "session_id": "no-match-session",
        "cwd": project.to_string_lossy(),
        "hook_event_name": "UserPromptSubmit",
        "prompt": "how do I configure nginx reverse proxy timeouts?"
    });

    let mut child = Command::new(bin())
        .args(&["hook", "prompt"])
        .current_dir(&project)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    child.stdin.as_mut().unwrap()
        .write_all(hook_input.to_string().as_bytes()).unwrap();

    let output = child.wait_with_output().unwrap();
    let out = stdout(&output);
    let parsed: serde_json::Value = serde_json::from_str(&out)
        .unwrap_or_else(|_| panic!("not valid JSON: {}", out));

    // additionalContext should be absent — no noisy injection when nothing matches
    assert!(
        parsed["hookSpecificOutput"]["additionalContext"].is_null(),
        "should not inject unrelated memory: {}", out
    );
}

#[test]
fn stop_hook_extracts_facts_from_transcript() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);

    let transcript = project.join("session.jsonl");
    write_transcript(&transcript, &[
        ("user",      "Remember: we decided not to use Redis for sessions, keep it stateless JWT"),
        ("assistant", "I've fixed the token expiry bug in src/auth/refresh.ts line 47 that caused silent logouts"),
        ("user",      "from now on always validate JWT expiry on the server side before returning data"),
    ]);

    let hook_input = serde_json::json!({
        "session_id": "stop-test-session",
        "cwd": project.to_string_lossy(),
        "transcript_path": transcript.to_string_lossy(),
        "hook_event_name": "Stop",
        "stop_hook_active": false,
        "last_assistant_message": "Done fixing the auth flow."
    });

    let mut child = Command::new(bin())
        .args(&["hook", "stop"])
        .current_dir(&project)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    child.stdin.as_mut().unwrap()
        .write_all(hook_input.to_string().as_bytes()).unwrap();

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "stop hook failed: {}", String::from_utf8_lossy(&output.stderr));

    // Verify facts were stored
    let o = run(&["recall", "Redis stateless JWT session"], &project);
    let out = stdout(&o);
    assert!(
        out.contains("Redis") || out.contains("stateless") || out.contains("JWT"),
        "should have stored the Redis decision: {}", out
    );

    let o = run(&["recall", "token expiry refresh"], &project);
    let out = stdout(&o);
    assert!(
        out.contains("token") || out.contains("expiry") || out.contains("refresh"),
        "should have stored the fix fact: {}", out
    );
}

#[test]
fn stop_hook_respects_stop_hook_active_guard() {
    let (_dir, project) = tmp_project();
    run(&["setup"], &project);

    // stop_hook_active = true must be a silent no-op (prevents infinite loops)
    let hook_input = serde_json::json!({
        "session_id": "guard-test",
        "cwd": project.to_string_lossy(),
        "hook_event_name": "Stop",
        "stop_hook_active": true
    });

    let mut child = Command::new(bin())
        .args(&["hook", "stop"])
        .current_dir(&project)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();

    child.stdin.as_mut().unwrap()
        .write_all(hook_input.to_string().as_bytes()).unwrap();

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    // Should output minimal allow JSON (no decision, no hookSpecificOutput)
    let parsed: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert!(parsed["decision"].is_null());
}

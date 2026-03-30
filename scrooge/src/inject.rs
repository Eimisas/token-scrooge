use crate::config::{scrooge_binary_path, settings_json_path};
use anyhow::Result;
use serde_json::{json, Value};
use std::fs;

/// Inject (or update) scrooge hooks in ~/.claude/settings.json.
/// Idempotent — safe to call on every startup.
pub fn inject_hooks() -> Result<()> {
    let settings_path = settings_json_path()?;
    let binary_path = scrooge_binary_path()?.to_string_lossy().to_string();

    let mut settings: Value = if settings_path.exists() {
        let raw = fs::read_to_string(&settings_path)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };

    let modified =
        inject_event_hook(&mut settings, "UserPromptSubmit", &binary_path, "hook prompt")
        | inject_event_hook(&mut settings, "Stop", &binary_path, "hook stop");

    if modified {
        let parent = settings_path.parent().expect("settings.json has no parent dir");
        fs::create_dir_all(parent)?;
        let tmp = settings_path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(&settings)?)?;
        fs::rename(&tmp, &settings_path)?;
    }

    Ok(())
}

/// Returns true if settings were changed.
fn inject_event_hook(
    settings: &mut Value,
    event: &str,
    binary_path: &str,
    subcommand: &str,
) -> bool {
    let command = format!("{} {}", binary_path, subcommand);

    // Ensure settings["hooks"][event] exists as an array
    if settings.get("hooks").is_none() {
        settings["hooks"] = json!({});
    }
    if settings["hooks"].get(event).is_none() {
        settings["hooks"][event] = json!([]);
    }

    let hooks_arr = match settings["hooks"][event].as_array_mut() {
        Some(a) => a,
        None => {
            settings["hooks"][event] = json!([]);
            settings["hooks"][event].as_array_mut().unwrap()
        }
    };

    // Scan for an existing scrooge entry
    for entry in hooks_arr.iter_mut() {
        let inner = match entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            Some(a) => a,
            None => continue,
        };
        for hook in inner.iter_mut() {
            let cmd = match hook.get("command").and_then(|c| c.as_str()) {
                Some(s) => s,
                None => continue,
            };
            if cmd.contains("scrooge") {
                if cmd == command {
                    return false; // already correct, nothing to do
                }
                // Path changed — update in place
                hook["command"] = json!(command);
                return true;
            }
        }
    }

    // No existing entry — append a fresh one
    hooks_arr.push(json!({
        "matcher": "",
        "hooks": [{ "type": "command", "command": command }]
    }));
    true
}

/// Remove all scrooge hooks from settings.json (for uninstall).
#[allow(dead_code)]
pub fn remove_hooks() -> Result<()> {
    let settings_path = settings_json_path()?;
    if !settings_path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(&settings_path)?;
    let mut settings: Value = serde_json::from_str(&raw)?;

    for event in &["UserPromptSubmit", "Stop"] {
        if let Some(arr) = settings["hooks"][event].as_array_mut() {
            arr.retain(|entry| {
                !entry
                    .get("hooks")
                    .and_then(|h| h.as_array())
                    .map(|hooks| {
                        hooks.iter().any(|h| {
                            h.get("command")
                                .and_then(|c| c.as_str())
                                .map(|c| c.contains("scrooge"))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            });
        }
    }

    let tmp = settings_path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_string_pretty(&settings)?)?;
    fs::rename(&tmp, &settings_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injects_into_empty_settings() {
        let mut s = json!({});
        inject_event_hook(&mut s, "UserPromptSubmit", "/usr/local/bin/scrooge", "hook prompt");
        let arr = s["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["hooks"][0]["command"], "/usr/local/bin/scrooge hook prompt");
    }

    #[test]
    fn idempotent() {
        let mut s = json!({});
        let c1 = inject_event_hook(&mut s, "Stop", "/usr/local/bin/scrooge", "hook stop");
        let c2 = inject_event_hook(&mut s, "Stop", "/usr/local/bin/scrooge", "hook stop");
        assert!(c1);
        assert!(!c2);
        assert_eq!(s["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn updates_stale_path() {
        let mut s = json!({
            "hooks": { "Stop": [{
                "matcher": "",
                "hooks": [{ "type": "command", "command": "/old/scrooge hook stop" }]
            }]}
        });
        let changed = inject_event_hook(&mut s, "Stop", "/new/scrooge", "hook stop");
        assert!(changed);
        assert_eq!(s["hooks"]["Stop"][0]["hooks"][0]["command"], "/new/scrooge hook stop");
    }
}

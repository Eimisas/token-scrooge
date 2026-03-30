pub mod prompt;
pub mod stop;

use serde::{Deserialize, Serialize};

/// JSON payload received on stdin for every hook event.
#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
pub struct HookInput {
    pub session_id:              String,
    pub transcript_path:         Option<String>,
    pub cwd:                     Option<String>,
    pub hook_event_name:         Option<String>,
    // UserPromptSubmit
    pub prompt:                  Option<String>,
    // Stop
    #[serde(default)]
    pub stop_hook_active:        bool,
    pub last_assistant_message:  Option<String>,
}

/// JSON written to stdout to communicate back to Claude Code.
#[derive(Debug, Serialize, Default)]
pub struct HookOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    #[serde(rename = "hookSpecificOutput", skip_serializing_if = "Option::is_none")]
    pub hook_specific_output: Option<HookSpecificOutput>,
}

#[derive(Debug, Serialize)]
pub struct HookSpecificOutput {
    #[serde(rename = "hookEventName")]
    pub hook_event_name: String,

    /// Injected invisibly into Claude's system context. The only injection
    /// mechanism available from UserPromptSubmit.
    #[serde(rename = "additionalContext", skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
}

impl HookOutput {
    pub fn allow() -> Self {
        Self::default()
    }

    pub fn allow_with_context(event_name: &str, context: String) -> Self {
        HookOutput {
            hook_specific_output: Some(HookSpecificOutput {
                hook_event_name:    event_name.to_string(),
                additional_context: Some(context),
            }),
            ..Self::default()
        }
    }
}

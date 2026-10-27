//! UI-safe, provider-neutral pending requests. Wire request IDs never leave the
//! owning runtime worker; opaque tokens prevent cross-run and stale responses.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Codex workspace boundary. Read-only intent always wins over this setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexAccess {
    #[default]
    Workspace,
    Full,
}
impl CodexAccess {
    pub fn sandbox(self, edits: bool) -> &'static str {
        if !edits { "read-only" } else if self == Self::Full { "danger-full-access" } else { "workspace-write" }
    }
    pub fn policy(self, edits: bool, project: &std::path::Path) -> Value {
        if !edits { serde_json::json!({"type":"readOnly","networkAccess":false}) }
        else if self == Self::Full { serde_json::json!({"type":"dangerFullAccess"}) }
        else { serde_json::json!({"type":"workspaceWrite","writableRoots":[project],"networkAccess":false,"excludeSlashTmp":true,"excludeTmpdirEnvVar":true}) }
    }
}

/// Command approval review is independent of filesystem sandbox permissions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexApprovals {
    #[default]
    Ask,
    AutoReview,
    Never,
}
impl CodexApprovals {
    pub fn policy(self) -> &'static str {
        match self {
            Self::Never => "never",
            _ => "on-request",
        }
    }
    pub fn reviewer(self) -> &'static str {
        match self {
            Self::AutoReview => "auto_review",
            _ => "user",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeOption {
    pub id: String,
    pub label: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<NativeOption>,
    pub secret: bool,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default)]
    pub required: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeRequestKind {
    Approval { choices: Vec<NativeOption> },
    Questions { questions: Vec<NativeQuestion> },
    Unsupported,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRequest {
    pub id: String,
    pub method: String,
    pub title: String,
    pub detail: String,
    #[serde(rename = "request_kind")]
    pub kind: NativeRequestKind,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeAnswer {
    Cancel,
    Choice(String),
    Answers(BTreeMap<String, Vec<String>>),
}
#[derive(Debug, Clone)]
pub enum NativeControl {
    Respond {
        id: String,
        answer: NativeAnswer,
    },
    Steer(String),
    Interrupt,
    #[allow(dead_code)] // Kept for protocol extensions; every adapter gates methods.
    Rpc {
        method: String,
        params: Value,
    },
}

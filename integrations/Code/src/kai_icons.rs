//! Kai's semantic icon selection. Matching does not allocate or load files.

#[cfg(test)]
use crate::svg_assets as embedded_assets;

pub const SCENES: [&str; 24] = [
    "listening",
    "thinking",
    "waiting",
    "pencil",
    "code",
    "terminal",
    "files",
    "git",
    "search",
    "agents",
    "tool",
    "image",
    "document",
    "read",
    "plan",
    "review",
    "settings",
    "permission",
    "question",
    "compact",
    "success",
    "failure",
    "declined",
    "warning",
];

/// Returns a scene identifier understood by the native morphing player.
pub fn scene_for_status(label: &str) -> &'static str {
    if let Some(scene) = SCENES
        .iter()
        .find(|scene| label.eq_ignore_ascii_case(scene))
    {
        return scene;
    }
    let rules: &[(&str, &[&str])] = &[
        ("failure", &["failed", "failure", "error", "needsattention"]),
        (
            "declined",
            &["declined", "denied", "cancelled", "canceled", "stopped"],
        ),
        ("warning", &["warning", "warn"]),
        (
            "permission",
            &["permission", "approval", "trust", "authenticate", "login"],
        ),
        (
            "question",
            &[
                "question",
                "requestuserinput",
                "askuserquestion",
                "userinput",
            ],
        ),
        (
            "success",
            &[
                "completed",
                "complete",
                "finished",
                "succeeded",
                "success",
                "done",
            ],
        ),
        (
            "listening",
            &["listening", "dictation", "microphone", "voice"],
        ),
        (
            "compact",
            &[
                "compact",
                "compacting",
                "summarizing",
                "summarise",
                "context",
            ],
        ),
        (
            "agents",
            &[
                "agents",
                "agent",
                "delegate",
                "delegating",
                "delegation",
                "spawnagent",
                "subagent",
                "waitagent",
                "sendmessagetoagent",
            ],
        ),
        (
            "git",
            &[
                "git", "commit", "checkout", "branch", "rebase", "push", "pull", "diff",
            ],
        ),
        (
            "terminal",
            &[
                "terminal",
                "shell",
                "bash",
                "execcommand",
                "commandexecution",
                "command",
                "runcommand",
                "writestdin",
                "execute",
                "executing",
            ],
        ),
        (
            "search",
            &[
                "search",
                "searching",
                "grep",
                "ripgrep",
                "web",
                "browse",
                "browsing",
                "fetch",
            ],
        ),
        (
            "image",
            &[
                "image",
                "images",
                "imagegen",
                "viewimage",
                "screenshot",
                "photo",
            ],
        ),
        (
            "read",
            &["read", "reading", "readfile", "readfiles", "openfile"],
        ),
        ("document", &["document", "documents", "pdf", "markdown"]),
        (
            "files",
            &[
                "files",
                "file",
                "listfiles",
                "listdirectory",
                "glob",
                "directory",
                "explorer",
            ],
        ),
        (
            "plan",
            &["plan", "planning", "updateplan", "todo", "todowrite"],
        ),
        (
            "review",
            &[
                "review",
                "reviewing",
                "verify",
                "verifying",
                "test",
                "tests",
                "testing",
            ],
        ),
        ("settings", &["settings", "configure", "configuration"]),
        (
            "code",
            &[
                "code",
                "edit",
                "editing",
                "applypatch",
                "writefile",
                "replace",
                "multiedit",
            ],
        ),
        (
            "pencil",
            &[
                "write",
                "writing",
                "responding",
                "generating",
                "answering",
                "streaming",
            ],
        ),
        (
            "thinking",
            &["thinking", "reasoning", "analyzing", "analysing"],
        ),
        (
            "waiting",
            &[
                "waiting",
                "idle",
                "connecting",
                "reconnecting",
                "loading",
                "queued",
            ],
        ),
    ];
    for (scene, names) in rules {
        if names.iter().any(|name| has_compound(label, name)) {
            return scene;
        }
    }
    "tool"
}

/// Recognizes service names, actual MCP namespaces, and document URLs.
/// Whole-word matching avoids interpreting local `git`, `sandbox`, or
/// `linearize` tools as GitHub, Box, or Linear integrations.
pub fn integration_for_label(label: &str) -> Option<&'static str> {
    for (url, id) in [
        ("docs.google.com/presentation", "googleslides"),
        ("docs.google.com/spreadsheets", "googlesheets"),
        ("docs.google.com/document", "googledocs"),
        ("drive.google.com", "googledrive"),
        ("calendar.google.com", "googlecalendar"),
        ("mail.google.com", "gmail"),
    ] {
        if ascii_contains(label, url) {
            return Some(id);
        }
    }
    // Specific Google document tools win over their shared Drive MCP server.
    for (id, aliases) in [
        ("googleslides", &["googleslides"] as &[&str]),
        ("googlesheets", &["googlesheets"]),
        ("googledocs", &["googledocs"]),
        ("googlecalendar", &["googlecalendar", "gcal"]),
        ("googledrive", &["googledrive", "gdrive"]),
        ("gmail", &["gmail"]),
        ("notion", &["notion"]),
        ("github", &["github"]),
        ("spotify", &["spotify"]),
        ("supabase", &["supabase"]),
        ("vercel", &["vercel"]),
        ("linear", &["linear"]),
        ("dropbox", &["dropbox"]),
        ("figma", &["figma"]),
        ("box", &["box"]),
        ("jira", &["jira"]),
        ("confluence", &["confluence"]),
        ("youtube", &["youtube", "youtu"]),
        ("slack", &["slack"]),
        ("linkedin", &["linkedin"]),
    ] {
        if aliases.iter().any(|alias| has_compound(label, alias)) {
            return Some(id);
        }
    }
    None
}

pub fn icon_for_tool(label: &str) -> &'static str {
    if let Some(id) = integration_for_label(label) {
        return integration_asset(id);
    }
    status_asset(scene_for_status(label))
}

pub fn integration_asset(id: &str) -> &'static str {
    match id {
        "googleslides" => "assets/kai/integrations/googleslides.svg",
        "googlesheets" => "assets/kai/integrations/googlesheets.svg",
        "googledocs" => "assets/kai/integrations/googledocs.svg",
        "googledrive" => "assets/kai/integrations/googledrive.svg",
        "googlecalendar" => "assets/kai/integrations/googlecalendar.svg",
        "gmail" => "assets/kai/integrations/gmail.svg",
        "notion" => "assets/kai/integrations/notion.svg",
        "github" => "assets/kai/integrations/github.svg",
        "spotify" => "assets/kai/integrations/spotify.svg",
        "supabase" => "assets/kai/integrations/supabase.svg",
        "vercel" => "assets/kai/integrations/vercel.svg",
        "linear" => "assets/kai/integrations/linear.svg",
        "dropbox" => "assets/kai/integrations/dropbox.svg",
        "figma" => "assets/kai/integrations/figma.svg",
        "box" => "assets/kai/integrations/box.svg",
        "jira" => "assets/kai/integrations/jira.svg",
        "confluence" => "assets/kai/integrations/confluence.svg",
        "youtube" => "assets/kai/integrations/youtube.svg",
        "slack" => "assets/kai/integrations/slack.svg",
        "linkedin" => "assets/kai/integrations/linkedin.svg",
        _ => "assets/kai/status/tool.svg",
    }
}

pub fn status_asset(id: &str) -> &'static str {
    match id {
        "listening" => "assets/kai/status/listening.svg",
        "thinking" => "assets/kai/status/thinking.svg",
        "waiting" => "assets/kai/status/waiting.svg",
        "pencil" => "assets/kai/status/pencil.svg",
        "code" => "assets/kai/status/code.svg",
        "terminal" => "assets/kai/status/terminal.svg",
        "files" => "assets/kai/status/files.svg",
        "git" => "assets/kai/status/git.svg",
        "search" => "assets/kai/status/search.svg",
        "agents" => "assets/kai/status/agents.svg",
        "image" => "assets/kai/status/image.svg",
        "document" => "assets/kai/status/document.svg",
        "read" => "assets/kai/status/read.svg",
        "plan" => "assets/kai/status/plan.svg",
        "review" => "assets/kai/status/review.svg",
        "settings" => "assets/kai/status/settings.svg",
        "permission" => "assets/kai/status/permission.svg",
        "question" => "assets/kai/status/question.svg",
        "compact" => "assets/kai/status/compact.svg",
        "success" => "assets/kai/status/success.svg",
        "failure" => "assets/kai/status/failure.svg",
        "declined" => "assets/kai/status/declined.svg",
        "warning" => "assets/kai/status/warning.svg",
        "file-open" => "assets/kai/status/file-open.svg",
        "file-closed" => "assets/kai/status/file-closed.svg",
        _ => "assets/kai/status/tool.svg",
    }
}

fn ascii_contains(text: &str, needle: &str) -> bool {
    text.as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

// Separators can be underscores, hyphens, spaces, or MCP namespace punctuation.
// Match consecutive whole tokens against a compact alias without allocating.
fn has_compound(text: &str, compact: &str) -> bool {
    let bytes = text.as_bytes();
    let target = compact.as_bytes();
    for start in 0..bytes.len() {
        if !bytes[start].is_ascii_alphanumeric()
            || (start > 0 && bytes[start - 1].is_ascii_alphanumeric())
        {
            continue;
        }
        let mut matched = 0;
        for (offset, byte) in bytes[start..].iter().enumerate() {
            if !byte.is_ascii_alphanumeric() {
                continue;
            }
            if !byte.eq_ignore_ascii_case(&target[matched]) {
                break;
            }
            matched += 1;
            if matched == target.len() {
                let end = start + offset + 1;
                if end == bytes.len() || !bytes[end].is_ascii_alphanumeric() {
                    return true;
                }
                break;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_kai_scenes_keep_their_identifiers() {
        for scene in SCENES {
            assert_eq!(scene_for_status(scene), scene);
            assert!(status_asset(scene).ends_with(&format!("/{scene}.svg")));
        }
    }

    #[test]
    fn native_tool_names_and_outcomes_select_semantic_scenes() {
        for (label, expected) in [
            ("Using exec_command", "terminal"),
            ("Running a command", "terminal"),
            ("Tool needs attention · Patch failed", "failure"),
            ("Tool needs attention", "failure"),
            ("Using apply_patch", "code"),
            ("Using read_file", "read"),
            ("Using spawn_agent", "agents"),
            ("Using request_user_input", "question"),
            ("Permission denied", "declined"),
            ("Agent failed", "failure"),
            ("Tool completed", "success"),
            ("Processing agent context", "compact"),
            ("Reconnecting", "waiting"),
            ("Using unfamiliar_tool", "tool"),
        ] {
            assert_eq!(scene_for_status(label), expected, "{label}");
        }
    }

    #[test]
    fn integration_names_match_real_namespaces_and_document_urls() {
        for (label, expected) in [
            ("mcp__codex_apps__google_drive__search", "googledrive"),
            (
                "mcp__codex_apps__google_drive__google_docs_edit",
                "googledocs",
            ),
            ("Google Calendar", "googlecalendar"),
            (
                "https://docs.google.com/presentation/d/example",
                "googleslides",
            ),
            (
                "https://docs.google.com/spreadsheets/d/example",
                "googlesheets",
            ),
            ("https://docs.google.com/document/d/example", "googledocs"),
            ("https://drive.google.com/file/d/example", "googledrive"),
            ("Using mcp__github__get_issue", "github"),
            ("Using MCP__SLACK__SEARCH", "slack"),
            ("mcp__atlassian__jira_search", "jira"),
        ] {
            assert_eq!(integration_for_label(label), Some(expected), "{label}");
        }
        for local in [
            "git status",
            "git_push",
            "sandbox",
            "linearize",
            "boxing",
            "Githubish",
        ] {
            assert_eq!(integration_for_label(local), None, "{local}");
        }
    }

    #[test]
    fn every_integration_has_a_local_asset() {
        for id in [
            "googleslides",
            "googlesheets",
            "googledocs",
            "googledrive",
            "googlecalendar",
            "gmail",
            "notion",
            "github",
            "spotify",
            "supabase",
            "vercel",
            "linear",
            "dropbox",
            "figma",
            "box",
            "jira",
            "confluence",
            "youtube",
            "slack",
            "linkedin",
        ] {
            assert_eq!(integration_for_label(id), Some(id));
            assert!(integration_asset(id).ends_with(&format!("/{id}.svg")));
            assert_eq!(icon_for_tool(id), integration_asset(id));
        }
    }

    #[test]
    fn all_exported_artwork_resolves_through_embedded_catalog() {
        let paths = SCENES
            .into_iter()
            .chain(["file-open", "file-closed"])
            .map(status_asset)
            .chain(
                [
                    "googleslides",
                    "googlesheets",
                    "googledocs",
                    "googledrive",
                    "googlecalendar",
                    "gmail",
                    "notion",
                    "github",
                    "spotify",
                    "supabase",
                    "vercel",
                    "linear",
                    "dropbox",
                    "figma",
                    "box",
                    "jira",
                    "confluence",
                    "youtube",
                    "slack",
                    "linkedin",
                ]
                .into_iter()
                .map(integration_asset),
            );
        for path in paths {
            let svg = embedded_assets::asset(path).expect(path);
            let text = std::str::from_utf8(svg).expect("SVG must be UTF-8");
            assert!(text.starts_with("<svg"), "{path}");
            assert!(text.contains("viewBox="), "{path}");
            assert!(text.contains("</svg>"), "{path}");
        }
    }
}

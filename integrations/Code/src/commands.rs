//! Small native action catalog. Never sends terminal UI commands as model prose.
use crate::discovery::AgentKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Workflow,
    Artifacts,
    Fork,
    New,
    Search,
    Projects,
    Files,
    Diffs,
    Undo,
    Redo,
    Children,
    Settings,
    Wallpaper,
    Models,
    Agents,
    Status,
    Context,
    Stop,
    ReadOnly,
    Edit,
    Permissions,
    CopySession,
    Latest,
    Motion,
    Sidebar,
    Conversations,
    Attach,
    ChatContext,
    Team,
    Shell,
    Dictate,
    Help,
    Handoff,
    Delegate,
    Orchestrate,
    Compact,
    Steer,
    Terminal,
    Interrupt,
    Review,
    Goal,
    Inventory(crate::agent_controls::CatalogKind),
}
#[derive(Clone, Copy, Debug)]
pub struct Command {
    pub name: &'static str,
    pub description: &'static str,
    pub action: Action,
    /// Only commands documented by this provider receive its equivalent label.
    pub native_equivalent: bool,
}
#[derive(Clone)]
pub enum Entry {
    Local(Command),
    Native(std::sync::Arc<str>),
}
impl Entry {
    pub fn name(&self) -> &str {
        match self {
            Self::Local(c) => c.name,
            Self::Native(name) => name,
        }
    }
    pub fn description(&self) -> &str {
        match self {
            Self::Local(c) => c.description,
            Self::Native(_) => "Run this command through the installed CLI",
        }
    }
    pub fn source(&self) -> &'static str {
        match self {
            Self::Local(c) if c.native_equivalent => "Code equivalent",
            Self::Local(_) => "Code",
            Self::Native(_) => "CLI",
        }
    }
}
pub fn advertised(value: &serde_json::Value) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    if let Some(commands) = value.as_array() {
        for name in commands
            .iter()
            .take(256)
            .filter_map(serde_json::Value::as_str)
        {
            if !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':'))
            {
                names.insert(name.to_owned());
            }
            if names.len() == 128 {
                break;
            }
        }
    }
    names.into_iter().collect()
}
const BASE: &[(&str, &str, Action)] = &[
    ("workflow", "Save or run a named project workflow", Action::Workflow),
    ("artifacts", "Find saved documents and interactive reports", Action::Artifacts),
    ("fork", "Branch this conversation without changing its source", Action::Fork),
    (
        "children",
        "Inspect and control native subagents",
        Action::Children,
    ),
    (
        "undo",
        "Preview undoing this chat's latest prompt changes",
        Action::Undo,
    ),
    (
        "redo",
        "Preview redoing this chat's latest undone prompt",
        Action::Redo,
    ),
    ("orchestrate", "Start a Codex workspace orchestrator", Action::Orchestrate),
    ("new", "Start a new conversation", Action::New),
    (
        "clear",
        "Start fresh; keep the previous conversation saved",
        Action::New,
    ),
    (
        "resume",
        "Find and resume a saved conversation",
        Action::Search,
    ),
    ("search", "Search conversations", Action::Search),
    (
        "sessions",
        "Show or hide the conversations list",
        Action::Conversations,
    ),
    ("projects", "Choose a project", Action::Projects),
    (
        "files",
        "Show or hide the native file explorer",
        Action::Files,
    ),
    ("diff", "Review current Git changes", Action::Diffs),
    ("settings", "Open appearance settings", Action::Settings),
    (
        "wallpaper",
        "Adjust background artwork and effects",
        Action::Wallpaper,
    ),
    ("model", "Choose a model for this agent", Action::Models),
    ("agent", "Choose a coding CLI", Action::Agents),
    (
        "handoff",
        "Continue with another agent and transferred context",
        Action::Handoff,
    ),
    (
        "delegate",
        "Ask this agent to launch a native subagent for a task",
        Action::Delegate,
    ),
    (
        "status",
        "Agent, native session, mode, and reported usage",
        Action::Status,
    ),
    (
        "context",
        "Inspect reported context and usage",
        Action::Context,
    ),
    (
        "usage",
        "Inspect provider-reported token and cost data",
        Action::Context,
    ),
    (
        "permissions",
        "Choose read-only or workspace editing",
        Action::Permissions,
    ),
    (
        "read-only",
        "Use the CLI's existing read-only/plan policy",
        Action::ReadOnly,
    ),
    (
        "edit",
        "Allow workspace edits; keep native tool permissions",
        Action::Edit,
    ),
    (
        "copy-session",
        "Copy the current native session ID",
        Action::CopySession,
    ),
    ("latest", "Jump to recent messages", Action::Latest),
    ("motion", "Toggle interface animations", Action::Motion),
    (
        "sidebar",
        "Show or hide the project sidebar",
        Action::Sidebar,
    ),
    ("attach", "Add images to the prompt", Action::Attach),
    (
        "attach-chat",
        "Attach saved context from a chat in any project",
        Action::ChatContext,
    ),
    (
        "team",
        "Assign tasks to a team of installed coding agents",
        Action::Team,
    ),
    (
        "shell",
        "Open or hide the native interactive terminal",
        Action::Shell,
    ),
    (
        "dictate",
        "Start or stop Apple Speech dictation · Fn + Control",
        Action::Dictate,
    ),
    (
        "stop",
        "Stop this conversation's running agent",
        Action::Stop,
    ),
    ("help", "Browse commands available in Code", Action::Help),
];

fn build_catalog(agent: AgentKind) -> Vec<Command> {
    let mut commands: Vec<_> = BASE
        .iter()
        .filter(|(_, _, action)| {
            let capabilities = crate::provider_capabilities::for_provider(agent);
            (*action != Action::Delegate || capabilities.native_delegation)
                && (*action != Action::Attach || capabilities.image_input)
        })
        .map(|&(name, description, action)| Command {
            name,
            description,
            action,
            native_equivalent: match agent {
                AgentKind::Codex => matches!(
                    name,
                    "new" | "resume" | "model" | "status" | "diff" | "permissions" | "help"
                ),
                AgentKind::Claude => matches!(
                    name,
                    "clear" | "resume" | "model" | "status" | "context" | "permissions" | "help"
                ),
                AgentKind::Gemini => matches!(name, "clear" | "model" | "help"),
                AgentKind::OpenCode => {
                    matches!(name, "new" | "clear" | "sessions" | "resume" | "help")
                }
                AgentKind::Cursor => matches!(name, "new" | "clear" | "resume" | "model" | "help"),
                // Headless flags are supported; terminal slash catalogs are not assumed.
                AgentKind::Antigravity | AgentKind::Grok => false,
            },
        })
        .collect();
    if agent == AgentKind::Codex {
        commands.push(Command { name: "review", description: "Run Codex's native reviewer: changes, base <branch>, commit <hash>, or instructions", action: Action::Review, native_equivalent: true });
        commands.push(Command { name: "goal", description: "Show, set, pause, resume, clear, or budget this conversation's native goal", action: Action::Goal, native_equivalent: true });
        commands.push(Command {
            name: "terminal",
            description: "Run a command through Codex's workspace sandbox",
            action: Action::Terminal,
            native_equivalent: false,
        });
        commands.push(Command {
            name: "steer",
            description: "Send steering text to the active native Codex turn",
            action: Action::Steer,
            native_equivalent: true,
        });
        commands.push(Command {
            name: "interrupt",
            description: "Interrupt this agent's active turn",
            action: Action::Interrupt,
            native_equivalent: true,
        });
        commands.push(Command {
            name: "compact",
            description: "Compact this native Codex session",
            action: Action::Compact,
            native_equivalent: true,
        });
        for (name, description, kind) in [
            (
                "skills",
                "Inspect this project's native Codex skills",
                crate::agent_controls::CatalogKind::Skills,
            ),
            (
                "mcp",
                "Inspect configured native MCP servers and tool counts",
                crate::agent_controls::CatalogKind::Mcp,
            ),
            (
                "plugins",
                "Inspect locally available native Codex plugins",
                crate::agent_controls::CatalogKind::Plugins,
            ),
            (
                "tools",
                "Inspect configured MCP tool names, descriptions, and inputs",
                crate::agent_controls::CatalogKind::Tools,
            ),
            (
                "hooks",
                "Inspect discovered lifecycle hooks without running them",
                crate::agent_controls::CatalogKind::Hooks,
            ),
            (
                "apps",
                "Inspect native connected apps and access state",
                crate::agent_controls::CatalogKind::Apps,
            ),
        ] {
            commands.push(Command {
                name,
                description,
                action: Action::Inventory(kind),
                native_equivalent: true,
            });
        }
    }
    let alias = match agent {
        AgentKind::Claude | AgentKind::Grok => Some((
            "cost",
            "Inspect CLI-reported costs and usage",
            Action::Context,
        )),
        AgentKind::Gemini => Some((
            "stats",
            "Inspect Gemini's reported statistics",
            Action::Context,
        )),
        AgentKind::OpenCode => Some((
            "models",
            "Choose an available provider/model",
            Action::Models,
        )),
        _ => None,
    };
    if let Some((name, description, action)) = alias {
        commands.push(Command {
            name,
            description,
            action,
            native_equivalent: agent != AgentKind::Grok,
        });
    }
    if matches!(
        agent,
        AgentKind::Claude
            | AgentKind::Gemini
            | AgentKind::OpenCode
            | AgentKind::Antigravity
            | AgentKind::Cursor
    ) {
        commands.push(Command {
            name: "plan",
            description: "Use this CLI's plan mode for the next turn",
            action: Action::ReadOnly,
            native_equivalent: agent != AgentKind::Antigravity,
        });
    }
    commands
}

/// Check only a short single-line command at the composer caret. Paths, code,
/// selected ranges and IME compositions retain normal text navigation.
pub fn query(text: &str) -> Option<&str> {
    if text.len() > 160 || !text.starts_with('/') || text.contains(['\n', '\r']) {
        return None;
    }
    let word = text[1..]
        .split_once(char::is_whitespace)
        .map_or(&text[1..], |(word, _)| word);
    (word.len() <= 64
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':')))
    .then_some(word)
}
/// The token touching the caret, bounded by whitespace. URLs, filesystem paths,
/// code fences and slash characters inside words are ordinary draft text.
pub fn span_at(text: &str, caret: usize) -> Option<std::ops::Range<usize>> {
    if !text.is_char_boundary(caret) { return None; }
    // Tab completion leaves a separating space; Enter still accepts that token.
    let caret=text[..caret].char_indices().next_back().filter(|(_,c)|*c==' ').map_or(caret,|(i,_)|i);
    let prefix=&text[..caret];
    let start=prefix.char_indices().rev().find(|(_,c)| c.is_whitespace()).map_or(0,|(i,c)|i+c.len_utf8());
    let end=text[caret..].char_indices().find(|(_,c)|c.is_whitespace()).map_or(text.len(),|(i,_)|caret+i);
    let token=&text[start..end];
    if !token.starts_with('/') || token.len()>65 || caret<=start {return None;}
    token[1..].bytes().all(|b|b.is_ascii_alphanumeric() || matches!(b,b'-'|b'_'|b':')).then_some(start..end)
}
/// Dispatch uses only the bounded first word; arguments may be multiline or
/// longer than the small popup query without bypassing native validation.
pub fn name(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    let end = rest
        .bytes()
        .take(65)
        .position(|b| b.is_ascii_whitespace())
        .unwrap_or(rest.len().min(65));
    let word = rest.get(..end)?;
    (!word.is_empty()
        && word.len() <= 64
        && word
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':')))
    .then_some(word)
}
pub fn catalog(agent: AgentKind) -> &'static [Command] {
    static CATALOG: std::sync::OnceLock<[Vec<Command>; 7]> = std::sync::OnceLock::new();
    let catalogs = CATALOG.get_or_init(|| {
        [
            AgentKind::Codex,
            AgentKind::Claude,
            AgentKind::Gemini,
            AgentKind::OpenCode,
            AgentKind::Antigravity,
            AgentKind::Cursor,
            AgentKind::Grok,
        ]
        .map(build_catalog)
    });
    &catalogs[match agent {
        AgentKind::Codex => 0,
        AgentKind::Claude => 1,
        AgentKind::Gemini => 2,
        AgentKind::OpenCode => 3,
        AgentKind::Antigravity => 4,
        AgentKind::Cursor => 5,
        AgentKind::Grok => 6,
    }]
}
pub fn ascii_contains(name: &str, query: &str) -> bool {
    query.is_empty()
        || name
            .as_bytes()
            .windows(query.len())
            .any(|window| window.eq_ignore_ascii_case(query.as_bytes()))
}
pub fn matching(agent: AgentKind, query: &str) -> Vec<Command> {
    let query = query.to_ascii_lowercase();
    let mut out: Vec<_> = catalog(agent)
        .iter()
        .copied()
        .filter(|c| c.name.contains(&query))
        .collect();
    out.sort_by_key(|c| (!c.name.starts_with(&query), !c.native_equivalent, c.name));
    out
}
pub fn lookup(agent: AgentKind, name: &str) -> Option<Command> {
    catalog(agent).iter().copied().find(|c| c.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inline_commands_follow_the_caret_without_matching_paths_or_urls(){
        let text="日本語 please /mod then continue";
        let caret=text.find(" then").unwrap();
        let span=span_at(text,caret).unwrap();assert_eq!(&text[span],"/mod");
        for text in ["https://host/path","please /tmp/file","please //comment","please word/model","please /a.rs"]{assert!(span_at(text,text.len()).is_none(),"{text}");}
        let text="long draft\nplease /";assert_eq!(&text[span_at(text,text.len()).unwrap()],"/");
    }
    #[test]
    fn each_provider_has_an_explicit_unique_bounded_catalog() {
        for agent in [
            AgentKind::Codex,
            AgentKind::Claude,
            AgentKind::Gemini,
            AgentKind::OpenCode,
            AgentKind::Antigravity,
            AgentKind::Cursor,
            AgentKind::Grok,
        ] {
            let commands = catalog(agent);
            let names: std::collections::HashSet<_> = commands.iter().map(|c| c.name).collect();
            assert_eq!(names.len(), commands.len());
            // Includes orchestration plus workflows, artifacts, and safe branching.
            assert!(commands.len() <= 50);
            assert_eq!(lookup(agent,"orchestrate").unwrap().action,Action::Orchestrate);
            assert_eq!(
                lookup(agent, "attach-chat").unwrap().action,
                Action::ChatContext
            );
            assert_eq!(lookup(agent, "team").unwrap().action, Action::Team);
            assert_eq!(lookup(agent, "shell").unwrap().action, Action::Shell);
            assert!(lookup(agent, "handoff").is_some());
            assert_eq!(
                lookup(agent, "delegate").is_some(),
                matches!(
                    agent,
                    AgentKind::Codex | AgentKind::Claude | AgentKind::OpenCode | AgentKind::Cursor
                )
            );
            assert!(
                lookup(agent, "compact").is_some() == (agent == AgentKind::Codex),
                "Only advertise native compaction where a genuine protocol bridge exists"
            );
        }
        assert!(lookup(AgentKind::Codex, "plan").is_none());
        assert!(lookup(AgentKind::Claude, "cost").is_some());
        assert!(lookup(AgentKind::Gemini, "stats").is_some());
        assert!(lookup(AgentKind::OpenCode, "models").is_some());
    }
    #[test]
    fn native_catalog_rejects_malformed_names_and_long_arguments_still_dispatch() {
        let values = serde_json::json!([
            "compact",
            "compact",
            "plugin:skill",
            "../escape",
            "/bad",
            "two words",
            "valid-skill",
            42
        ]);
        assert_eq!(
            advertised(&values),
            ["compact", "plugin:skill", "valid-skill"]
        );
        let many = serde_json::Value::Array(
            (0..1000)
                .map(|i| serde_json::Value::String(format!("skill-{i}")))
                .collect(),
        );
        assert_eq!(advertised(&many).len(), 128);
        let large = format!("/plugin:skill {}\nmore", "🦀".repeat(10000));
        assert_eq!(name(&large), Some("plugin:skill"));
        assert!(query(&large).is_none());
        assert!(name("/Users/me/file.rs").is_none());
    }

    #[test]
    fn command_detection_leaves_paths_multiline_unicode_and_large_text_alone() {
        for text in [
            "/Users/me",
            "//comment",
            "/a.rs",
            "/cmd\ncode",
            "hello /model",
            "/🦀",
        ] {
            assert!(query(text).is_none(), "{text}");
        }
        assert_eq!(query("/"), Some(""));
        assert_eq!(query("/model real-model"), Some("model"));
        assert_eq!(matching(AgentKind::Claude, "mod")[0].name, "model");
    }
}

mod agent_controls;
mod provider_defaults;
mod agent_picker;
mod appearance;
mod attachments;
mod background;
mod broker_ui;
mod chat_interactions;
mod chat_navigation;
mod checkpoint_diff;
mod checkpoint_ui;
mod checkpoints;
mod citations;
mod command_ui;
mod commands;
mod composer_controls;
mod context_refs;
mod context_ui;
mod diff_view;
mod discovery;
mod editor;
mod editor_ui;
mod engine;
mod file_context;
mod file_context_ui;
mod files;
mod global_hotkey;
mod handoff;
mod handoff_ui;
mod history_artifact;
mod input;
mod interaction_ui;
mod kai_icons;
mod kai_indicator;
mod loading_logo;
mod native;
mod native_request_ui;
#[cfg(test)]
mod personalization_tests;
mod personalization_ui;
mod popovers;
mod preferences;
mod presentation;
mod process_env;
mod project_lifecycle;
mod project_tree;
mod prompt_library;
mod prompt_library_ui;
mod prompt_queue;
mod provider_capabilities;
mod queue_ui;
mod quick_chat;
#[cfg(test)]
mod quick_chat_tests;
mod quick_surface;
mod response_buffer;
mod runtime;
mod sidebar_activity;
mod speech;
mod store;
#[path = "../assets/generated.rs"]
mod svg_assets;
mod syntax;
mod team_ui;
mod teams;
mod terminal;
mod terminal_ui;
mod theme;
mod theme_controls;
#[cfg(test)]
mod ui_tests;
mod unified;
mod text_decode;
mod workspace_motion;
mod usage;
mod voice;
mod voice_bridge;
mod voice_ui;
mod workspace_state;
mod workspace_ui;

use discovery::{Agent, AgentKind, Project, SessionSummary};
use engine::{Engine, Event};
use gpui::{prelude::*, *};
use input::{InputEvent, TextInput};
use presentation::{InlineStyle, MessageBlock};
use runtime::{RunHandle, RunRequest, RuntimeEvent};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};
use theme::{rgb, rgba};

const GLASS_HOVER: u32 = 0xebebeb14;
const GLASS_SURFACE: u32 = 0xebebeb0a;
const TEXT: u32 = 0xebebeb;
const MUTED: u32 = 0x929292;
const ACCENT: u32 = 0x459bf7;
actions!(
    orbit,
    [
        Quit,
        CloseWindow,
        MinimizeWindow,
        NewConversation,
        ToggleSearch,
        ToggleSidebar,
        ToggleFiles,
        ToggleConversations,
        ToggleSettings,
        ReturnToNotch,
        ToggleDiffs,
        Escape,
        Send
    ]
);

struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        let Some(data) = svg_assets::asset(path) else {
            return Ok(None);
        };
        Ok(Some(Cow::Borrowed(data)))
    }
    fn list(&self, _: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(vec![])
    }
}
fn icon(name: &'static str, size: f32, color: u32) -> Svg {
    let color = if matches!(
        name,
        "codex" | "claude" | "gemini" | "opencode" | "cursor" | "grok" | "antigravity"
    ) {
        theme::brand_rgb(color)
    } else {
        rgb(color)
    };
    svg().path(name).size(px(size)).text_color(color)
}
fn rich_text(
    id: impl Into<ElementId>,
    text: &str,
    highlights: &[(std::ops::Range<usize>, InlineStyle)],
    on_link: impl Fn(&str, &mut Window, &mut App) + 'static,
) -> InteractiveText {
    let links: Vec<_> = highlights
        .iter()
        .filter_map(|(range, style)| {
            if let InlineStyle::Link(url) = style {
                Some((range.clone(), url.clone()))
            } else {
                None
            }
        })
        .collect();
    let styled = StyledText::new(text.to_owned()).with_highlights(
        presentation::inline_runs(text.len(), highlights)
            .into_iter()
            .map(|(range, styles)| {
                let mut highlight = HighlightStyle::default();
                for style in &styles {
                    match style {
                        InlineStyle::Bold => highlight.font_weight = Some(FontWeight::SEMIBOLD),
                        InlineStyle::Italic => highlight.font_style = Some(FontStyle::Italic),
                        InlineStyle::Code => {
                            highlight.background_color = Some(rgba(0xffffff10).into());
                            highlight.color = Some(rgb(0xc5c9e8).into());
                        }
                        InlineStyle::Link(_) => {
                            highlight.color = Some(rgb(0x7dd3fc).into());
                            highlight.underline = Some(UnderlineStyle {
                                thickness: px(1.),
                                color: None,
                                wavy: false,
                            });
                        }
                        InlineStyle::Strike => {
                            highlight.strikethrough = Some(StrikethroughStyle {
                                thickness: px(1.),
                                color: None,
                            })
                        }
                    }
                }
                (range, highlight)
            }),
    );
    InteractiveText::new(id, styled).on_click(
        links.iter().map(|(r, _)| r.clone()).collect(),
        move |index, window, cx| {
            if let Some((_, url)) = links.get(index) {
                on_link(url, window, cx);
            }
        },
    )
}
fn activity_icon(label: &str) -> &'static str {
    kai_icons::icon_for_tool(label)
}
fn provider_asset(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Codex => "assets/providers/codex.svg",
        AgentKind::Claude => "assets/providers/claude.svg",
        AgentKind::Gemini => "assets/providers/gemini.svg",
        AgentKind::OpenCode => "assets/providers/opencode.svg",
        AgentKind::Antigravity => "assets/providers/antigravity.svg",
        AgentKind::Cursor => "assets/providers/cursor.svg",
        AgentKind::Grok => "assets/providers/grok.svg",
    }
}
fn activity_caption(status: &str) -> String {
    if let Some(id) = kai_icons::integration_for_label(status) {
        let name = match id {
            "googleslides" => "Google Slides",
            "googlesheets" => "Google Sheets",
            "googledocs" => "Google Docs",
            "googledrive" => "Google Drive",
            "googlecalendar" => "Google Calendar",
            "gmail" => "Gmail",
            "github" => "GitHub",
            "notion" => "Notion",
            "spotify" => "Spotify",
            "supabase" => "Supabase",
            "vercel" => "Vercel",
            "linear" => "Linear",
            "dropbox" => "Dropbox",
            "figma" => "Figma",
            "box" => "Box",
            "jira" => "Jira",
            "confluence" => "Confluence",
            "youtube" => "YouTube",
            "slack" => "Slack",
            "linkedin" => "LinkedIn",
            _ => "Connected tool",
        };
        let stage = match kai_icons::scene_for_status(status) {
            "failure" => "Needs attention",
            "declined" => "Stopped",
            "read" => "Reading results",
            _ => "Using",
        };
        format!("{stage} · {name}")
    } else {
        short(status, 64)
    }
}

fn merge_session_metadata(fresh: &mut SessionSummary, previous: &SessionSummary) {
    if previous.updated_at > fresh.updated_at {
        fresh.title = previous.title.clone();
        fresh.updated_at = previous.updated_at;
    }
    if previous.external_id.is_some() {
        fresh.external_id = previous.external_id.clone();
    }
    if fresh.execution_path.is_none() {
        fresh.execution_path = previous.execution_path.clone();
    }
}
fn imported_codex_needs_fork(session: &SessionSummary) -> bool {
    session.agent == AgentKind::Codex
        && session.transcript_path.is_some()
        && session
            .external_id
            .as_deref()
            .is_some_and(|native| session.id.strip_prefix("codex:") == Some(native))
}
fn short(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}
fn spring_animation() -> Animation {
    Animation::new(Duration::from_millis(650))
}
fn agent_color(agent: AgentKind) -> u32 {
    match agent {
        AgentKind::Claude => 0xd7ac91,
        AgentKind::Codex => 0xa39cfa,
        _ => 0xa7b8de,
    }
}
fn agent_icon(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Codex => "codex",
        AgentKind::Claude => "claude",
        AgentKind::Gemini => "gemini",
        AgentKind::OpenCode => "opencode",
        AgentKind::Antigravity => "antigravity",
        AgentKind::Cursor => "cursor",
        AgentKind::Grok => "grok",
    }
}
fn agent_name(agent: AgentKind) -> &'static str {
    agent.name()
}
// Provider identity travels with each assistant turn, so switching the active
// CLI never relabels earlier replies. Old canonical roles keep their fallback.
fn agent_for_role(role: Option<&str>, fallback: AgentKind) -> AgentKind {
    let Some(command) = role.and_then(|role| {
        role.strip_prefix("assistant:")
            .or_else(|| role.strip_prefix("continuation:"))
    }) else {
        return fallback;
    };
    [
        AgentKind::Codex,
        AgentKind::Claude,
        AgentKind::Gemini,
        AgentKind::OpenCode,
        AgentKind::Antigravity,
        AgentKind::Cursor,
        AgentKind::Grok,
    ]
    .into_iter()
    .find(|kind| kind.command() == command)
    .unwrap_or(fallback)
}

#[derive(Clone, Copy, PartialEq)]
enum Modal {
    Search,
    #[cfg(test)]
    Agents,
    Projects,
    Settings,
    Status,
    Permissions,
    Realtime,
    Activity,
    Handoff,
    Background,
    WorkspaceTrust,
    SessionConflict,
}
#[derive(Clone)]
struct WorkspaceTrust {
    session: String,
    agent: AgentKind,
    project: PathBuf,
}
enum MenuTarget {
    Project(Project),
    Chat(SessionSummary),
}
struct ActionMenu {
    target: MenuTarget,
    anchor: Point<Pixels>,
    started: Instant,
    closing: bool,
    closing_from: f32,
}
#[derive(Clone)]
struct Row {
    role: Option<String>,
    block: MessageBlock,
}
fn row_has_header(role: Option<&str>) -> bool {
    role.is_some_and(|role| !role.starts_with("continuation:"))
}
fn stamp_row_providers(rows: &mut [Row], fallback: AgentKind) {
    let mut assistant = None;
    for row in rows {
        match row.role.as_deref() {
            Some("assistant") => {
                assistant = Some(fallback);
                row.role = Some(format!("assistant:{}", fallback.command()));
            }
            Some(role) if role.starts_with("assistant:") => {
                assistant = Some(agent_for_role(Some(role), fallback));
            }
            Some(role) if role.starts_with("continuation:") => {}
            Some(_) => assistant = None,
            None => {
                if let Some(agent) = assistant {
                    row.role = Some(format!("continuation:{}", agent.command()));
                }
            }
        }
    }
}
struct Running {
    agent: AgentKind,
    submitted_prompt: String,
    handle: RunHandle,
    text: String,
    live_prefix_omitted: bool,
    status: String,
    stream_start: Option<usize>,
    revision: u64,
    parsed_revision: u64,
    parse_pending: bool,
    live_blocks: Vec<MessageBlock>,
    live_earlier: bool,
    parse_epoch: uuid::Uuid,
    retry: Option<RunRequest>,
    submitted_attachments: Vec<attachments::Attachment>,
}
fn append_live_text(run: &mut Running, text: &str) {
    const PREVIEW_BYTES: usize = 64 * 1024;
    if text.len() > PREVIEW_BYTES {
        let mut start = text.len() - PREVIEW_BYTES;
        while !text.is_char_boundary(start) {
            start += 1;
        }
        run.text.clear();
        run.text.push_str(&text[start..]);
        run.live_prefix_omitted = true;
    } else {
        let excess = run
            .text
            .len()
            .saturating_add(text.len())
            .saturating_sub(PREVIEW_BYTES);
        if excess > 0 {
            let mut start = excess.min(run.text.len());
            while !run.text.is_char_boundary(start) {
                start += 1;
            }
            run.text.drain(..start);
            run.live_prefix_omitted = true;
        }
        run.text.push_str(text);
    }
}
struct HistoryPage {
    number: usize,
    start: usize,
    len: usize,
    resident: bool,
    used: u64,
}
struct DeferredRow {
    page: usize,
    height: Pixels,
    checkpoint: Option<String>,
    user_boundary: bool,
}
type RevealRange = std::rc::Rc<std::cell::Cell<Option<(usize, usize)>>>;
struct ListReveal {
    started: Option<Instant>,
    only_from: usize,
    visible: RevealRange,
    from_paint: bool,
    #[cfg(test)]
    test_elapsed: Option<f32>,
}
impl ListReveal {
    const STAGGER: f32 = 0.028;
    const DURATION: f32 = 0.42;
    // Keep a single finite timeline, including on exceptionally tall displays.
    const LAST: usize = 31;
    fn new(first_visible: Option<usize>) -> Self {
        Self {
            started: None,
            only_from: 0,
            visible: std::rc::Rc::new(std::cell::Cell::new(
                first_visible.map(|first| (first, first)),
            )),
            from_paint: first_visible.is_none(),
            #[cfg(test)]
            test_elapsed: None,
        }
    }
    fn appended(start: usize) -> Self {
        Self {
            only_from: start,
            ..Self::new(None)
        }
    }
    fn observe(&mut self, range: std::ops::Range<usize>) {
        let range = range.start.max(self.only_from)..range.end;
        if self.from_paint || range.is_empty() {
            return;
        }
        Self::record(&self.visible, range.start, range.end - 1);
    }
    fn record(visible: &RevealRange, start: usize, end: usize) {
        let (first, last) = visible.get().unwrap_or((start, end));
        visible.set(Some((first.min(start), last.max(end))));
    }
    fn geometry_probe(&self, index: usize) -> Option<RevealRange> {
        // Only the first layout pass needs probes. List's measuring overdraw
        // must not count toward the visible text's stagger or animation budget.
        (self.from_paint && index >= self.only_from && self.visible.get().is_none())
            .then(|| self.visible.clone())
    }
    fn rank(&self, index: usize) -> usize {
        index
            .saturating_sub(self.visible.get().map_or(index, |(first, _)| first))
            .min(Self::LAST)
    }
    fn elapsed(&self) -> f32 {
        #[cfg(test)]
        if let Some(elapsed) = self.test_elapsed {
            return elapsed;
        }
        self.started
            .map_or(0., |started| started.elapsed().as_secs_f32())
    }
    fn duration(&self) -> f32 {
        let last = self.visible.get().map_or(0, |(_, last)| last);
        Self::DURATION + self.rank(last) as f32 * Self::STAGGER
    }
    fn row(&self, index: usize, elapsed: f32) -> (f32, f32) {
        if index < self.only_from {
            return (1., 0.);
        }
        let t =
            ((elapsed - self.rank(index) as f32 * Self::STAGGER) / Self::DURATION).clamp(0., 1.);
        // Opacity stays monotonic while position has a subtle spring overshoot.
        (1. - (1. - t).powi(3), 1. - presentation::spring(t))
    }
    fn advance(state: &mut Option<Self>, motion: bool, visible: bool, active: bool) -> bool {
        if !motion {
            *state = None;
        } else if visible
            && active
            && let Some(reveal) = state
        {
            reveal.started.get_or_insert_with(Instant::now);
            if reveal.elapsed() < reveal.duration() {
                return true;
            }
            *state = None;
        }
        false
    }
}
struct Orbit {
    unified_history_end:Option<(String,usize)>,
    unified_history_wait:bool,
    unified_back:Vec<SessionSummary>,
    unified_forward:Vec<SessionSummary>,
    unified_navigating:bool,
    unified_text_morph:bool,
    reasoning_efforts:std::collections::BTreeMap<String,String>,
    unified_children:Vec<runtime::broker::ChildSnapshot>,
    unified_text_decode:text_decode::TextDecode,
    unified_successes: std::collections::VecDeque<(String, String)>,
    unified_context: Option<String>,
    unified_voice_context: Option<String>,
    unified_voice_images:Vec<PathBuf>,
    voice_error:Option<String>,
    voice_runs:HashSet<(String,uuid::Uuid)>,
    unified_attachment_expected: Option<usize>,
    unified_notch_sources: Vec<PathBuf>,
    unified_attachment_sources: HashMap<PathBuf, uuid::Uuid>,
    activity: sidebar_activity::Activity,
    broker_panel: Option<broker_ui::Panel>,
    broker_refreshing: bool,
    broker_dirty: bool,
    broker_children: HashMap<String, usize>,
    broker_project: PathBuf,
    file_context_picker: Option<file_context_ui::Picker>,
    prompt_library_picker: Option<prompt_library_ui::Picker>,
    prompt_queue: Vec<prompt_queue::Prompt>,
    queue_dispatch: Option<String>,
    queue_editing: Option<queue_ui::Editing>,
    terminal_panel: Option<Entity<terminal_ui::TerminalPanel>>,
    terminal_visible: bool,
    terminal_subscription: Option<Subscription>,
    teams: Vec<teams::Team>,
    team_panel: Option<team_ui::Panel>,
    team_starting: HashSet<String>,
    context_picker: Option<context_ui::Picker>,
    context_pending: HashMap<uuid::Uuid, context_ui::Pending>,
    context_search_generation: u64,
    chat_context: HashMap<String, Vec<context_refs::Reference>>,
    native_pending: HashMap<String, native_request_ui::LiveRequest>,
    native_request_focus: Option<String>,
    native_models: HashMap<AgentKind, Vec<agent_controls::ModelChoice>>,
    native_commands: HashMap<(AgentKind, PathBuf), Vec<std::sync::Arc<str>>>,
    native_command_revision: u64,
    handoff_pending: Option<handoff_ui::Pending>,
    handoff_packets: HashMap<String, std::sync::Arc<handoff::Packet>>,
    settling: HashMap<String, async_channel::Receiver<()>>,
    slash: Option<command_ui::SlashPicker>,
    slash_dismissed: Option<String>,
    conversations_visible: bool,
    sidebar_split: bool,
    expanded_projects: HashSet<PathBuf>,
    project_tree: project_tree::ProjectTree,
    usage: HashMap<String, usage::Usage>,
    appearance: appearance::Appearance,
    theme: theme::Theme,
    resolved_theme: theme::ResolvedTheme,
    appearance_search: Option<Entity<TextInput>>,
    appearance_search_subscription: Option<Subscription>,
    appearance_editor: Option<personalization_ui::Editor>,
    quick_shortcut: global_hotkey::Shortcut,
    shortcut_recorder: Option<Entity<quick_chat::ShortcutRecorder>>,
    shortcut_subscription: Option<Subscription>,
    overlay_window: Option<WindowId>,
    primary_window: WindowId,
    voice_call: Option<voice_ui::Call>,
    voice_choice: String,
    workspace_store: Option<std::sync::Arc<workspace_state::StateStore>>,
    restore_pending: Option<workspace_state::WorkspaceState>,
    restore_scroll: Option<workspace_state::TranscriptAnchor>,
    restore_model: Option<(String, Option<String>)>,
    preferences_ready: bool,
    workspace_save: Option<Task<()>>,
    quit_pending: bool,
    workspace_signature: u64,
    saved_viewport: Option<(String, u64, usize)>,
    composer_revision: u64,
    following_bottom: bool,
    window_state: Option<workspace_state::WindowState>,
    background: Entity<background::ChatBackground>,
    live_indicator: Entity<kai_indicator::KaiIndicator>,
    loading_logo: Entity<loading_logo::LoadingLogo>,
    file_indicator: Entity<kai_indicator::KaiFileGlyph>,
    indicator_terminal: Option<(String, &'static str)>,
    engine: Engine,
    checkpoint_repository: Result<checkpoints::Repository, String>,
    checkpoint_runs: HashMap<String, String>,
    checkpoint_actions: HashMap<String, Option<checkpoints::Direction>>,
    checkpoint_action_loading: usize,
    checkpoint_busy: usize,
    checkpoint_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    checkpoint_timeline: Option<checkpoint_ui::Timeline>,
    preferences: preferences::Preferences,
    hidden_projects: HashSet<PathBuf>,
    restored_projects: HashSet<PathBuf>,
    source_projects: Vec<Project>,
    custom_projects: Vec<Project>,
    projects_revealed: bool,
    project_reveal: Option<ListReveal>,
    sessions_revealed: bool,
    sessions_scope: Option<(bool, PathBuf)>,
    sessions_reveal: Option<ListReveal>,
    transcript_reveal: Option<ListReveal>,
    action_menu: Option<ActionMenu>,
    agent_picker: Option<agent_picker::Picker>,
    controls: HashMap<&'static str, (Instant, agent_controls::ControlInfo)>,
    controls_pending: HashSet<&'static str>,
    controls_home: PathBuf,
    controls_cache: PathBuf,
    model: Option<String>,
    speech: Option<speech::SpeechHandle>,
    speech_generation: u64,
    dictation_hotkey_down: bool,
    dictation_status: Option<&'static str>,
    dictation_transition: Option<Instant>,
    attachments: Vec<attachments::Attachment>,
    attachment_drafts: HashMap<String, Vec<attachments::Attachment>>,
    attachment_dir: PathBuf,
    attachment_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    attachment_generation: u64,
    attachment_pending: usize,
    attachment_inserts: HashMap<uuid::Uuid, Instant>,
    attachment_removals: HashMap<uuid::Uuid, Instant>,
    attachment_hover: HashMap<uuid::Uuid, (Instant, bool, f32)>,
    attachment_preview: Option<(attachments::Attachment, Instant)>,
    attachment_preview_image: Option<std::sync::Arc<RenderImage>>,
    attachment_preview_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    session_models: HashMap<String, Option<String>>,
    agent_models: HashMap<&'static str, Option<String>>,
    new_execution_path: Option<PathBuf>,
    recently_removed: Option<Project>,
    hidden_sessions: HashSet<String>,
    restored_sessions: HashSet<String>,
    recently_deleted: Option<SessionSummary>,
    lifecycle: preferences::Lifecycle,
    lifecycle_editor: Option<project_lifecycle::Editor>,
    pending_project_creations: usize,
    archived_view: bool,
    archive_ids: Vec<String>,
    archive_generation: u64,
    archive_offset: usize,
    archive_page_end: usize,
    archive_loading: bool,
    file_browser: Option<Entity<files::FileBrowser>>,
    file_observer: Option<Subscription>,
    diff_observer: Option<Subscription>,
    files_visible: bool,
    diff_view: Option<Entity<diff_view::DiffView>>,
    diff_started: Option<Instant>,
    agents: Vec<Agent>,
    projects: Vec<Project>,
    sessions: Vec<SessionSummary>,
    visible_sessions: Vec<SessionSummary>,
    workspace_sessions: Vec<SessionSummary>,
    all_projects: bool,
    open_tabs: Vec<String>,
    drafts: HashMap<String, String>,
    send_after_load: Option<handoff_ui::Submission>,
    expanded_tools: HashSet<usize>,
    interaction_animations: HashMap<usize, (Instant, bool, f32)>,
    question_choices: HashMap<String, (String, Instant)>,
    project_generation: u64,
    project: PathBuf,
    agent: AgentKind,
    active: Option<String>,
    composer: Entity<TextInput>,
    search: Entity<TextInput>,
    focus: FocusHandle,
    modal: Option<Modal>,
    panel_popover: Option<popovers::PanelPopover>,
    panel_at_pointer_down: Option<Modal>,
    workspace_trust: Option<WorkspaceTrust>,
    session_conflict: Option<WorkspaceTrust>,
    session_conflict_open: bool,
    fork_next: Option<String>,
    workspace_trust_open: bool,
    workspace_retry: Option<WorkspaceTrust>,
    trusted_workspaces: std::collections::HashSet<(AgentKind, PathBuf)>,
    sidebar: bool,
    sidebar_transition: Option<(Instant, f32)>,
    sidebar_layout_transition: Option<(Instant, f32)>,
    sidebar_width: f32,
    allow_edits: bool,
    codex_approvals: runtime::CodexApprovals,
    starting: bool,
    pending_response_start: Option<usize>,
    rows: Vec<Row>,
    list: ListState,
    runs: HashMap<String, Running>,
    finalizing: HashSet<String>,
    generation: u64,
    search_generation: u64,
    loading: bool,
    loading_earlier: bool,
    has_older: bool,
    history_offset: usize,
    has_newer: bool,
    history_anchor: Option<ListOffset>,
    history_pages: Vec<HistoryPage>,
    deferred_rows: HashMap<usize, DeferredRow>,
    history_pending: HashSet<usize>,
    history_failed: HashSet<usize>,
    history_visible: std::ops::Range<usize>,
    navigation: Entity<chat_navigation::ScrollRail>,
    navigation_history: bool,
    navigation_previous: Option<bool>,
    history_clock: u64,
    history_animation: Option<Instant>,
    discovering: bool,
    notice: Option<String>,
    motion: bool,
    home_generation: u64,
    _subscriptions: Vec<Subscription>,
    #[cfg(test)]
    rendered_rows: std::cell::Cell<usize>,
}
impl Orbit {
    /// Window release can precede a background launch returning its handle.
    /// Cancel the tokens checked at that boundary as well as existing workers.
    fn cancel_pending_launches(&mut self) {
        self.checkpoint_cancel
            .store(true, std::sync::atomic::Ordering::Release);
        for team in &mut self.teams {
            for member in &mut team.members {
                member
                    .cancel
                    .store(true, std::sync::atomic::Ordering::Release);
                if member.status == teams::Status::Queued {
                    member.status = teams::Status::Held;
                }
            }
        }
        for run in self.runs.values() {
            run.handle.cancel();
        }
    }
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        workspace_store: Option<std::sync::Arc<workspace_state::StateStore>>,
    ) -> Self {
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let home = dirs::home_dir().unwrap_or(root.clone());
        let data = std::env::var_os("ORBIT_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("Library/Application Support/Orbit"));
        Self::with_state(window, cx, home, root, data, workspace_store)
    }
    #[cfg(test)]
    fn with_paths(
        window: &mut Window,
        cx: &mut Context<Self>,
        home: PathBuf,
        root: PathBuf,
        data: PathBuf,
    ) -> Self {
        Self::with_state(window, cx, home, root, data, None)
    }
    fn with_state(
        window: &mut Window,
        cx: &mut Context<Self>,
        home: PathBuf,
        root: PathBuf,
        data: PathBuf,
        workspace_store: Option<std::sync::Arc<workspace_state::StateStore>>,
    ) -> Self {
        let restored = workspace_store
            .as_ref()
            .map(|store| store.initial().clone());
        let saved_theme = restored
            .as_ref()
            .map(|s| s.theme.normalized())
            .unwrap_or_default();
        let saved_appearance = restored
            .as_ref()
            .map_or_else(Default::default, |s| s.appearance);
        let resolved_theme = saved_theme.resolve(is_dark(window.appearance()), saved_appearance);
        theme::install_resolved(resolved_theme.clone());
        native::apply_appearance(
            window,
            &appearance::Appearance {
                blur_radius: if resolved_theme.native_transparency {
                    saved_appearance.blur_radius
                } else {
                    0
                },
                ..saved_appearance
            },
        );
        native::apply_theme(window, &resolved_theme);
        let weak_appearance = cx.weak_entity();
        let system_appearance = window.observe_window_appearance(move |window, cx| {
            let _ = weak_appearance.update(cx, |this, cx| this.apply_personalization(window, cx));
        });
        let root = restored
            .as_ref()
            .and_then(|state| state.project.clone())
            .unwrap_or(root);
        let saved_sidebar = restored.as_ref().is_none_or(|state| state.sidebar);
        let saved_motion = restored.as_ref().is_none_or(|state| state.motion);
        let saved_sidebar_width = if restored.as_ref().is_some_and(|state| state.sidebar_split) {
            200.
        } else {
            280.
        };
        let saved_expanded_projects = restored
            .as_ref()
            .map(|state| state.expanded_projects.iter().cloned().collect())
            .unwrap_or_else(|| HashSet::from([root.clone()]));
        let engine = Engine::new(home.clone(), root.clone(), data.join("history.sqlite"));
        let preferences = preferences::Preferences::new(data.join("projects.json"));
        let background =
            cx.new(|cx| background::ChatBackground::new(data.clone(), root.clone(), window, cx));
        let background_changes = cx.subscribe(&background, |this, _, _, cx| {
            if this.modal == Some(Modal::Background) {
                cx.notify();
            }
        });
        let live_indicator = cx.new(kai_indicator::KaiIndicator::new);
        let loading_logo = cx.new(loading_logo::LoadingLogo::new);
        let logo_settled = cx.subscribe(&loading_logo, |_, _, _: &loading_logo::Settled, cx| {
            cx.notify()
        });
        let navigation = cx.new(chat_navigation::ScrollRail::new);
        let navigation_seek =
            cx.subscribe(&navigation, |this, _, event: &chat_navigation::Seek, cx| {
                this.seek_chat(event.0, cx);
            });
        let file_indicator = cx.new(kai_indicator::KaiFileGlyph::new);
        let activation = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.overlay_window = Some(window.window_handle().window_id());
            }
            if !window.is_window_active() {
                this.dictation_hotkey_down = false;
            }
            cx.notify();
        });
        let bounds_changed = cx.observe_window_bounds(window, |this, window, cx| {
            this.window_state = Some(workspace_ui::capture_window(window));
            this.queue_workspace_save(cx);
        });
        let before_quit = cx.on_app_quit(|this, cx| {
            this.stop_dictation(cx);
            this.stop_voice(cx);
            this.quit_pending = true;
            this.background
                .update(cx, |background, cx| background.set_quitting(true, cx));
            this.cancel_pending_launches();
            drop(this.shutdown_app_brokers());
            // Normal Quit waits for protocol cleanup in quit_after_save. The
            // short platform hook also starts cancellation for system shutdown.
            this.save_workspace_now(cx);
            let flush = this.workspace_store.as_ref().map(|store| {
                let acknowledgement = store.flush();
                let history_acknowledgement = this.engine.flush();
                cx.background_executor().spawn(async move {
                    let _ = history_acknowledgement.recv_timeout(Duration::from_secs(2));
                    if let Ok(Err(error)) = acknowledgement.recv_timeout(Duration::from_secs(2)) {
                        eprintln!("Could not save Code workspace: {error}");
                    }
                })
            });
            async move {
                if let Some(flush) = flush {
                    flush.await;
                }
            }
        });
        cx.on_release(|this, cx| {
            if let Some(mut call) = this.voice_call.take() {
                call.media.stop();
                if let Some(run) = this.runs.get(&call.owner) {
                    run.handle.cancel();
                }
            }
            this.cancel_pending_launches();
            drop(this.shutdown_brokers());
            if let Ok(repository) = &this.checkpoint_repository {
                for (session, id) in this.checkpoint_runs.drain() {
                    if let Some(run) = this.runs.get(&session) {
                        drop(repository.cleanup_on_orphan(id, run.handle.settled.clone()));
                    }
                }
            }
            this.save_workspace_now(cx);
        })
        .detach();
        #[cfg(not(test))]
        {
            let wake = runtime::broker::wake();
            cx.spawn(async move |this, cx| {
                while wake.recv().await.is_ok() {
                    Timer::after(Duration::from_millis(16)).await;
                    if this.update(cx, |this, cx| this.refresh_broker(cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
            let events = engine.events.clone();
            cx.spawn(async move |this, cx| {
                while let Ok(event) = events.recv().await {
                    if this.update(cx, |this, cx| this.receive(event, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
            let preference_events = preferences.events.clone();
            cx.spawn(async move |this, cx| {
                while let Ok(event) = preference_events.recv().await {
                    if this
                        .update(cx, |this, cx| this.receive_preferences(event, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        let composer = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Ask, build, or explore this project…", cx);
            input.set_image_paste(true);
            input
        });
        let search = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Search conversations or saved messages…", cx);
            input
        });
        composer.read(cx).focus_handle.focus(window);
        let submit = cx.subscribe_in(&composer, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::Submit) {
                this.send(window, cx);
            }
            if matches!(
                event,
                InputEvent::DictationCancelled | InputEvent::LimitReached
            ) {
                this.stop_dictation(cx);
            }
            if matches!(event, InputEvent::LimitReached) {
                this.notice =
                    Some("Keep the prompt under 64 KB. Reference large files by path.".into());
                cx.notify();
            }
            if matches!(event, InputEvent::Changed) {
                this.composer_revision = this.composer_revision.wrapping_add(1);
                cx.notify();
                this.queue_workspace_save(cx);
                this.update_slash(window, cx);
            }
            match event {
                InputEvent::ImagePasted(bytes) => this.add_clipboard_image(bytes.clone(), cx),
                InputEvent::ClipboardError(error) => {
                    this.notice = Some(error.clone());
                    cx.notify();
                }
                InputEvent::CursorChanged => this.update_slash(window, cx),
                InputEvent::CommandPrevious => this.move_slash(-1, cx),
                InputEvent::CommandNext => this.move_slash(1, cx),
                InputEvent::CommandAccept => this.accept_slash(false, window, cx),
                InputEvent::CommandComplete => this.accept_slash(true, window, cx),
                _ => {}
            }
        });
        let search_change =
            cx.subscribe_in(&search, window, |this, _, event, window, cx| match event {
                InputEvent::Submit => {
                    if let Some(session) = this.visible_sessions.first().cloned() {
                        this.open_session(session, window, cx);
                    }
                }
                InputEvent::Changed => {
                    this.search_generation += 1;
                    let generation = this.search_generation;
                    cx.spawn(async move |this, cx| {
                        Timer::after(Duration::from_millis(100)).await;
                        let _ = this.update(cx, |this, cx| {
                            if this.search_generation == generation
                                && this.modal == Some(Modal::Search)
                            {
                                this.engine.search(this.search.read(cx).text(), generation);
                            }
                        });
                    })
                    .detach();
                }
                _ => {}
            });
        let list = ListState::new(0, ListAlignment::Bottom, px(400.));
        list.set_scroll_handler(cx.listener(|this, event: &ListScrollEvent, _, cx| {
            // A real scroll ends the entrance immediately; changing the visible
            // tail must never make already readable text fade out again.
            this.transcript_reveal = None;
            this.following_bottom = !event.is_scrolled && !this.navigation_history;
            let visible_count = event.visible_range.len();
            let is_scrolled = event.is_scrolled;
            let generation = this.generation;
            let weak = cx.weak_entity();
            // GPUI invokes this handler while its list is mutably borrowed.
            // Defer reading the final offset until scrolling has completed.
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    if generation != this.generation {
                        return;
                    }
                    let start = this.list.logical_scroll_top().item_ix;
                    this.history_visible =
                        start..start.saturating_add(visible_count + 2).min(this.rows.len());
                    this.rehydrate_visible_history(cx);
                    this.queue_workspace_save(cx);
                    this.navigation
                        .update(cx, |rail, cx| rail.scrolled(start, this.rows.len(), cx));
                    if is_scrolled && start < 3 {
                        this.load_history_page(true, cx);
                    }
                });
            });
        }));
        Self {
            unified_text_morph:restored.as_ref().is_none_or(|s|s.text_morph),
            unified_history_end:None,unified_history_wait:false,
            unified_back:vec![],unified_forward:vec![],unified_navigating:false,
            reasoning_efforts:restored.as_ref().map(|s|s.reasoning_efforts.clone()).unwrap_or_default(),
            unified_children:vec![],
            unified_text_decode:Default::default(),
            unified_successes: Default::default(),
            unified_context: None,
            unified_voice_context: None,
            unified_voice_images:vec![],voice_error:None,voice_runs:HashSet::new(),
            unified_attachment_expected: None,
            unified_notch_sources: vec![],
            unified_attachment_sources: HashMap::new(),
            broker_panel: None,
            broker_refreshing: false,
            broker_dirty: false,
            broker_children: HashMap::new(),
            broker_project: PathBuf::new(),
            file_context_picker: None,
            prompt_library_picker: None,
            prompt_queue: Vec::new(),
            queue_dispatch: None,
            queue_editing: None,
            terminal_panel: None,
            terminal_visible: false,
            terminal_subscription: None,
            teams: Vec::new(),
            team_panel: None,
            team_starting: HashSet::new(),
            context_picker: None,
            context_pending: HashMap::new(),
            context_search_generation: 0,
            chat_context: HashMap::new(),
            native_pending: HashMap::new(),
            native_request_focus: None,
            native_models: HashMap::new(),
            native_commands: HashMap::new(),
            native_command_revision: 0,
            handoff_pending: None,
            handoff_packets: HashMap::new(),
            settling: HashMap::new(),
            slash: None,
            slash_dismissed: None,
            sidebar_split: restored.as_ref().is_some_and(|state| state.sidebar_split),
            expanded_projects: saved_expanded_projects,
            project_tree: project_tree::ProjectTree::default(),
            conversations_visible: restored
                .as_ref()
                .is_none_or(|state| state.conversations_visible),
            usage: HashMap::new(),
            appearance: restored
                .as_ref()
                .map_or_else(Default::default, |state| state.appearance),
            theme: saved_theme,
            resolved_theme,
            appearance_search: None,
            appearance_search_subscription: None,
            appearance_editor: None,
            quick_shortcut: restored
                .as_ref()
                .map(|s| s.quick_shortcut.clone())
                .unwrap_or_default(),
            shortcut_recorder: None,
            shortcut_subscription: None,
            overlay_window: None,
            primary_window: window.window_handle().window_id(),
            voice_call: None,
            activity: sidebar_activity::Activity::default(),
            voice_choice: restored
                .as_ref()
                .map(|s| s.voice_choice.clone())
                .unwrap_or_else(|| "cove".into()),
            workspace_store,
            restore_pending: restored,
            restore_scroll: None,
            restore_model: None,
            preferences_ready: cfg!(test),
            workspace_save: None,
            quit_pending: false,
            workspace_signature: 0,
            saved_viewport: None,
            composer_revision: 0,
            following_bottom: true,
            window_state: Some(workspace_ui::capture_window(window)),
            background,
            live_indicator,
            loading_logo,
            file_indicator,
            indicator_terminal: None,
            engine,
            checkpoint_repository: Ok(checkpoints::Repository::at(data.join("checkpoints"))),
            checkpoint_runs: HashMap::new(),
            checkpoint_actions: HashMap::new(),
            checkpoint_action_loading: 0,
            checkpoint_busy: 0,
            checkpoint_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            checkpoint_timeline: None,
            preferences,
            hidden_projects: HashSet::new(),
            restored_projects: HashSet::new(),
            source_projects: vec![],
            custom_projects: vec![],
            projects_revealed: false,
            project_reveal: None,
            sessions_revealed: false,
            sessions_scope: None,
            sessions_reveal: None,
            transcript_reveal: None,
            action_menu: None,
            agent_picker: None,
            controls: HashMap::new(),
            controls_pending: HashSet::new(),
            controls_home: home,
            controls_cache: data.join("login"),
            model: None,
            speech: None,
            speech_generation: 0,
            dictation_hotkey_down: false,
            dictation_status: None,
            dictation_transition: None,
            attachments: vec![],
            attachment_drafts: HashMap::new(),
            attachment_dir: data.join("attachments"),
            attachment_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            attachment_generation: 0,
            attachment_pending: 0,
            attachment_inserts: HashMap::new(),
            attachment_removals: HashMap::new(),
            attachment_hover: HashMap::new(),
            attachment_preview: None,
            attachment_preview_image: None,
            attachment_preview_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                false,
            )),
            session_models: HashMap::new(),
            agent_models: HashMap::new(),
            new_execution_path: None,
            recently_removed: None,
            hidden_sessions: HashSet::new(),
            restored_sessions: HashSet::new(),
            recently_deleted: None,
            lifecycle: Default::default(),
            lifecycle_editor: None,
            pending_project_creations: 0,
            archived_view: false,
            archive_ids: vec![],
            archive_generation: 0,
            archive_offset: 0,
            archive_page_end: 0,
            archive_loading: false,
            file_browser: None,
            file_observer: None,
            diff_observer: None,
            files_visible: false,
            diff_view: None,
            diff_started: None,
            agents: vec![],
            projects: vec![],
            sessions: vec![],
            visible_sessions: vec![],
            workspace_sessions: vec![],
            all_projects: false,
            open_tabs: vec![],
            drafts: HashMap::new(),
            send_after_load: None,
            expanded_tools: HashSet::new(),
            interaction_animations: HashMap::new(),
            question_choices: HashMap::new(),
            project_generation: 0,
            project: root,
            agent: AgentKind::Codex,
            active: None,
            composer,
            search,
            focus: cx.focus_handle(),
            modal: None,
            panel_popover: None,
            panel_at_pointer_down: None,
            workspace_trust: None,
            session_conflict: None,
            session_conflict_open: false,
            fork_next: None,
            workspace_trust_open: false,
            workspace_retry: None,
            trusted_workspaces: std::collections::HashSet::new(),
            sidebar: saved_sidebar,
            sidebar_transition: None,
            sidebar_layout_transition: None,
            sidebar_width: if saved_sidebar {
                saved_sidebar_width
            } else {
                0.
            },
            allow_edits: false,
            codex_approvals: runtime::CodexApprovals::default(),
            starting: false,
            pending_response_start: None,
            rows: vec![],
            list,
            runs: HashMap::new(),
            finalizing: HashSet::new(),
            generation: 0,
            search_generation: 0,
            loading: false,
            loading_earlier: false,
            has_older: false,
            history_offset: 0,
            has_newer: false,
            history_anchor: None,
            history_pages: vec![],
            deferred_rows: HashMap::new(),
            history_pending: HashSet::new(),
            history_failed: HashSet::new(),
            history_visible: 0..0,
            navigation,
            navigation_history: false,
            navigation_previous: None,
            history_clock: 0,
            history_animation: None,
            discovering: true,
            notice: None,
            motion: saved_motion && std::env::var_os("ORBIT_REDUCE_MOTION").is_none(),
            home_generation: 0,
            _subscriptions: vec![
                logo_settled,
                navigation_seek,
                submit,
                search_change,
                background_changes,
                activation,
                bounds_changed,
                system_appearance,
                before_quit,
            ],
            #[cfg(test)]
            rendered_rows: std::cell::Cell::new(0),
        }
    }
    fn open_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.quit_pending {
            return;
        }
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open workspace".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = selection.await
                && let Some(path) = paths.first()
            {
                let path = path.clone();
                let _ = this.update_in(cx, |this, window, cx| {
                    if this.quit_pending {
                        return;
                    }
                    this.hidden_projects.remove(&path);
                    this.restored_projects.insert(path.clone());
                    let project = Project {
                        name: path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                        path: path.clone(),
                    };
                    this.preferences.add(project.clone());
                    this.custom_projects.retain(|p| p.path != path);
                    this.custom_projects.push(project);
                    if !this.projects.iter().any(|p| p.path == path) {
                        this.projects.push(Project {
                            name: path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                            path: path.clone(),
                        });
                    }
                    this.select_project(path, window, cx);
                });
            }
        })
        .detach();
    }
    fn rebuild_workspace_sessions(&mut self) {
        if self.sessions_scope.as_ref().is_none_or(|(all, project)| {
            *all != self.all_projects || (!self.all_projects && *project != self.project)
        }) {
            self.sessions_scope = Some((self.all_projects, self.project.clone()));
            self.sessions_revealed = false;
            self.sessions_reveal = None;
        }
        self.workspace_sessions = self
            .sessions
            .iter()
            .filter(|session| {
                !self.hidden_sessions.contains(&session.id)
                    && self.lifecycle.archived_chats.contains(&session.id) == self.archived_view
                    && (self.all_projects || session.project_path.starts_with(&self.project))
            })
            .cloned()
            .collect();
        self.workspace_sessions.sort_by_key(|session| {
            (
                std::cmp::Reverse(if self.all_projects {
                    self.activity.opened_rank(&session.id)
                } else {
                    0
                }),
                !self.lifecycle.pinned_chats.contains(&session.id),
                std::cmp::Reverse(session.updated_at),
            )
        });
        self.rebuild_project_tree();
    }
    fn rebuild_projects(&mut self) {
        let mut seen = HashSet::new();
        self.projects = self
            .source_projects
            .iter()
            .chain(&self.custom_projects)
            .filter(|p| !self.hidden_projects.contains(&p.path) && seen.insert(p.path.clone()))
            .cloned()
            .collect();
        self.projects
            .sort_by_key(|project| !self.lifecycle.pinned_projects.contains(&project.path));
        self.rebuild_project_tree();
    }
    fn tick_list_reveals(&mut self, window: &Window, cx: &Context<Self>) {
        let active = window.is_window_active();
        let sessions_visible = self.sidebar_split
            && self.conversations_visible
            && (!(self.files_visible || self.diff_view.is_some())
                || window.viewport_size().width >= px(1180.));
        if !self.projects_revealed
            && !self.projects.is_empty()
            && (!self.motion || (self.sidebar && active))
        {
            self.projects_revealed = true;
            self.project_reveal = self.motion.then(|| ListReveal::new(Some(0)));
        }
        if !self.sessions_revealed
            && !self.workspace_sessions.is_empty()
            && (!self.motion || (sessions_visible && active))
        {
            self.sessions_revealed = true;
            self.sessions_reveal = self.motion.then(|| ListReveal::new(Some(0)));
        }
        let mut animating = false;
        for (reveal, visible) in [
            (&mut self.project_reveal, self.sidebar),
            (&mut self.sessions_reveal, sessions_visible),
            (
                &mut self.transcript_reveal,
                self.active.is_some() && !self.loading,
            ),
        ] {
            animating |= ListReveal::advance(reveal, self.motion, visible, active);
        }
        if animating {
            request_motion_frame(window, cx);
        }
    }
    fn receive_preferences(&mut self, event: preferences::Event, cx: &mut Context<Self>) {
        match event {
            preferences::Event::Loaded(hidden, custom, hidden_sessions, mut lifecycle) => {
                // Edits made while the preferences worker starts win over its
                // initial snapshot; discovered metadata never owns these names.
                let edited_before_load = !self.lifecycle.project_names.is_empty()
                    || !self.lifecycle.chat_names.is_empty()
                    || !self.lifecycle.pinned_projects.is_empty()
                    || !self.lifecycle.pinned_chats.is_empty()
                    || !self.lifecycle.archived_chats.is_empty();
                lifecycle
                    .project_names
                    .extend(self.lifecycle.project_names.drain());
                lifecycle
                    .chat_names
                    .extend(self.lifecycle.chat_names.drain());
                lifecycle
                    .pinned_projects
                    .extend(self.lifecycle.pinned_projects.drain());
                lifecycle
                    .pinned_chats
                    .extend(self.lifecycle.pinned_chats.drain());
                lifecycle
                    .archived_chats
                    .extend(self.lifecycle.archived_chats.drain());
                self.lifecycle = *lifecycle;
                self.refresh_archive_index();
                if self.archived_view {
                    self.load_archived_page(cx);
                }
                self.preferences_ready = true;
                if edited_before_load {
                    self.preferences.save_lifecycle(self.lifecycle.clone());
                }
                self.hidden_sessions.extend(
                    hidden_sessions
                        .into_iter()
                        .filter(|id| !self.restored_sessions.contains(id)),
                );
                self.visible_sessions
                    .retain(|session| !self.hidden_sessions.contains(&session.id));
                self.open_tabs
                    .retain(|id| !self.hidden_sessions.contains(id));
                if self
                    .active
                    .as_ref()
                    .is_some_and(|id| self.hidden_sessions.contains(id))
                {
                    self.reset_conversation(cx);
                }
                self.rebuild_workspace_sessions();
                self.hidden_projects.extend(
                    hidden
                        .into_iter()
                        .filter(|p| !self.restored_projects.contains(p)),
                );
                for project in custom {
                    if !self.hidden_projects.contains(&project.path)
                        && !self.custom_projects.iter().any(|p| p.path == project.path)
                    {
                        self.custom_projects.push(project);
                    }
                }
                self.rebuild_projects();
            }
            preferences::Event::Error(error) => self.notice = Some(error),
        }
        cx.notify();
    }
    fn remove_project(&mut self, project: Project, cx: &mut Context<Self>) {
        self.hidden_projects.insert(project.path.clone());
        self.restored_projects.remove(&project.path);
        self.custom_projects.retain(|p| p.path != project.path);
        self.projects.retain(|p| p.path != project.path);
        self.preferences.remove(project.path.clone());
        self.notice = Some(format!(
            "Removed {} from Code. Files and agent chats are kept.",
            project.name
        ));
        self.recently_removed = Some(project);
        self.recently_deleted = None;
        self.modal = None;
        self.dismiss_action_menu();
        self.all_projects = true;
        self.rebuild_workspace_sessions();
        cx.notify();
    }
    fn restore_project(&mut self, cx: &mut Context<Self>) {
        if let Some(project) = self.recently_removed.take() {
            self.hidden_projects.remove(&project.path);
            self.restored_projects.insert(project.path.clone());
            self.preferences.add(project.clone());
            self.custom_projects.retain(|p| p.path != project.path);
            self.custom_projects.push(project.clone());
            if !self.projects.iter().any(|p| p.path == project.path) {
                self.projects.push(project);
            }
            self.notice = None;
            cx.notify();
        }
    }
    fn dismiss_action_menu(&mut self) {
        if !self.motion {
            self.action_menu = None;
        } else if let Some(menu) = self.action_menu.as_mut()
            && !menu.closing
        {
            menu.closing_from =
                presentation::spring((menu.started.elapsed().as_secs_f32() / 0.32).min(1.))
                    .clamp(0., 1.);
            menu.closing = true;
            menu.started = Instant::now();
        }
    }
    fn can_delete_chat(&self, id: &str) -> bool {
        !(self.hidden_sessions.contains(id)
            || self.runs.contains_key(id)
            || self.finalizing.contains(id)
            || (self.starting && self.active.as_deref() == Some(id)))
    }
    fn delete_chat(
        &mut self,
        session: SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_delete_chat(&session.id) {
            return;
        }
        let closing_menu = self.action_menu.take();
        self.hidden_sessions.insert(session.id.clone());
        self.restored_sessions.remove(&session.id);
        self.preferences.hide_session(session.id.clone());
        self.close_tab(&session.id, window, cx);
        self.visible_sessions.retain(|item| item.id != session.id);
        self.rebuild_workspace_sessions();
        self.recently_deleted = Some(session);
        self.recently_removed = None;
        self.action_menu = closing_menu;
        self.dismiss_action_menu();
        self.notice = Some("Chat deleted from Code · original agent session kept".into());
        cx.notify();
    }
    fn restore_chat(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = self.recently_deleted.take() {
            self.hidden_sessions.remove(&session.id);
            self.restored_sessions.insert(session.id.clone());
            self.preferences.restore_session(session.id.clone());
            if !self.sessions.iter().any(|item| item.id == session.id) {
                self.sessions.push(session);
                self.sessions
                    .sort_by_key(|s| std::cmp::Reverse(s.updated_at));
            }
            self.visible_sessions = self
                .sessions
                .iter()
                .filter(|session| !self.hidden_sessions.contains(&session.id))
                .cloned()
                .collect();
            self.rebuild_workspace_sessions();
            self.notice = None;
            cx.notify();
        }
    }
    fn action_menu(&self, menu: &ActionMenu, cx: &Context<Self>) -> impl IntoElement {
        let progress = if !self.motion {
            1.
        } else {
            let duration = if menu.closing { 0.12 } else { 0.32 };
            (menu.started.elapsed().as_secs_f32() / duration).min(1.)
        };
        let reveal = if menu.closing {
            menu.closing_from * (1. - progress * progress)
        } else {
            presentation::spring(progress)
        };
        let (target, selector, action_id, label, enabled) = match &menu.target {
            MenuTarget::Project(project) => (
                MenuTarget::Project(project.clone()),
                "project-action-menu",
                "remove-project",
                "Remove project",
                true,
            ),
            MenuTarget::Chat(session) => (
                MenuTarget::Chat(session.clone()),
                "thread-action-menu",
                "delete-chat",
                "Delete chat",
                self.can_delete_chat(&session.id),
            ),
        };
        anchored()
            .position(menu.anchor + point(px(-6. * (1. - reveal)), px(0.)))
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id(selector)
                    .debug_selector(move || selector.into())
                    .w(px(160.))
                    .p(px(4.))
                    .rounded_lg()
                    .bg(rgba(appearance::MENU_SURFACE))
                    .border_1()
                    .border_color(rgba(0xebebeb18))
                    .opacity(reveal.clamp(0., 1.))
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.dismiss_action_menu();
                        cx.notify();
                    }))
                    .child(self.lifecycle_menu_actions(&menu.target, menu.anchor, menu.closing, cx))
                    .child(
                        div()
                            .id(action_id)
                            .debug_selector(move || action_id.into())
                            .h(px(30.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .text_size(theme::ui_px(12.))
                            .text_color(rgb(if enabled { 0xf2a6a6 } else { MUTED }))
                            .when(!menu.closing && enabled, |s| {
                                s.cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| match &target {
                                            MenuTarget::Project(project) => {
                                                this.remove_project(project.clone(), cx)
                                            }
                                            MenuTarget::Chat(session) => {
                                                this.delete_chat(session.clone(), window, cx)
                                            }
                                        },
                                    ))
                            })
                            .child(icon("trash", 13., if enabled { 0xf2a6a6 } else { MUTED }))
                            .child(label),
                    ),
            )
    }
    fn toggle_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.broker_panel = None;
        self.diff_view = None;
        self.diff_observer = None;
        self.diff_started = None;
        self.files_visible = !self.files_visible;
        if self.files_visible {
            self.sync_file_root(window, cx);
        } else if let Some(browser) = &self.file_browser {
            browser.update(cx, |browser, cx| browser.set_visible(false, cx));
            self.composer.read(cx).focus_handle.focus(window);
        }
        cx.notify();
    }
    fn open_changes(&mut self, cx: &mut Context<Self>) {
        self.broker_panel = None;
        let root = self
            .execution_root()
            .unwrap_or_else(|| self.project.clone());
        self.files_visible = false;
        if let Some(browser) = &self.file_browser {
            browser.update(cx, |browser, cx| browser.set_visible(false, cx));
        }
        let review = cx.new(|cx| diff_view::DiffView::new(root, cx));
        review.update(cx, |review, cx| review.set_appearance(self.appearance, cx));
        self.diff_observer = Some(cx.observe(&review, |this, _, cx| this.queue_workspace_save(cx)));
        self.diff_view = Some(review);
        self.diff_started = Some(Instant::now());
        cx.notify();
    }
    fn sync_file_root(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.files_visible {
            self.broker_panel = None;
        }
        if !self.files_visible {
            return;
        }
        let root = self
            .execution_root()
            .unwrap_or_else(|| self.project.clone());
        if let Some(browser) = &self.file_browser {
            browser.update(cx, |browser, cx| {
                browser.set_root(root, window, cx);
                browser.set_visible(true, cx);
            });
        } else {
            let browser = cx.new(|cx| files::FileBrowser::new(root, window, cx));
            browser.update(cx, |browser, cx| {
                browser.set_appearance(self.appearance, cx)
            });
            self.file_observer =
                Some(cx.observe(&browser, |this, _, cx| this.queue_workspace_save(cx)));
            self.file_browser = Some(browser);
        }
    }
    fn select_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.project != path {
            self.stop_voice(cx);
        }
        self.all_projects = false;
        self.new_execution_path = None;
        self.project = path;
        self.project_generation += 1;
        self.engine
            .load_project(self.project.clone(), self.project_generation);
        self.rebuild_workspace_sessions();
        if let Some(session) = self.workspace_sessions.first().cloned() {
            self.open_session(session, window, cx);
        } else {
            self.new_conversation(window, cx);
            self.new_execution_path = None;
        }
        self.sync_file_root(window, cx);
    }
    fn toggle_sidebar(&mut self) {
        self.dismiss_action_menu();
        self.sidebar = !self.sidebar;
        self.sidebar_transition = Some((Instant::now(), self.sidebar_width));
    }
    fn receive(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::SidebarProject(path, token, sessions) => {
                self.receive_project_branch(path, token, Ok(sessions), cx);
            }
            Event::SidebarProjectError(path, token, error) => {
                self.receive_project_branch(path, token, Err(error), cx);
            }
            Event::Metadata(sessions) => {
                for session in sessions {
                    if !self.sessions.iter().any(|current| current.id == session.id) {
                        self.sessions.push(session);
                    }
                }
                self.rebuild_workspace_sessions();
            }
            Event::MetadataError(error) => {
                self.notice = Some(format!("Could not recover saved task metadata: {error}"))
            }
            Event::ContextSearch(generation, results) => {
                if let Some(picker) = &mut self.context_picker
                    && picker.generation == generation
                {
                    picker.results = results;
                }
            }
            Event::Handoff(token, result) => {
                if self.context_pending.contains_key(&token) {
                    self.receive_context(token, result, cx);
                    cx.notify();
                    return;
                }
                if self
                    .handoff_pending
                    .as_ref()
                    .is_some_and(|p| p.token == token)
                {
                    match result {
                        Ok(packet) => self.handoff_pending.as_mut().unwrap().ready = Some(packet),
                        Err(error) => {
                            if let Some(pending) = self.handoff_pending.take() {
                                self.hold_queue(&pending.source.id, cx);
                            }
                            self.notice = Some(format!(
                                "Could not transfer context: {error}. Your draft is preserved."
                            ));
                        }
                    }
                }
            }
            Event::AgentSwitched(token, result) => self.handoff_bound(token, result, cx),

            Event::Model(id, generation, model, usage, packet) => {
                if self.active.as_deref() == Some(&id) && self.generation == generation {
                    if let Some(usage) = usage
                        && !(self.usage.contains_key(&id)
                            && (self.runs.contains_key(&id)
                                || self.finalizing.contains(&id)
                                || self.settling.contains_key(&id)
                                || self
                                    .broker_children
                                    .get(&id)
                                    .is_some_and(|count| *count > 0)))
                    {
                        self.remember_usage(id.clone(), *usage);
                    }
                    if let Some(packet) = packet {
                        self.remember_handoff(id.clone(), std::sync::Arc::new(packet));
                    }
                    if self
                        .restore_model
                        .as_ref()
                        .is_some_and(|(restore, _)| restore == &id)
                    {
                        let (_, restored) = self.restore_model.take().unwrap();
                        if self
                            .sessions
                            .iter()
                            .any(|session| session.id == id && session.agent == self.agent)
                        {
                            self.model = restored.clone();
                        }
                        self.session_models.insert(id, restored);
                        return;
                    }
                    if self
                        .sessions
                        .iter()
                        .any(|session| session.id == id && session.agent == self.agent)
                    {
                        self.model = model.clone();
                    }
                    self.session_models.insert(id, model);
                }
            }
            Event::Ready(discovery, sessions, preferred_project) => {
                self.invalidate_project_branches();
                self.agents = discovery.agents;
                self.source_projects = discovery.projects;
                self.rebuild_projects();
                let previous = std::mem::take(&mut self.sessions);
                self.sessions = sessions;
                let positions: HashMap<_, _> = self
                    .sessions
                    .iter()
                    .enumerate()
                    .map(|(ix, s)| (s.id.clone(), ix))
                    .collect();
                for old in previous {
                    if let Some(&ix) = positions.get(&old.id) {
                        merge_session_metadata(&mut self.sessions[ix], &old);
                    } else if self.active.as_deref() == Some(&old.id)
                        || self.runs.contains_key(&old.id)
                        || self.finalizing.contains(&old.id)
                        || self.lifecycle.archived_chats.contains(&old.id)
                        || self.lifecycle.pinned_chats.contains(&old.id)
                        || old.transcript_path.is_none()
                    {
                        self.sessions.push(old);
                    }
                }
                self.sessions
                    .sort_by_key(|s| std::cmp::Reverse(s.updated_at));
                if self.project == std::path::Path::new("/")
                    && let Some(path) = preferred_project
                {
                    self.project = path;
                }
                self.visible_sessions = self
                    .sessions
                    .iter()
                    .filter(|session| !self.hidden_sessions.contains(&session.id))
                    .cloned()
                    .collect();
                self.rebuild_workspace_sessions();
                self.discovering = false;
                if self.active.is_none()
                    && !self
                        .agents
                        .iter()
                        .any(|a| a.kind == self.agent && a.available)
                    && let Some(agent) = self.agents.iter().find(|a| a.available)
                {
                    self.agent = agent.kind;
                }
            }
            Event::HistoryRange(id, generation, start, end)
                if self.active.as_deref() == Some(&id) && generation == self.generation =>
            {
                let row = self.list.logical_scroll_top().item_ix;
                self.navigation.update(cx, |rail, cx| {
                    rail.set_span(start, end);
                    rail.scrolled(row, self.rows.len(), cx);
                });
            }
            Event::HistorySeeked(
                id,
                generation,
                messages,
                has_older,
                has_newer,
                fraction,
                span,
            ) if self.active.as_deref() == Some(&id) && generation == self.generation => {
                self.navigation_previous = None;
                self.navigation_history = fraction < 1.;
                self.pending_response_start = None;
                self.receive(
                    Event::Loaded(id, generation, messages, has_older, has_newer),
                    cx,
                );
                self.transcript_reveal = None;
                self.following_bottom = fraction >= 1.;
                let row = if fraction <= 0. { 0 } else { self.rows.len() };
                self.list.scroll_to(ListOffset {
                    item_ix: row,
                    offset_in_item: px(0.),
                });
                self.history_visible = if fraction <= 0. {
                    0..40.min(self.rows.len())
                } else {
                    self.rows.len().saturating_sub(40)..self.rows.len()
                };
                self.navigation.update(cx, |rail, cx| {
                    rail.set_span(span.0, span.1);
                    rail.set_position(fraction, cx);
                });
                self.queue_workspace_save(cx);
            }
            Event::ViewportRestored(id, generation, number, messages, has_older, has_newer)
                if self.active.as_deref() == Some(&id) && generation == self.generation =>
            {
                let anchor = self.restore_scroll.take();
                self.navigation_previous = None;
                self.navigation_history = has_newer;
                self.receive(
                    Event::Loaded(id, generation, messages, has_older, has_newer),
                    cx,
                );
                self.history_offset = number;
                if let Some(page) = self.history_pages.first_mut() {
                    page.number = number;
                }
                if let Some(anchor) = anchor {
                    self.following_bottom = anchor.at_bottom && !has_newer;
                    if !anchor.at_bottom && !self.rows.is_empty() {
                        self.list.scroll_to(ListOffset {
                            item_ix: anchor.row_index.min(self.rows.len().saturating_sub(1)),
                            offset_in_item: px(anchor.pixel_offset),
                        });
                    }
                }
            }
            Event::Loaded(id, generation, messages, has_older, has_newer)
                if self.active.as_deref() == Some(&id) && generation == self.generation =>
            {
                self.navigation_previous = None;
                self.rows.clear();
                self.rows = Self::history_rows(messages);
                self.navigation.update(cx, |rail, cx| {
                    rail.reset(cx);
                    rail.set_motion(self.motion, cx);
                });
                self.history_pages = vec![HistoryPage {
                    number: 0,
                    start: 0,
                    len: self.rows.len(),
                    resident: true,
                    used: 0,
                }];
                self.deferred_rows.clear();
                self.history_pending.clear();
                self.history_failed.clear();
                self.history_offset = 0;
                self.history_visible = self.rows.len().saturating_sub(40)..self.rows.len();
                self.list.reset(self.rows.len());
                if let Some(anchor) = self.history_anchor.take() {
                    self.list.scroll_to(anchor);
                }
                self.has_older = has_older;
                self.has_newer = has_newer;
                self.loading_earlier = false;
                self.loading = false;
                if let Some(run) = self.runs.get_mut(&id) {
                    run.stream_start = None;
                }
                self.update_stream(&id, cx);
                self.transcript_reveal =
                    (self.motion && !self.rows.is_empty()).then(|| ListReveal::new(None));
                if self.send_after_load.is_some() {
                    self.composer
                        .update(cx, |_, cx| cx.emit(InputEvent::Submit));
                }
            }
            Event::OlderLoaded(id, generation, messages, has_older)
                if self.active.as_deref() == Some(&id) && generation == self.generation =>
            {
                // Prepending history retains the current viewport and presentation.
                self.transcript_reveal = None;
                let mut older = Self::history_rows(messages);
                let count = older.len();
                if self.unified_history_end.as_ref().is_some_and(|(owner,_)|owner==&self.unified_token()){
                    if let Some((_,end))=&mut self.unified_history_end{*end=if self.unified_history_wait{count}else{end.saturating_add(count)}}
                }
                self.unified_history_wait=false;
                let anchor = self.list.logical_scroll_top();
                self.history_offset += 1;
                for page in &mut self.history_pages {
                    page.start += count;
                }
                self.deferred_rows = std::mem::take(&mut self.deferred_rows)
                    .into_iter()
                    .map(|(ix, row)| (ix + count, row))
                    .collect();
                self.expanded_tools = std::mem::take(&mut self.expanded_tools)
                    .into_iter()
                    .map(|ix| ix + count)
                    .collect();
                self.interaction_animations = std::mem::take(&mut self.interaction_animations)
                    .into_iter()
                    .map(|(ix, state)| (ix + count, state))
                    .collect();
                if let Some(start) = &mut self.pending_response_start {
                    *start += count;
                }
                for run in self.runs.values_mut() {
                    if let Some(start) = &mut run.stream_start {
                        *start += count;
                    }
                }
                self.history_visible =
                    self.history_visible.start + count..self.history_visible.end + count;
                older.append(&mut self.rows);
                self.rows = older;
                self.history_clock += 1;
                self.history_pages.insert(
                    0,
                    HistoryPage {
                        number: self.history_offset,
                        start: 0,
                        len: count,
                        resident: true,
                        used: self.history_clock,
                    },
                );
                self.list.splice(0..0, count);
                self.list.scroll_to(ListOffset {
                    item_ix: anchor.item_ix + count,
                    offset_in_item: anchor.offset_in_item,
                });
                self.has_older = has_older;
                self.has_newer = self.history_offset > 0;
                self.loading_earlier = false;
                self.history_animation = Some(Instant::now());
                self.evict_history_pages();
            }
            Event::HistoryPage(id, generation, number, messages, has_older)
                if self.active.as_deref() == Some(&id) && generation == self.generation =>
            {
                if number == self.history_offset {
                    self.has_older = has_older;
                }
                self.history_pending.remove(&number);
                let restored = Self::history_rows(messages);
                if let Some(page) = self.history_pages.iter_mut().find(|p| p.number == number) {
                    if restored.len() != page.len {
                        self.history_failed.insert(number);
                        self.notice = Some(
                            "History changed while loading. Reopen this conversation to refresh."
                                .into(),
                        );
                    } else {
                        let range = page.start..page.start + page.len;
                        let anchor = self.list.logical_scroll_top();
                        for (ix, row) in range.clone().zip(restored) {
                            self.rows[ix] = row;
                            self.deferred_rows.remove(&ix);
                        }
                        self.history_clock += 1;
                        page.used = self.history_clock;
                        page.resident = true;
                        self.list.splice(range.clone(), range.len());
                        self.list.scroll_to(anchor);
                        self.evict_history_pages();
                    }
                }
            }
            Event::ProjectSessions(path, generation, sessions)
                if path == self.project && generation == self.project_generation =>
            {
                let positions: HashMap<_, _> = self
                    .sessions
                    .iter()
                    .enumerate()
                    .map(|(ix, s)| (s.id.clone(), ix))
                    .collect();
                for mut session in sessions {
                    if let Some(&ix) = positions.get(&session.id) {
                        merge_session_metadata(&mut session, &self.sessions[ix]);
                        self.sessions[ix] = session;
                    } else {
                        self.sessions.push(session);
                    }
                }
                self.sessions
                    .sort_by_key(|s| std::cmp::Reverse(s.updated_at));
                self.rebuild_workspace_sessions();
            }
            Event::ArchivedLoaded(generation, sessions)
                if generation == self.archive_generation =>
            {
                self.archive_loading = false;
                self.archive_offset = self.archive_page_end;
                let positions: HashMap<_, _> = self
                    .sessions
                    .iter()
                    .enumerate()
                    .map(|(ix, s)| (s.id.clone(), ix))
                    .collect();
                for mut session in sessions {
                    if let Some(&ix) = positions.get(&session.id) {
                        merge_session_metadata(&mut session, &self.sessions[ix]);
                        self.sessions[ix] = session;
                    } else {
                        self.sessions.push(session);
                    }
                }
                self.rebuild_workspace_sessions();
            }
            Event::ArchivedError(generation, error) if generation == self.archive_generation => {
                self.archive_loading = false;
                self.notice = Some(error);
            }
            Event::Search(generation, mut sessions) if generation == self.search_generation => {
                let query = self.search.read(cx).text().to_lowercase();
                let mut seen: HashSet<_> = sessions.iter().map(|s| s.id.clone()).collect();
                for session in &self.sessions {
                    if self
                        .lifecycle
                        .chat_names
                        .get(&session.id)
                        .is_some_and(|name| name.to_lowercase().contains(&query))
                        && seen.insert(session.id.clone())
                        && sessions.len() < 100
                    {
                        sessions.push(session.clone());
                    }
                }
                self.visible_sessions = sessions
                    .into_iter()
                    .filter(|session| !self.hidden_sessions.contains(&session.id))
                    .collect();
            }
            Event::FinishError(id, error) => {
                self.finalizing.remove(&id);
                if self.active.as_deref() == Some(&id) {
                    self.pending_response_start = None;
                    self.notice = Some(format!("The response could not be saved: {error}"));
                }
            }
            Event::PagedFinished(id) => {
                self.finalizing.remove(&id);
                self.saved_viewport = None;
                if self.navigation_history && self.active.as_deref() == Some(&id) {
                    self.has_newer = true;
                    self.pending_response_start = None;
                    self.queue_workspace_save(cx);
                    cx.notify();
                    return;
                }
                if self.active.as_deref() == Some(&id) {
                    self.pending_response_start = None;
                    self.history_offset = 0;
                    self.history_anchor = None;
                    if let Some(session) = self
                        .sessions
                        .iter()
                        .find(|session| session.id == id)
                        .cloned()
                    {
                        self.loading = true;
                        self.engine.load(session, self.generation);
                    }
                }
                self.queue_workspace_save(cx);
            }
            Event::Parsed(id, blocks) => {
                self.finalizing.remove(&id);
                if self
                    .saved_viewport
                    .as_ref()
                    .is_some_and(|(saved, _, _)| saved == &id)
                {
                    self.saved_viewport = None;
                }
                if self.active.as_deref() != Some(&id) {
                    return;
                }
                if self.navigation_history {
                    self.has_newer = true;
                    self.pending_response_start = None;
                    self.queue_workspace_save(cx);
                    cx.notify();
                    return;
                }
                self.queue_workspace_save(cx);
                let start = self
                    .pending_response_start
                    .take()
                    .unwrap_or(self.rows.len())
                    .min(self.rows.len());
                let old = self.rows.len();
                self.rows.truncate(start);
                for (ix, block) in blocks.into_iter().enumerate() {
                    self.rows.push(Row {
                        role: Some(format!(
                            "{}:{}",
                            if ix == 0 { "assistant" } else { "continuation" },
                            self.agent.command()
                        )),
                        block,
                    });
                }
                self.list.splice(start..old, self.rows.len() - start);
                if old == start && self.motion && self.rows.len() > start {
                    self.transcript_reveal = Some(ListReveal::appended(start));
                }
            }
            Event::LoadError(id, generation, error)
                if self.active.as_deref() == Some(&id) && generation == self.generation =>
            {
                if self.restore_scroll.take().is_some()
                    && let Some(session) = self
                        .sessions
                        .iter()
                        .find(|session| session.id == id)
                        .cloned()
                {
                    self.notice =
                        Some("Previous reading position changed. Showing recent messages.".into());
                    self.engine.load(session, generation);
                    return;
                }
                self.notice = Some(error);
                self.loading = false;
                self.loading_earlier = false;
                if let Some(previous) = self.navigation_previous.take() {
                    self.navigation_history = previous;
                    if previous {
                        self.has_newer = true;
                    }
                    self.update_stream(&id, cx);
                    let row = self.list.logical_scroll_top().item_ix;
                    self.navigation
                        .update(cx, |rail, cx| rail.scrolled(row, self.rows.len(), cx));
                }
                self.history_failed.extend(self.history_pending.drain());
                if let Some(submission) = self.send_after_load.take() {
                    self.hold_queue(&submission.session.id, cx);
                }
            }
            Event::ProjectError(path, generation, error)
                if path == self.project && generation == self.project_generation =>
            {
                self.notice = Some(error);
            }
            Event::Error(error) => {
                self.notice = Some(error);
                self.finalizing.clear();
                self.discovering = false;
                self.loading = false;
                self.loading_earlier = false;
                self.send_after_load = None;
            }
            _ => return,
        }
        cx.notify();
    }
    fn remember_draft(&mut self, cx: &Context<Self>) {
        self.finish_attachment_removals();
        if let Some(id) = &self.active {
            if self.attachments.is_empty() {
                self.attachment_drafts.remove(id);
            } else {
                self.attachment_drafts
                    .insert(id.clone(), self.attachments.clone());
            }
            let text = self.composer.read(cx).text();
            if text.is_empty() {
                self.drafts.remove(id);
            } else {
                self.drafts.insert(id.clone(), text);
            }
        }
    }
    fn close_tab(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tabs.retain(|tab| tab != id);
        if self.active.as_deref() == Some(id) {
            if let Some(next) = self
                .open_tabs
                .last()
                .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
                .cloned()
            {
                self.open_session(next, window, cx);
            } else {
                self.new_conversation(window, cx);
            }
        }
        cx.notify();
    }
    fn reset_conversation(&mut self, cx: &mut Context<Self>) {
        self.transcript_reveal = None;
        self.restore_scroll = None;
        self.following_bottom = true;
        self.diff_view = None;
        self.diff_observer = None;
        self.diff_started = None;
        self.stop_dictation(cx);
        self.remember_draft(cx);
        self.reset_attachment_imports();
        self.attachments.clear();
        self.send_after_load = None;
        self.active = None;
        self.history_offset = 0;
        self.has_newer = false;
        self.has_older = false;
        self.history_anchor = None;
        self.loading_earlier = false;
        self.rows.clear();
        self.expanded_tools.clear();
        self.interaction_animations.clear();
        self.question_choices.clear();
        self.history_pages.clear();
        self.deferred_rows.clear();
        self.history_pending.clear();
        self.history_failed.clear();
        self.history_visible = 0..0;
        self.navigation.update(cx, |rail, cx| rail.reset(cx));
        self.navigation_history = false;
        self.navigation_previous = None;
        self.list.reset(0);
        self.generation += 1;
        self.home_generation += 1;
        self.loading = false;
        self.pending_response_start = None;
        self.modal = None;
        self.notice = None;
        self.composer.update(cx, |input, cx| input.set_text("", cx));
    }
    fn new_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.unified_navigating{if let Some(old)=self.active.as_ref().and_then(|id|self.sessions.iter().find(|s|&s.id==id)).cloned(){self.unified_back.push(old);if self.unified_back.len()>100{self.unified_back.remove(0);}self.unified_forward.clear();}}
        self.stop_voice(cx);
        self.new_execution_path = if self.project == self.unprojected_root() {
            Some(self.project.join(uuid::Uuid::new_v4().to_string()))
        } else {
            self.execution_root()
        };
        self.reset_conversation(cx);
        self.agent_picker = None;
        self.model = self
            .agent_models
            .get(self.agent.command())
            .cloned()
            .flatten();
        self.action_menu = None;
        self.composer.read(cx).focus_handle.focus(window);
        self.sync_file_root(window, cx);
        cx.notify();
    }
    fn open_session(
        &mut self,
        session: SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.unified_navigating && self.active.as_ref()!=Some(&session.id){
            if let Some(old)=self.active.as_ref().and_then(|id|self.sessions.iter().find(|s|&s.id==id)).cloned(){
                self.unified_back.push(old);if self.unified_back.len()>100{self.unified_back.remove(0);}
            }
            self.unified_forward.clear();
        }

        if self.active.as_ref() != Some(&session.id) {
            self.stop_voice(cx);
        }
        if self.hidden_sessions.contains(&session.id) {
            return;
        }
        if self
            .restore_model
            .as_ref()
            .is_none_or(|(id, _)| id != &session.id)
        {
            self.activity.open(&session.id);
        }
        self.transcript_reveal = None;
        if self
            .restore_model
            .as_ref()
            .is_some_and(|(id, _)| id != &session.id)
            || (self.restore_scroll.is_some()
                && self.active.as_ref().is_some_and(|id| id != &session.id))
        {
            self.restore_model = None;
            self.restore_scroll = None;
        }
        self.following_bottom = self
            .restore_scroll
            .as_ref()
            .is_none_or(|anchor| anchor.at_bottom);
        self.diff_view = None;
        self.diff_observer = None;
        self.diff_started = None;
        self.action_menu = None;
        let session = self
            .sessions
            .iter()
            .find(|current| current.id == session.id)
            .cloned()
            .unwrap_or(session);
        self.stop_dictation(cx);
        self.remember_draft(cx);
        self.reset_attachment_imports();
        self.attachments = self
            .attachment_drafts
            .get(&session.id)
            .cloned()
            .unwrap_or_default();
        self.send_after_load = None;
        let draft = self.drafts.get(&session.id).cloned().unwrap_or_default();
        self.composer
            .update(cx, |input, cx| input.set_text(&draft, cx));
        self.history_offset = 0;
        self.has_newer = false;
        self.has_older = false;
        self.history_anchor = None;
        self.loading_earlier = false;
        if self.all_projects || !session.project_path.starts_with(&self.project) {
            self.project = session.project_path.clone();
        }
        self.agent_picker = None;
        self.new_execution_path = None;
        self.agent = session.agent;
        self.model = self.session_models.get(&session.id).cloned().flatten();
        self.active = Some(session.id.clone());
        self.sync_file_root(window, cx);
        if !self.open_tabs.contains(&session.id) {
            self.open_tabs.push(session.id.clone());
            if self.open_tabs.len() > 8 {
                self.open_tabs.remove(0);
            }
        }
        if !self.sidebar_split
            && self
                .restore_model
                .as_ref()
                .is_none_or(|(id, _)| id != &session.id)
            && let Some(path) = self
                .project
                .ancestors()
                .find(|path| self.projects.iter().any(|p| p.path == *path))
        {
            self.expanded_projects.insert(path.to_path_buf());
        }
        self.rebuild_workspace_sessions();
        self.pending_response_start = None;
        self.modal = None;
        self.generation += 1;
        self.rows.clear();
        self.expanded_tools.clear();
        self.interaction_animations.clear();
        self.question_choices.clear();
        self.history_pages.clear();
        self.deferred_rows.clear();
        self.history_pending.clear();
        self.history_failed.clear();
        self.history_visible = 0..0;
        self.navigation.update(cx, |rail, cx| rail.reset(cx));
        self.navigation_history = false;
        self.navigation_previous = None;
        self.list.reset(0);
        self.loading = true;
        self.notice = None;
        self.engine.load_model(session.id.clone(), self.generation);
        if let Some(anchor) = &self.restore_scroll {
            self.engine
                .restore_viewport(session, self.generation, anchor.history_offset);
        } else {
            self.engine.load(session, self.generation);
        }
        self.composer.read(cx).focus_handle.focus(window);
        cx.notify();
    }
    fn load_history_page(&mut self, older: bool, cx: &mut Context<Self>) {
        if self.loading_earlier
            || self.loading
            || self
                .active
                .as_ref()
                .is_some_and(|id| self.runs.contains_key(id) || self.finalizing.contains(id))
                && !self.navigation_history
            || (older && !self.has_older)
            || (!older && !self.has_newer)
        {
            return;
        }
        if !older && let Some(id) = self.active.clone().filter(|id| self.runs.contains_key(id)) {
            self.restore_live_tail(&id, cx);
            return;
        }
        if let Some(session) = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .cloned()
        {
            if older {
                self.loading_earlier = true;
                self.engine.load_older(session, self.generation, 200);
            } else {
                self.generation += 1;
                self.navigation_history = false;
                self.loading = true;
                self.loading_earlier = false;
                self.engine.load(session, self.generation);
            }
            cx.notify();
        }
    }
    fn history_rows(messages: Vec<engine::ParsedMessage>) -> Vec<Row> {
        messages
            .into_iter()
            .flat_map(|message| {
                message
                    .blocks
                    .into_iter()
                    .enumerate()
                    .map(move |(ix, block)| Row {
                        role: if ix == 0 {
                            Some(message.role.clone())
                        } else {
                            message
                                .role
                                .strip_prefix("assistant:")
                                .map(|provider| format!("continuation:{provider}"))
                        },
                        block,
                    })
            })
            .collect()
    }
    fn request_history_page(&mut self, number: usize) {
        if self.history_pending.contains(&number) || self.history_failed.contains(&number) {
            return;
        }
        if let Some(session) = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .cloned()
        {
            self.history_pending.insert(number);
            self.engine
                .load_history_page(session, self.generation, number);
        }
    }
    fn rehydrate_visible_history(&mut self, _cx: &mut Context<Self>) {
        let pages: HashSet<_> = self
            .history_visible
            .clone()
            .filter_map(|ix| self.deferred_rows.get(&ix).map(|row| row.page))
            .collect();
        for number in pages {
            self.request_history_page(number);
        }
    }
    fn estimated_row_height(row: &Row, width: Pixels) -> Pixels {
        let text_height = |text: &str, leading: f32| {
            let columns = (f32::from(width).max(200.) / 7.).max(1.) as usize;
            let lines: usize = text
                .split('\n')
                .map(|line| line.chars().count().max(1).div_ceil(columns))
                .sum();
            lines as f32 * leading
        };
        let body = match &row.block {
            MessageBlock::Voice { text, .. }
            | MessageBlock::Paragraph(text)
            | MessageBlock::RichParagraph { text, .. } => text_height(text, 24.),
            MessageBlock::Code { text, .. } => 56. + text.lines().count().max(1) as f32 * 20.,
            MessageBlock::Activity { .. } => 36.,
            MessageBlock::Attachment { .. } => 50.,
            MessageBlock::Interaction(card) => match card {
                chat_interactions::ChatInteraction::Question(question) => {
                    text_height(&question.prompt, 23.) + question.options.len() as f32 * 64. + 50.
                }
                chat_interactions::ChatInteraction::Speaker { text, .. } => {
                    text_height(text, 23.) + 40.
                }
                chat_interactions::ChatInteraction::Answer { question, answer } => {
                    text_height(question, 21.) + text_height(answer, 21.) + 24.
                }
                _ => 36.,
            },
            MessageBlock::Heading(text) => text_height(text, 28.),
            MessageBlock::Quote(text) => text_height(text, 24.),
            MessageBlock::ListItem { text, .. } => text_height(text, 23.),
            MessageBlock::TableRow { cells, .. } => {
                cells
                    .iter()
                    .map(|text| text_height(text, 21.))
                    .fold(21., f32::max)
                    + 16.
            }
        };
        px(body
            + 8.
            + if row_has_header(row.role.as_deref()) {
                50.
            } else {
                0.
            })
    }
    fn evict_history_pages(&mut self) {
        // Keep nearby pages decoded; everything else retains only row geometry
        // and a disk-cache page key. Returning to it requests that bounded page.
        let visible = self.history_visible.start.saturating_sub(64)
            ..self.history_visible.end.saturating_add(64);
        while self.history_pages.iter().filter(|p| p.resident).count() > 8 {
            let candidate = self
                .history_pages
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.resident
                        && !self.history_pending.contains(&p.number)
                        && (p.start + p.len <= visible.start || p.start >= visible.end)
                })
                .min_by_key(|(_, p)| p.used)
                .map(|(ix, _)| ix);
            let Some(index) = candidate else {
                break;
            };
            let page = &mut self.history_pages[index];
            page.resident = false;
            let number = page.number;
            let range = page.start..page.start + page.len;
            let width = self.list.viewport_bounds().size.width;
            for ix in range {
                let height = self
                    .list
                    .bounds_for_item(ix)
                    .map(|b| b.size.height)
                    .unwrap_or_else(|| Self::estimated_row_height(&self.rows[ix], width));
                self.deferred_rows.insert(
                    ix,
                    DeferredRow {
                        page: number,
                        height,
                        checkpoint: checkpoint_ui::row_checkpoint(&self.rows[ix])
                            .map(str::to_owned),
                        user_boundary: self.rows[ix].role.as_deref() == Some("user"),
                    },
                );
                self.rows[ix] = Row {
                    role: None,
                    block: MessageBlock::Paragraph(String::new()),
                };
            }
        }
    }
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.queue_editing.is_some() {
            self.save_queued_edit(cx);
            return;
        }
        if self.quit_pending {
            return;
        }
        if self
            .voice_call
            .as_ref()
            .is_some_and(|call| call.connecting(self.active.as_deref()))
        {
            self.notice = Some(
                "Voice is connecting. Your draft is preserved; send it once voice is ready.".into(),
            );
            cx.notify();
            return;
        }
        if self.send_after_load.is_some() {
            if self.loading || self.loading_earlier {
                return;
            }
            let submission = self.send_after_load.take().unwrap();
            if self.active.as_deref() == Some(&submission.session.id) {
                self.route_submission(submission, window, cx);
            }
            return;
        }
        if self.checkpoint_busy > 0
            && !self
                .active
                .as_ref()
                .is_some_and(|id| self.runs.contains_key(id))
        {
            self.notice =
                Some("Wait for file checkpoint work to finish. Your draft is preserved.".into());
            cx.notify();
            return;
        }
        let draft = self.composer.read(cx).text();
        if draft.trim().is_empty()
            && self.attachments.is_empty()
            && self.voice_call.as_ref().is_some_and(|call| call.active)
        {
            self.stop_voice(cx);
            return;
        }

        if let Some(call) = self
            .voice_call
            .as_ref()
            .filter(|call| call.active && self.active.as_ref() == Some(&call.owner))
            && !draft.trim().is_empty()
            && !draft.trim_start().starts_with('/')
        {
            if !self.attachments.is_empty() {
                self.notice = Some("End voice before sending image attachments. Your draft and images are preserved.".into());
            } else if let Some(run) = self.runs.get(&call.owner) {
                match run.handle.steer(&draft) {
                    Ok(()) => {
                        self.composer.update(cx, |input, cx| input.set_text("", cx));
                        self.queue_workspace_save(cx);
                    }
                    Err(error) => self.notice = Some(error.to_string()),
                }
            }
            cx.notify();
            return;
        }
        let delegate_task = (commands::name(draft.trim()) == Some("delegate")
            && runtime::supports_native_delegation(self.agent))
        .then(|| {
            draft
                .trim()
                .split_once(char::is_whitespace)
                .map_or("", |(_, task)| task.trim())
        })
        .filter(|task| !task.is_empty());
        if commands::name(draft.trim()).is_some_and(|name| self.native_command_available(name))
            && self
                .active
                .as_ref()
                .and_then(|id| self.sessions.iter().find(|session| &session.id == id))
                .is_some_and(|session| session.agent != self.agent)
        {
            self.notice = Some("Send a message to continue with the selected agent before invoking its native commands.".into());
            cx.notify();
            return;
        }
        if delegate_task.is_none() && self.handle_command(window, cx) {
            return;
        }
        let delegation_prompt = match delegate_task
            .map(|task| runtime::delegation_prompt(self.agent, task))
            .transpose()
        {
            Ok(prompt) => prompt,
            Err(error) => {
                self.notice = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        if delegation_prompt.is_some()
            && self
                .active
                .as_ref()
                .is_some_and(|id| self.runs.contains_key(id))
        {
            self.notice = Some("Finish or stop this turn before requesting another subagent. Your task is kept in the composer.".into());
            cx.notify();
            return;
        }
        if self.handoff_pending.is_some() {
            self.notice = Some("The submitted message is still preparing its context. Your current draft is preserved.".into());
            cx.notify();
            return;
        }
        self.stop_dictation(cx);
        self.finish_attachment_removals();
        if self
            .active
            .as_ref()
            .is_some_and(|id| self.runs.contains_key(id))
            && (!self.composer.read(cx).text().trim().is_empty() || !self.attachments.is_empty())
        {
            self.enqueue_followup(cx);
            return;
        }
        if let Some(id) = &self.active
            && let Some(run) = self.runs.get_mut(id)
        {
            run.handle.cancel();
            run.status = "Stopping…".into();
            cx.notify();
            return;
        }
        if self
            .active
            .as_ref()
            .is_some_and(|id| self.finalizing.contains(id))
        {
            return;
        }
        let prompt = self.composer.read(cx).text().trim().to_string();
        if self
            .context_pending
            .values()
            .any(|p| p.scope == self.context_scope())
        {
            self.notice = Some(
                "Attached chat context is still being prepared. Your draft is preserved.".into(),
            );
            cx.notify();
            return;
        }
        if self.attachment_pending > 0 {
            self.notice = Some("The image previews are still loading.".into());
            cx.notify();
            return;
        }
        if !self.attachments.is_empty() && !runtime::supports_images(self.agent) {
            self.notice = Some(format!(
                "{} does not expose image input here. Choose Codex, Claude Code, Gemini or OpenCode to send these images.",
                agent_name(self.agent)
            ));
            cx.notify();
            return;
        }
        let prompt = if prompt.is_empty() && !self.attachments.is_empty() {
            "Describe these images.".to_owned()
        } else {
            prompt
        };
        if prompt.is_empty() || self.loading || self.loading_earlier || self.starting {
            return;
        }
        if self
            .active
            .as_ref()
            .is_some_and(|id| self.settling.contains_key(id))
        {
            self.notice = Some(
                "The previous CLI is finishing process cleanup. Try again in a moment.".into(),
            );
            cx.notify();
            return;
        }
        let Some(executable) = self
            .agents
            .iter()
            .find(|a| a.kind == self.agent)
            .and_then(|a| a.executable.clone())
        else {
            self.notice = Some(format!(
                "Install {} and sign in from Terminal to get started.",
                agent_name(self.agent)
            ));
            cx.notify();
            return;
        };
        if self.runs.len() + self.team_starting.len() >= 4 {
            self.notice = Some("Four agents are running. Wait for one to finish.".into());
            cx.notify();
            return;
        }
        let now = chrono::Utc::now().timestamp();
        let mut session = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .cloned()
            .unwrap_or_else(|| SessionSummary {
                id: uuid::Uuid::new_v4().to_string(),
                title: short(&prompt.replace('\n', " "), 46),
                agent: self.agent,
                project_path: self.project.clone(),
                updated_at: now,
                external_id: None,
                transcript_path: None,
                execution_path: self.new_execution_path.clone(),
            });
        session.updated_at = now;
        let id = session.id.clone();
        let execution_path = session
            .execution_path
            .clone()
            .unwrap_or_else(|| session.project_path.clone());
        let native_command =
            commands::name(&prompt).is_some_and(|name| self.native_command_available(name));
        if self.teams.iter().any(|team| {
            teams::overlapping(&team.project, &execution_path)
                && (self.allow_edits || team.allow_edits)
                && team.members.iter().any(|member| member.status.active())
        }) {
            self.notice = Some("An agent team is using this workspace. Finish or stop its editing task before starting another turn.".into());
            cx.notify();
            return;
        }
        let references = self.context_references();
        let native_prompt = match context_refs::with_context(
            &delegation_prompt.unwrap_or_else(|| prompt.clone()),
            &references,
        ) {
            Ok(prompt) => prompt,
            Err(error) => {
                self.notice = Some(error.to_string());
                cx.notify();
                return;
            }
        };
        let context_scope = self.context_scope();
        let request = RunRequest { reasoning_effort:self.unified_effort().map(str::to_owned),
            agent: self.agent,
            executable,
            project: execution_path.clone(),
            prompt: native_prompt,
            resume_id: session.external_id.clone(),
            fork_session: imported_codex_needs_fork(&session)
                || self.fork_next.as_deref() == Some(&id),
            allow_edits: self.allow_edits,
            model: self.model.clone(),
            images: self.attachments.iter().map(|a| a.path.clone()).collect(),
            codex_approvals: self.codex_approvals,
            trust_workspace: self
                .trusted_workspaces
                .contains(&(self.agent, execution_path)),
        };
        let mut saved_prompt = self.prompt_with_attachments(&prompt);
        for reference in &references {
            saved_prompt.push_str(&format!(
                "\n\nAttached chat: {} · {} · {}{}",
                reference.title,
                reference.provider,
                reference.project,
                if reference.older_omitted {
                    " (bounded excerpt)"
                } else {
                    ""
                }
            ));
        }
        let submission = handoff_ui::Submission {
            session,
            request,
            prompt,
            original_prompt: self.composer.read(cx).text(),
            saved_prompt,
            attachments: self.attachments.clone(),
            references,
            context_scope,
            native_command,
            queued_prompt_id: self.queue_dispatch.clone(),
        };
        if self.history_offset > 0 || self.navigation_history {
            self.navigation_previous = Some(true);
            self.navigation_history = false;
            self.history_offset = 0;
            self.history_anchor = None;
            self.loading = true;
            self.generation += 1;
            self.engine
                .load(submission.session.clone(), self.generation);
            self.send_after_load = Some(submission);
            cx.notify();
            return;
        }
        self.route_submission(submission, window, cx);
    }
    fn route_submission(
        &mut self,
        submission: handoff_ui::Submission,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if submission.session.agent != submission.request.agent {
            self.begin_send_handoff(submission, window, cx);
        } else {
            self.submit_captured(submission, window, cx);
        }
    }
    fn submit_captured(
        &mut self,
        submission: handoff_ui::Submission,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handoff_ui::Submission {
            session,
            request,
            prompt,
            original_prompt,
            saved_prompt,
            attachments: submitted_attachments,
            references,
            context_scope,
            native_command,
            queued_prompt_id,
        } = submission;
        let id = session.id.clone();
        self.fork_next = None;
        self.indicator_terminal = None;
        self.active = Some(id.clone());
        self.modal = None;
        self.notice = None;
        self.sessions.retain(|s| s.id != id);
        self.sessions.insert(0, session.clone());
        self.visible_sessions = self
            .sessions
            .iter()
            .filter(|session| !self.hidden_sessions.contains(&session.id))
            .cloned()
            .collect();
        self.rebuild_workspace_sessions();
        if !self.open_tabs.contains(&id) {
            self.open_tabs.push(id.clone());
            if self.open_tabs.len() > 8 {
                self.open_tabs.remove(0);
            }
        }
        if self.session_models.get(&id) != Some(&request.model) {
            self.engine
                .set_model(session.clone(), request.model.clone());
            self.session_models
                .insert(id.clone(), request.model.clone());
        }
        self.agent_picker = None;
        let checkpoint_title = prompt.chars().take(256).collect::<String>();
        let checkpoint_key = uuid::Uuid::new_v4().to_string();
        let checkpoint_session = id.clone();
        let checkpoint_repository = self.checkpoint_repository.clone();
        let checkpoint_cleanup_repository = checkpoint_repository.clone();
        let checkpoint_cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.checkpoint_cancel = checkpoint_cancel.clone();
        let save_session = session.clone();
        let old = self.rows.len();
        self.rows.push(Row {
            role: Some("user".into()),
            block: MessageBlock::Paragraph(prompt),
        });
        for attachment in &submitted_attachments {
            self.rows.push(Row {
                role: None,
                block: MessageBlock::Attachment {
                    name: attachment.name.clone(),
                    target: attachment.path.to_string_lossy().into_owned(),
                },
            });
        }
        self.pending_response_start = Some(self.rows.len());
        self.list.splice(old..old, self.rows.len() - old);
        self.transcript_reveal = self.motion.then(|| ListReveal::appended(old));
        self.list.scroll_to_reveal_item(old);
        if self.composer.read(cx).text() == original_prompt {
            self.composer.update(cx, |input, cx| input.set_text("", cx));
            self.drafts.remove(&id);
        } else {
            self.drafts
                .insert(id.clone(), self.composer.read(cx).text());
        }
        self.composer.read(cx).focus_handle.focus(window);
        self.notice = Some(format!("Connecting to {}…", agent_name(request.agent)));
        self.starting = true;
        let retry = request.clone();
        let queued_state_saved = queued_prompt_id
            .as_ref()
            .and_then(|_| self.workspace_store.as_ref().map(|store| store.flush()));
        let handoff_packet = self
            .handoff_packets
            .get(&id)
            .filter(|_| request.resume_id.is_none())
            .cloned();
        let unprojected_workspace = self.unprojected_workspace(&session);
        let background = cx.background_executor().spawn(async move {
            if let Some(saved) = queued_state_saved {
                saved
                    .recv_timeout(Duration::from_secs(10))
                    .map_err(|e| anyhow::anyhow!("Queued prompt state did not save: {e}"))?
                    .map_err(anyhow::Error::msg)?;
            }
            let mut request = request;
            if let Some(packet) = handoff_packet {
                request.prompt = packet.prompt(&request.prompt);
            }
            let native_input = store::NativeInput::from_prompt(&request.prompt);
            let repository = checkpoint_repository.map_err(anyhow::Error::msg)?;
            if let Some(workspace) = unprojected_workspace {
                std::fs::create_dir_all(&workspace).map_err(|error| {
                    anyhow::anyhow!("Could not create this chat's workspace: {error}")
                })?;
            }
            checkpoint_ui::launch_checkpointed(
                &repository,
                request,
                &checkpoint_session,
                &checkpoint_key,
                &checkpoint_title,
                native_command,
                &checkpoint_cancel,
            )
            .map(|(handle, checkpoint)| (handle, checkpoint, native_input))
        });
        cx.spawn(async move |this, cx| match background.await {
            Ok((handle, checkpoint_id, native_input)) => {
                let mut cleanup = checkpoint_cleanup_repository.ok().map(|repository| {
                    repository.cleanup_on_orphan(checkpoint_id.clone(), handle.settled.clone())
                });
                let wake = handle.wake.clone();
                if this
                    .update(cx, |this, cx| {
                        if this.active.as_deref() == Some(&id) {
                            this.notice = None;
                        }
                        this.starting = false;
                        if let Some(current) = this.chat_context.get_mut(&context_scope) {
                            current.retain(|reference| !references.contains(reference));
                            if current.is_empty() {
                                this.chat_context.remove(&context_scope);
                            }
                        }
                        this.queue_submitted(queued_prompt_id.as_deref(), cx);
                        this.checkpoint_runs
                            .insert(id.clone(), checkpoint_id.clone());
                        let saved_prompt =
                            format!("{saved_prompt}\n\n<!--code-checkpoint:{checkpoint_id}-->");
                        this.engine
                            .save_prompt(save_session, saved_prompt, native_input);
                        if this.active.as_deref() == Some(&id)
                            && let Some(at) = this.pending_response_start
                            && at <= this.rows.len()
                        {
                            this.rows.insert(
                                at,
                                Row {
                                    role: None,
                                    block: MessageBlock::Activity {
                                        label: "Prompt changes".into(),
                                        details: checkpoint_id.clone(),
                                    },
                                },
                            );
                            this.list.splice(at..at, 1);
                            this.pending_response_start = Some(at + 1);
                        }
                        for attachment in &submitted_attachments {
                            attachment.mark_submitted();
                        }
                        let was_submitted = |attachment: &attachments::Attachment| {
                            submitted_attachments
                                .iter()
                                .any(|old| old.id == attachment.id)
                        };
                        if let Some(draft) = this.attachment_drafts.get_mut(&id) {
                            draft.retain(|attachment| !was_submitted(attachment));
                            if draft.is_empty() {
                                this.attachment_drafts.remove(&id);
                            }
                        }
                        if this.active.as_deref() == Some(&id) {
                            this.attachments
                                .retain(|attachment| !was_submitted(attachment));
                            this.attachment_inserts.retain(|id, _| {
                                !submitted_attachments.iter().any(|old| &old.id == id)
                            });
                            this.attachment_removals.retain(|id, _| {
                                !submitted_attachments.iter().any(|old| &old.id == id)
                            });
                            if this
                                .attachment_preview
                                .as_ref()
                                .is_some_and(|(attachment, _)| was_submitted(attachment))
                            {
                                this.attachment_preview = None;
                                this.attachment_preview_image = None;
                                this.attachment_preview_cancel
                                    .store(true, std::sync::atomic::Ordering::Release);
                            }
                        }
                        this.runs.insert(
                            id.clone(),
                            Running {
                                agent: retry.agent,
                                submitted_prompt: original_prompt,
                                handle,
                                text: String::new(),
                                live_prefix_omitted: false,
                                status: "Thinking".into(),
                                stream_start: None,
                                revision: 0,
                                parsed_revision: 0,
                                parse_pending: false,
                                live_blocks: Vec::new(),
                                live_earlier: false,
                                parse_epoch: uuid::Uuid::new_v4(),
                                retry: Some(retry),
                                submitted_attachments: submitted_attachments.clone(),
                            },
                        );
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                if let Some(cleanup) = cleanup.as_mut() {
                    cleanup.disarm();
                }
                loop {
                    // A quiet agent causes no UI timer wakeups. Coalesce actual
                    // output for one frame, then drain a bounded event batch.
                    if wake.recv().await.is_ok() {
                        Timer::after(Duration::from_millis(16)).await;
                    }
                    match this.update(cx, |this, cx| this.poll(&id, cx)) {
                        Ok(true) => {}
                        _ => break,
                    }
                }
            }
            Err(error) => {
                let _ = this.update(cx, |this, cx| {
                    this.hold_queue(&id, cx);
                    if this.active.as_deref() == Some(&id) {
                        this.notice = Some(error.to_string());
                    }
                    this.starting = false;
                    let restored = if this.active.as_deref() == Some(&id) {
                        let current = this.composer.read(cx).text();
                        if current.is_empty() {
                            original_prompt.clone()
                        } else {
                            current
                        }
                    } else {
                        this.drafts
                            .get(&id)
                            .filter(|draft| !draft.is_empty())
                            .cloned()
                            .unwrap_or_else(|| original_prompt.clone())
                    };
                    this.drafts.insert(id.clone(), restored);
                    if this.active.as_deref() == Some(&id) {
                        if this.pending_response_start.is_some() {
                            let end = this.rows.len();
                            this.rows.truncate(old);
                            this.list.splice(old..end, 0);
                            this.pending_response_start = None;
                        }
                        if this.composer.read(cx).text().is_empty() {
                            this.composer
                                .update(cx, |input, cx| input.set_text(&original_prompt, cx));
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }
    fn approve_session_fork(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(context) = self.session_conflict.take() {
            if self.workspace_matches(&context) && context.agent == AgentKind::Codex {
                self.fork_next = Some(context.session.clone());
                self.workspace_retry = Some(context);
            } else {
                self.notice = Some("Return to the original chat to continue it here.".into());
            }
        }
        self.session_conflict_open = false;
        self.dismiss_panel(window, cx);
    }
    fn approve_workspace_trust(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(trust) = self.workspace_trust.take() {
            if self.workspace_matches(&trust) {
                if self.trusted_workspaces.len() >= 128
                    && let Some(old) = self.trusted_workspaces.iter().next().cloned()
                {
                    self.trusted_workspaces.remove(&old);
                }
                self.trusted_workspaces
                    .insert((trust.agent, trust.project.clone()));
                // Final parsing is asynchronous. Retry after it finishes, so no
                // old response can splice over the new invocation's output.
                self.workspace_retry = Some(trust);
            } else {
                self.notice = Some("Return to the original chat to retry this project.".into());
            }
        }
        self.workspace_trust_open = false;
        self.dismiss_panel(window, cx);
    }
    fn workspace_matches(&self, trust: &WorkspaceTrust) -> bool {
        self.active.as_deref() == Some(&trust.session)
            && self.agent == trust.agent
            && self
                .sessions
                .iter()
                .find(|s| s.id == trust.session)
                .is_some_and(|s| {
                    s.execution_path.as_ref().unwrap_or(&s.project_path) == &trust.project
                })
    }
    fn poll(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        self.sync_sidebar_activity(cx);
        let Some(run) = self.runs.get_mut(id) else {
            return false;
        };
        let mut text_dirty = false;
        let mut status_changed = false;
        let mut finished = false;
        let mut failed = None;
        let mut external = None;
        let mut voice_signals = Vec::new();
        for _ in 0..128 {
            match run.handle.events.try_recv() {
                Ok(RuntimeEvent::Voice(signal)) => {
                    voice_signals.push(signal);
                    status_changed = true;
                }
                Ok(RuntimeEvent::Request(request)) => {
                    self.native_pending.insert(
                        request.id.clone(),
                        native_request_ui::LiveRequest::new(id, run.parse_epoch, *request),
                    );
                    run.status = "Waiting for your decision".into();
                    status_changed = true;
                }
                Ok(RuntimeEvent::RequestResolved(request)) => {
                    self.native_pending.remove(&request);
                    run.status = "Working".into();
                    status_changed = true;
                }
                Ok(RuntimeEvent::RequestRejected { id, error }) => {
                    if let Some(pending) = self.native_pending.get_mut(&id) {
                        pending.retry();
                    }
                    self.notice = Some(error);
                    status_changed = true;
                }
                Ok(RuntimeEvent::Models(models)) => {
                    if let Some(request) = &run.retry {
                        self.native_models.insert(
                            request.agent,
                            models
                                .into_iter()
                                .filter(|model| runtime::valid_model_id(&model.id))
                                .take(256)
                                .collect(),
                        );
                        status_changed = true;
                    }
                }
                Ok(RuntimeEvent::Commands(commands)) => {
                    if let Some(request) = &run.retry {
                        let key = (request.agent, request.project.clone());
                        if self.native_commands.len() >= 16
                            && !self.native_commands.contains_key(&key)
                            && let Some(old) = self.native_commands.keys().next().cloned()
                        {
                            self.native_commands.remove(&old);
                        }
                        self.native_commands.insert(
                            key,
                            commands.into_iter().map(std::sync::Arc::from).collect(),
                        );
                        self.native_command_revision = self.native_command_revision.wrapping_add(1);
                        status_changed = true;
                    }
                }
                Ok(RuntimeEvent::Usage(usage)) => {
                    self.usage.entry(id.to_owned()).or_default().merge(*usage);
                    status_changed = true;
                }
                Ok(RuntimeEvent::Text(text)) => {
                    append_live_text(run, &text);
                    if run.status != "Writing" {
                        run.status = "Writing".into();
                        status_changed = true;
                    }
                    run.revision = run.revision.wrapping_add(1);
                    text_dirty = true;
                }
                Ok(RuntimeEvent::ReplaceText(text)) => {
                    run.text = text;
                    run.live_prefix_omitted = false;
                    run.revision = run.revision.wrapping_add(1);
                    text_dirty = true;
                }
                Ok(RuntimeEvent::ReplacePreview(text)) => {
                    run.text = text;
                    run.live_prefix_omitted = true;
                    run.revision = run.revision.wrapping_add(1);
                    text_dirty = true;
                }
                Ok(RuntimeEvent::Interaction(card)) => {
                    if matches!(card.as_ref(), chat_interactions::ChatInteraction::Status {title, ..} if title == "Context compacted")
                        && let Some(usage) = self.usage.get_mut(id)
                    {
                        usage.context_used = None;
                    }
                    let next = match card.as_ref() {
                        chat_interactions::ChatInteraction::Delegation {
                            status:
                                chat_interactions::InteractionStatus::Started
                                | chat_interactions::InteractionStatus::Working,
                            ..
                        } => Some("Delegating"),
                        chat_interactions::ChatInteraction::Question(_) => Some("Question for you"),
                        chat_interactions::ChatInteraction::Status {
                            status: chat_interactions::InteractionStatus::Working,
                            title,
                            ..
                        } if title != "Continued in Code" => Some(title.as_str()),
                        _ => None,
                    };
                    if let Some(next) = next {
                        run.status = next.into();
                        status_changed = true;
                    }
                    run.text.push_str("\n\n");
                    run.text
                        .push_str(&chat_interactions::serialize_interaction(&card));
                    run.text.push_str("\n\n");
                    run.revision = run.revision.wrapping_add(1);
                    text_dirty = true;
                }
                Ok(RuntimeEvent::Status(status)) => {
                    if status == "Stopped" {
                        let card = chat_interactions::ChatInteraction::Status {
                            title: "Stopped".into(),
                            details: "You stopped this response.".into(),
                            status: chat_interactions::InteractionStatus::Cancelled,
                        };
                        run.text.push_str("\n\n");
                        run.text
                            .push_str(&chat_interactions::serialize_interaction(&card));
                        run.revision = run.revision.wrapping_add(1);
                        text_dirty = true;
                    }
                    let status = short(&status, 64);
                    if run.status != status {
                        run.status = status;
                        status_changed = true;
                    }
                }
                Ok(RuntimeEvent::SessionId(session)) => {
                    external = Some(session);
                }
                Ok(RuntimeEvent::Finished) => {
                    finished = true;
                    break;
                }
                Ok(RuntimeEvent::Failed(error)) => {
                    let card = chat_interactions::ChatInteraction::Status {
                        title: if runtime::is_active_writer_conflict(&error) {
                            "Chat open in another app"
                        } else {
                            "Agent failed"
                        }
                        .into(),
                        details: runtime::diagnostic_summary(&error),
                        status: chat_interactions::InteractionStatus::Failed,
                    };
                    run.text.push_str("\n\n");
                    run.text
                        .push_str(&chat_interactions::serialize_interaction(&card));
                    run.revision = run.revision.wrapping_add(1);
                    text_dirty = true;
                    if self.active.as_deref() == Some(id) {
                        // The typed chat card is the sole failure surface.
                        self.notice = None;
                    }
                    failed = Some(error);
                    finished = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // A cancelled producer must never wait for queue space to
                    // publish its terminal status. Recover it out of band.
                    if run.handle.is_cancelled() && run.status != "Stopped" {
                        let card = chat_interactions::ChatInteraction::Status {
                            title: "Stopped".into(),
                            details: "You stopped this response.".into(),
                            status: chat_interactions::InteractionStatus::Cancelled,
                        };
                        run.text.push_str("\n\n");
                        run.text
                            .push_str(&chat_interactions::serialize_interaction(&card));
                        run.revision = run.revision.wrapping_add(1);
                        text_dirty = true;
                    }
                    finished = true;
                    break;
                }
                Err(_) => break,
            }
        }
        if run.text.len() > 64 * 1024 {
            let mut start = run.text.len() - 64 * 1024;
            while !run.text.is_char_boundary(start) {
                start += 1;
            }
            // Only the small live tail resides on the UI thread. The runtime
            // worker independently retains the complete response on disk.
            run.text.drain(..start);
            run.live_prefix_omitted = true;
        }
        for signal in voice_signals {
            self.handle_voice_signal(signal, cx);
        }
        self.synchronize_voice_status();
        if let Some(external) = external
            && let Some(session) = self.sessions.iter_mut().find(|s| s.id == id)
        {
            session.external_id = Some(external);
            self.engine.update_session(session.clone());
            // The service may announce child ownership before the UI has stored
            // its parent's native ID. Reconcile once when that association lands.
            self.refresh_broker(cx);
        }
        if text_dirty && self.active.as_deref() == Some(id) {
            self.update_stream(id, cx);
        }
        if finished {
            let ended_voice=self.runs.get(id).is_some_and(|r|self.voice_runs.remove(&(id.to_owned(),r.parse_epoch)));
            if self
                .voice_call
                .as_ref()
                .is_some_and(|call| call.owner == id)
            {
                self.voice_error=failed.clone();self.voice_call.take();
            }
            if failed.is_some()
                || self
                    .runs
                    .get(id)
                    .is_some_and(|run| run.handle.is_cancelled())
            {
                self.hold_queue(id, cx);
            }
            self.native_pending
                .retain(|_, request| request.owner != id || request.broker.is_some());
            if let Some(run) = self.runs.remove(id) {
                if failed.is_none()
                    && !ended_voice
                    && !run.handle.is_cancelled()
                    && !matches!(run.status.as_str(), "Stopped" | "Interrupted")
                {
                    if self.unified_successes.len() >= 128 {
                        self.unified_successes.pop_front();
                    }
                    self.unified_successes
                        .push_back((id.to_owned(), run.parse_epoch.to_string()));
                }
                self.team_finished(
                    id,
                    failed.is_some(),
                    run.handle.is_cancelled()
                        || matches!(run.status.as_str(), "Stopped" | "Interrupted"),
                    cx,
                );
                self.seal_checkpoint_run(id, run.handle.settled.clone(), cx);
                self.track_settling(id, &run.handle, cx);
                if self.active.as_deref() == Some(id) {
                    let scene = if failed.is_some() {
                        "failure"
                    } else if run.handle.is_cancelled()
                        || matches!(run.status.as_str(), "Stopped" | "Interrupted")
                    {
                        "declined"
                    } else {
                        "success"
                    };
                    self.indicator_terminal = Some((id.to_owned(), scene));
                }
                if let (Some(error), Some(request)) = (failed, run.retry) {
                    // Restore only the submitted draft; never overwrite new typing.
                    self.drafts
                        .entry(id.to_owned())
                        .or_insert_with(|| run.submitted_prompt.clone());
                    if !run.submitted_attachments.is_empty() {
                        self.attachment_drafts
                            .entry(id.to_owned())
                            .or_insert_with(|| run.submitted_attachments.clone());
                    }
                    if self.active.as_deref() == Some(id) {
                        if self.composer.read(cx).text().is_empty() && self.attachments.is_empty() {
                            self.composer
                                .update(cx, |input, cx| input.set_text(&run.submitted_prompt, cx));
                            self.attachments = run.submitted_attachments;
                        }
                        if request.agent == AgentKind::Codex
                            && runtime::is_active_writer_conflict(&error)
                        {
                            self.session_conflict = Some(WorkspaceTrust {
                                session: id.to_owned(),
                                agent: request.agent,
                                project: request.project.clone(),
                            });
                            self.session_conflict_open = true;
                        }
                        let needs_trust = match request.agent {
                            AgentKind::Gemini => {
                                error.contains("trusted directory")
                                    || error.contains("untrusted workspace")
                            }
                            AgentKind::Cursor => error.contains("Workspace Trust Required"),
                            _ => false,
                        };
                        if needs_trust {
                            self.workspace_trust = Some(WorkspaceTrust {
                                session: id.to_owned(),
                                agent: request.agent,
                                project: request.project,
                            });
                            self.workspace_trust_open = true;
                        }
                    }
                }
                self.finalizing.insert(id.to_string());
                if let Some(usage) = self.usage.get(id) {
                    self.engine.save_usage(id.to_owned(), usage.clone());
                }
                if let Some(usage) = self.usage.get(id).cloned() {
                    self.remember_usage(id.to_owned(), usage);
                }
                if let Some(session) = self.sessions.iter().find(|s| s.id == id) {
                    if let Some(source) = run.handle.take_response_source() {
                        self.engine.finish_source(session.clone(), source);
                    } else {
                        self.engine.finish(session.clone(), run.text);
                    }
                }
            }
            self.visible_sessions = self
                .sessions
                .iter()
                .filter(|session| !self.hidden_sessions.contains(&session.id))
                .cloned()
                .collect();
            self.sync_sidebar_activity(cx);
            cx.notify();
            return false;
        }
        if (text_dirty || status_changed) && self.active.as_deref() == Some(id) {
            cx.notify();
        }
        true
    }
    fn update_stream(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.navigation_history && self.active.as_deref() == Some(id) {
            return;
        }
        let Some(run) = self.runs.get_mut(id) else {
            return;
        };
        if run.text.is_empty() {
            return;
        }
        if run.parsed_revision == run.revision {
            self.apply_live_rows(id);
            return;
        }
        if run.parse_pending {
            return;
        }
        run.parse_pending = true;
        let revision = run.revision;
        let epoch = run.parse_epoch;
        let mut offset = run.text.len().saturating_sub(6000);
        while !run.text.is_char_boundary(offset) {
            offset += 1;
        }
        if offset > 0 {
            // Start at a complete line so a clipped protocol marker cannot be
            // mistaken for ordinary text. Giant single lines wait for final parsing.
            offset = run.text[offset..]
                .find('\n')
                .map_or(run.text.len(), |i| offset + i + 1);
        }
        let earlier = run.live_prefix_omitted || offset > 0;
        let tail = run.text[offset..].to_owned();
        let id = id.to_owned();
        let job = cx
            .background_executor()
            .spawn(async move { presentation::live_blocks(&tail) });
        cx.spawn(async move |this, cx| {
            let blocks = job.await;
            let _ = this.update(cx, |this, cx| {
                let Some(run) = this.runs.get_mut(&id) else {
                    return;
                };
                if run.parse_epoch != epoch {
                    return;
                }
                run.parse_pending = false;
                if run.revision == revision {
                    run.live_blocks = blocks;
                    run.live_earlier = earlier;
                    run.parsed_revision = revision;
                    if this.active.as_deref() == Some(&id) {
                        this.apply_live_rows(&id);
                        cx.notify();
                    }
                } else if this.active.as_deref() == Some(&id) {
                    this.update_stream(&id, cx);
                }
            });
        })
        .detach();
    }
    fn apply_live_rows(&mut self, id: &str) {
        if self.navigation_history && self.active.as_deref() == Some(id) {
            return;
        }
        let Some(run) = self.runs.get_mut(id) else {
            return;
        };
        let start = *run.stream_start.get_or_insert(self.rows.len());
        self.pending_response_start = Some(start);
        let old = self.rows.len();
        self.rows.truncate(start);
        if run.live_earlier {
            self.rows.push(Row {
                role: Some(format!("assistant:{}", run.agent.command())),
                block: MessageBlock::Paragraph(
                    "Earlier output is saved. Showing the live tail…".into(),
                ),
            });
        }
        for (ix, block) in run.live_blocks.iter().enumerate() {
            self.rows.push(Row {
                role: Some(format!(
                    "{}:{}",
                    if ix == 0 && !run.live_earlier {
                        "assistant"
                    } else {
                        "continuation"
                    },
                    run.agent.command()
                )),
                block: block.clone(),
            });
        }
        self.list.splice(start..old, self.rows.len() - start);
        if old == start && self.motion && self.rows.len() > start {
            self.transcript_reveal = Some(ListReveal::appended(start));
        }
    }
    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        div()
            .id("sidebar-layout-panel")
            .debug_selector(|| "project-rail".into())
            .on_click(cx.listener(|this,event:&ClickEvent,_,cx| {if event.click_count()==2 {this.toggle_sidebar_layout(cx);}}))
            .w(px(if self.sidebar_layout_transition.is_some() { self.sidebar_width } else { self.sidebar_target_width() }))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(gpui::rgba(self.resolved_theme.sidebar_rgba()))
            .border_r_1()
            .border_color(rgba(0xebebeb12))
            .child(
                div()
                    .h(px(40.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .px_3()
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .on_mouse_down(MouseButton::Left, |_, window, _| native::drag(window)),
                    )
                    .child(
                        div()
                            .id("collapse")
                            .cursor_pointer()
                            .p_1()
                            .rounded_md()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon("sidebar", 14., MUTED))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_sidebar();
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id("search")
                            .debug_selector(|| "search".into())
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .rounded_md()
                            .border_1()
                            .border_color(rgba(0xebebeb12))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon("search", 14., MUTED))
                            .child(div().flex_1().text_color(rgb(MUTED)).child("Search"))
                            .child(
                                div()
                                    .text_size(theme::ui_px(11.))
                                    .text_color(rgb(0x666666))
                                    .child("⌘K"),
                            )
                            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                cx.stop_propagation(); this.toggle_panel_from_click(Modal::Search, point(px(8.), event.position().y + px(20.)), false, window, cx)
                            })),
                    )
                    .child(
                        div()
                            .id("inbox")
                            .debug_selector(|| "inbox".into())
                            .h(px(32.))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .rounded_md()
                            .cursor_pointer()
                            .when(self.all_projects, |s| s.bg(rgba(GLASS_HOVER)))
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon("inbox", 16., MUTED))
                            .child("Priority")
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation(); this.all_projects = true; this.archived_view = false;
                                this.rebuild_workspace_sessions();
                                cx.notify();
                            })),
                    ),
            )
            .child(self.activity_section(cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(36.))
                    .px_3()
                    .mt_3()
                    .text_size(theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .debug_selector(|| "sidebar-layout-header".into())
                    .id("sidebar-layout-header")
                    .on_click(cx.listener(|this,e:&ClickEvent,_,cx| { if e.click_count()==1 && this.all_projects && !this.sidebar_split { cx.stop_propagation(); this.all_projects=false; this.rebuild_workspace_sessions(); cx.notify(); }}))
                    .when(self.all_projects && !self.sidebar_split, |s| s.child(icon("chevron-left",11.,MUTED)))
                    .child(if self.all_projects { "Priority" } else { "Projects" })
                    .child(div().flex_1())
                    .child(div().id("tree-archived").debug_selector(||"tree-archived".into()).p_1().cursor_pointer().child(icon("inbox",13.,if !self.project_tree.activity.is_empty() {ACCENT} else {MUTED})).on_click(cx.listener(|this,e:&ClickEvent,window,cx| {cx.stop_propagation(); this.toggle_panel_from_click(Modal::Activity, e.position() + point(px(-12.),px(14.)), false, window, cx);})))
                    .child(
                        div()
                            .id("add-project")
                            .p_1()
                            .cursor_pointer()
                            .child(icon("plus", 14., MUTED))
                            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                cx.stop_propagation(); this.toggle_panel_from_click(Modal::Projects, event.position(), false, window, cx)
                            })),
                    ),
            )
            .when(self.sidebar_split, |s| s.child(
                uniform_list(
                    "project-list",
                    self.projects.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        if let Some(reveal) = &mut this.project_reveal {
                            reveal.observe(range.clone());
                        }
                        let elapsed = this.project_reveal.as_ref().map(ListReveal::elapsed);
                        range
                            .map(|ix| {
                                let project = &this.projects[ix];
                                let menu_project = project.clone();
                                let path = project.path.clone();
                                let selected = !this.all_projects && this.project == path;
                                let entrance = elapsed.and_then(|elapsed| this.project_reveal.as_ref().map(|r| r.row(ix, elapsed)));
                                div()
                                    .id(("project", ix))
                                    .debug_selector(move || format!("project-{ix}"))
                                    .w_full()
                                    .min_w(px(0.))
                                    .h(px(32.))
                                    // UniformList positions the outer row itself. Move its
                                    // child so the spring changes paint, not list geometry.
                                    .child(div()
                                    .id(("project-content", ix))
                                    .debug_selector(move || format!("project-content-{ix}"))
                                    .group("project-row")
                                    .w_full()
                                    .min_w(px(0.))
                                    .h(px(32.))
                                    .when_some(entrance, |row, (opacity, offset)| {
                                        row.opacity(opacity)
                                            .relative()
                                            .left(px(8. * offset))
                                            .top(px(5. * offset))
                                    })
                                    .px_2()
                                    .overflow_hidden()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .line_height(theme::ui_line_height(20.))
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .text_color(rgb(if selected { TEXT } else { 0xb5b5b5 }))
                                    .child(
                                        icon(if this.lifecycle.pinned_projects.contains(&project.path) { "pin" } else { "folder" }, 15., if selected { TEXT } else { MUTED })
                                            .flex_none(),
                                    )
                                    .child(
                                        div()
                                            .debug_selector(move || format!("project-label-{ix}"))
                                            .flex_1()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .child(this.project_label(project)),
                                    )
                                    .child(
                                        div()
                                            .id(("project-menu", ix))
                                            .debug_selector(move || format!("project-menu-{ix}"))
                                            .size(px(20.))
                                            .flex_none()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded_md()
                                            .opacity(if selected { 1. } else { 0. })
                                            .group_hover("project-row", |s| s.opacity(1.))
                                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                            .child(icon("more", 14., MUTED))
                                            .on_click(cx.listener(
                                                move |this, event: &ClickEvent, window, cx| {
                                                    cx.stop_propagation();
                                                    this.modal = None;
                                                    if this.action_menu.as_ref().is_some_and(|menu| matches!(&menu.target, MenuTarget::Project(project) if project.path == menu_project.path)) {
                                                        this.dismiss_action_menu();
                                                    } else {
                                                        this.action_menu = Some(ActionMenu {
                                                            target: MenuTarget::Project(menu_project.clone()),
                                                            anchor: event.position()
                                                                + point(px(10.), px(-20.)),
                                                            started: Instant::now(),
                                                            closing: false,
                                                            closing_from: 1.,
                                                        });
                                                    }
                                                    this.focus.focus(window);
                                                    cx.notify();
                                                },
                                            )),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.select_project(path.clone(), window, cx)
                                    })))
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .px_2(),
            ))
            .when(!self.sidebar_split, |s| s.child(self.project_tree_list(cx)))
            .child(
                div()
                    .px_2()
                    .py_2()
                    .border_t_1()
                    .border_color(rgba(0xebebeb12))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .id("agents")
                            .h(px(32.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .children(self.agents.iter().map(|agent| {
                                let kind = agent.kind;
                                div()
                                    .debug_selector(move || {
                                        format!("agent-strip-{}", agent_icon(kind))
                                    })
                                    .size(px(14.))
                                    .flex_none()
                                    .opacity(if agent.available { 1. } else { 0.4 })
                                    .child(icon(agent_icon(kind), 14., agent_color(kind)))
                            }))
                            .child(div().flex_1())
                            .child(icon("chevron-right", 12., MUTED))
                            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                cx.stop_propagation(); this.toggle_agent_picker(event.position(), window, cx)
                            })),
                    )
                    .child(
                        div()
                            .id("settings")
                            .debug_selector(|| "settings".into())
                            .h(px(32.))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon("settings", 16., MUTED))
                            .child(div().text_color(rgb(MUTED)).child("Settings"))
                            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                cx.stop_propagation(); this.toggle_panel_from_click(Modal::Settings, point(px(8.), event.position().y - px(20.)), true, window, cx)
                            })),
                    ),
            )
    }
    fn workspace(&self, cx: &Context<Self>) -> impl IntoElement {
        let name = if self.all_projects {
            "Workspace".to_owned()
        } else {
            self.project_path_label(&self.project)
        };
        div()
            .id("workspace-layout-panel")
            .debug_selector(|| "workspace-pane".into())
            .on_click(cx.listener(|this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.toggle_sidebar_layout(cx);
                }
            }))
            .w(px(260.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(gpui::rgba(self.resolved_theme.sidebar_rgba()))
            .border_r_1()
            .border_color(rgba(0xebebeb12))
            .child(
                div()
                    .h(px(40.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgba(0xebebeb12))
                    .when(!self.sidebar, |s| {
                        s.child(
                            div()
                                .id("show-sidebar")
                                .debug_selector(|| "show-sidebar".into())
                                .size(px(28.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .child(icon("sidebar", 15., TEXT))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.toggle_sidebar();
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .overflow_hidden()
                            .text_ellipsis()
                            .on_mouse_down(MouseButton::Left, |_, window, _| native::drag(window))
                            .child(name),
                    )
                    .child(
                        div()
                            .id("workspace-search")
                            .debug_selector(|| "workspace-search".into())
                            .cursor_pointer()
                            .p_1()
                            .child(icon("search", 14., MUTED))
                            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.toggle_panel_from_click(
                                    Modal::Search,
                                    point(
                                        px(this.sidebar_width + 8.),
                                        event.position().y + px(20.),
                                    ),
                                    false,
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(
                        div()
                            .id("new")
                            .cursor_pointer()
                            .p_1()
                            .child(icon("plus", 16., MUTED))
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.new_conversation(window, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .h(px(36.))
                    .flex_none()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgba(0xebebeb12))
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(rgba(GLASS_HOVER))
                            .text_size(theme::ui_px(12.))
                            .child(if self.archived_view {
                                "Archived"
                            } else {
                                "Sessions"
                            }),
                    )
                    .child(
                        div()
                            .id("toggle-archived-chats")
                            .debug_selector(|| "toggle-archived-chats".into())
                            .p_1()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon(
                                "inbox",
                                14.,
                                if self.archived_view { ACCENT } else { MUTED },
                            ))
                            .on_click(cx.listener(|this, _, _, cx| {
                                cx.stop_propagation();
                                this.toggle_archived_view(cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(theme::ui_px(11.))
                            .text_color(rgb(MUTED))
                            .child(self.workspace_sessions.len().to_string()),
                    ),
            )
            .child(
                div()
                    .h(px(38.))
                    .flex_none()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(theme::ui_px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(icon("chevron-down", 12., MUTED))
                    .child(if self.all_projects {
                        "Recent conversations"
                    } else {
                        "Conversations"
                    }),
            )
            .child(
                uniform_list(
                    "sessions",
                    self.workspace_sessions.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        if let Some(reveal) = &mut this.sessions_reveal {
                            reveal.observe(range.clone());
                        }
                        let elapsed = this.sessions_reveal.as_ref().map(ListReveal::elapsed);
                        range
                            .map(|ix| {
                                let entrance = elapsed.and_then(|elapsed| {
                                    this.sessions_reveal.as_ref().map(|r| r.row(ix, elapsed))
                                });
                                div()
                                    .id(("session", ix))
                                    .debug_selector(move || format!("session-{ix}"))
                                    .w_full()
                                    .min_w(px(0.))
                                    .h(px(68.))
                                    .child(
                                        this.session_item(
                                            this.workspace_sessions[ix].clone(),
                                            ix,
                                            cx,
                                        )
                                        .when_some(
                                            entrance,
                                            |row, (opacity, offset)| {
                                                row.opacity(opacity)
                                                    .relative()
                                                    .left(px(8. * offset))
                                                    .top(px(5. * offset))
                                            },
                                        ),
                                    )
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .min_h(px(0.))
                .w_full()
                .px_2(),
            )
            .when(
                self.archived_view && self.archive_offset < self.archive_ids.len(),
                |s| {
                    s.child(
                        div()
                            .id("load-archived-page")
                            .debug_selector(|| "load-archived-page".into())
                            .mx_2()
                            .mb_2()
                            .px_2()
                            .h(px(30.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_size(theme::ui_px(11.))
                            .text_color(rgb(MUTED))
                            .bg(rgba(GLASS_SURFACE))
                            .child(if self.archive_loading {
                                "Loading archived chats…"
                            } else {
                                "Load more archived chats"
                            })
                            .when(!self.archive_loading, |s| {
                                s.cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.load_archived_page(cx)
                                    }))
                            }),
                    )
                },
            )
            .child(
                div()
                    .id("new-session")
                    .debug_selector(|| "new-session".into())
                    .h(px(36.))
                    .mx_2()
                    .mb_2()
                    .px_2()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(rgb(MUTED))
                    .hover(|s| s.bg(rgba(GLASS_HOVER)).text_color(rgb(TEXT)))
                    .child(icon("plus", 14., MUTED))
                    .child("New session")
                    .child(div().flex_1())
                    .child(div().text_size(theme::ui_px(11.)).child("⌘N"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        cx.stop_propagation();
                        this.new_conversation(window, cx)
                    })),
            )
    }
    fn session_item(
        &self,
        session: SessionSummary,
        ix: usize,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let menu_session = session.clone();
        let running = self.runs.contains_key(&session.id)
            || self
                .broker_children
                .get(&session.id)
                .is_some_and(|count| *count > 0);
        let selected = self.active.as_deref() == Some(&session.id);
        let seconds = chrono::Utc::now()
            .timestamp()
            .saturating_sub(session.updated_at)
            .max(0);
        let age = if seconds < 60 {
            "now".into()
        } else if seconds < 3600 {
            format!("{}m", seconds / 60)
        } else if seconds < 86400 {
            format!("{}h", seconds / 3600)
        } else {
            format!("{}d", seconds / 86400)
        };
        let project = self.project_path_label(&session.project_path);
        div()
            .id(("session-content", ix))
            .group("session-row")
            .debug_selector(move || format!("session-content-{ix}"))
            .w_full()
            .min_w(px(0.))
            .h(px(68.))
            .px(px(10.))
            .py(px(6.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .overflow_hidden()
            .rounded_md()
            .cursor_pointer()
            .when(selected, |s| s.bg(rgba(0xffffff14)))
            .hover(|s| s.bg(rgba(GLASS_HOVER)))
            .child(
                div()
                    .debug_selector(move || format!("session-provider-{ix}"))
                    .w_full()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .text_size(theme::ui_px(11.))
                    .line_height(theme::ui_line_height(16.))
                    .text_color(rgb(MUTED))
                    .child(
                        icon(agent_icon(session.agent), 12., agent_color(session.agent))
                            .flex_none(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(agent_name(session.agent)),
                    )
                    .when(self.lifecycle.pinned_chats.contains(&session.id), |s| s.child(icon("pin", 10., MUTED).flex_none()))
                    .child(div().flex_none().child(age))
                    .child(
                        div()
                            .id(("session-menu", ix))
                            .debug_selector(move || format!("session-menu-{ix}"))
                            .size(px(16.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_sm()
                            .opacity(if selected { 1. } else { 0. })
                            .group_hover("session-row", |s| s.opacity(1.))
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon("more", 12., MUTED))
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                cx.stop_propagation();
                                this.modal = None;
                                if this.action_menu.as_ref().is_some_and(|menu| matches!(&menu.target, MenuTarget::Chat(session) if session.id == menu_session.id)) {
                                    this.dismiss_action_menu();
                                } else {
                                    this.action_menu = Some(ActionMenu {
                                        target: MenuTarget::Chat(menu_session.clone()),
                                        anchor: event.position() + point(px(10.), px(-20.)),
                                        started: Instant::now(),
                                        closing: false,
                                        closing_from: 1.,
                                    });
                                }
                                this.focus.focus(window);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .debug_selector(move || format!("session-title-{ix}"))
                    .w_full()
                    .min_w(px(0.))
                    .text_size(theme::ui_px(13.))
                    .line_height(theme::ui_line_height(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.chat_label(&session)),
            )
            .child(
                div()
                    .debug_selector(move || format!("session-project-{ix}"))
                    .w_full()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(theme::ui_px(11.))
                    .line_height(theme::ui_line_height(16.))
                    .text_color(rgb(0x777777))
                    .child(icon("branch", 12., 0x777777).flex_none())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(project),
                    )
                    .when(running, |s| s.child(icon("sparkles", 12., ACCENT))),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.open_session(session.clone(), window, cx)
            }))
    }
    fn tabs(&self, cx: &Context<Self>) -> impl IntoElement {
        let tabs: Vec<_> = self
            .open_tabs
            .iter()
            .filter_map(|id| self.sessions.iter().find(|s| &s.id == id))
            .collect();
        div()
            .h(px(40.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.))
            .px_2()
            .border_b_1()
            .border_color(rgba(0xebebeb12))
            .overflow_hidden()
            .when(tabs.is_empty(), |s| {
                s.child(
                    div()
                        .h(px(30.))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_md()
                        .bg(rgba(GLASS_HOVER))
                        .text_size(theme::ui_px(12.))
                        .child(icon(agent_icon(self.agent), 14., agent_color(self.agent)))
                        .child("New session"),
                )
            })
            .children(tabs.into_iter().enumerate().map(|(ix, session)| {
                let session = session.clone();
                let id = session.id.clone();
                let selected = self.active.as_deref() == Some(&id);
                let closing_id = id.clone();
                let tab = div()
                    .id(("tab", ix))
                    .h(px(30.))
                    .max_w(px(230.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |s| s.bg(rgba(0xffffff20)))
                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                    .child(icon(
                        agent_icon(session.agent),
                        13.,
                        agent_color(session.agent),
                    ))
                    .child(
                        div()
                            .text_size(theme::ui_px(12.))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(self.chat_label(&session)),
                    )
                    .child(
                        div()
                            .id(("close-tab", ix))
                            .p_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xffffff15)))
                            .child(icon("close", 11., MUTED))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(&closing_id, window, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_session(session.clone(), window, cx)
                    }));
                if self.motion {
                    tab.with_animation(
                        SharedString::from(format!("tab-enter-{id}")),
                        Animation::new(Duration::from_millis(400)),
                        |s, p| {
                            let p = presentation::spring(p);
                            s.opacity(p.clamp(0., 1.)).max_w(px(210. + 20. * p))
                        },
                    )
                    .into_any_element()
                } else {
                    tab.into_any_element()
                }
            }))
    }
    fn home(&self, _cx: &Context<Self>) -> impl IntoElement {
        let greeting = div()
            .flex()
            .flex_col()
            .items_center()
            .gap_3()
            .child(self.loading_logo.clone())
            .child(
                div()
                    .text_size(theme::ui_px(25.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("What would you like to build?"),
            );
        let greeting = if self.motion {
            greeting
                .with_animation(
                    ("welcome", self.home_generation),
                    spring_animation(),
                    |s, p| {
                        let p = presentation::spring(p);
                        s.opacity(p.clamp(0., 1.)).mt(px(12. * (1. - p)))
                    },
                )
                .into_any_element()
        } else {
            greeting.into_any_element()
        };
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .items_center()
            .justify_center()
            .child(greeting)
    }
    /// Composer selection is only the destination for the next submission.
    /// Legacy transcript rows belong to the bound chat (or its active run).
    fn transcript_agent(&self) -> AgentKind {
        self.active
            .as_ref()
            .and_then(|id| {
                self.runs.get(id).map(|run| run.agent).or_else(|| {
                    self.sessions
                        .iter()
                        .find(|session| &session.id == id)
                        .map(|session| session.agent)
                })
            })
            .unwrap_or(self.agent)
    }
    fn row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        #[cfg(test)]
        self.rendered_rows.set(self.rendered_rows.get() + 1);
        if let Some(deferred) = self.deferred_rows.get(&ix) {
            let page = deferred.page;
            let height = deferred.height;
            self.request_history_page(page);
            return div()
                .w_full()
                .h(height)
                .px_4()
                .text_size(theme::ui_px(12.))
                .text_color(rgb(MUTED))
                .child(if self.history_failed.contains(&page) {
                    "Page unavailable. Reopen this conversation to refresh."
                } else {
                    "Loading messages…"
                })
                .into_any_element();
        }
        let page_index = self
            .history_pages
            .partition_point(|p| p.start <= ix)
            .saturating_sub(1);
        if let Some(page) = self.history_pages.get_mut(page_index)
            && ix < page.start + page.len
        {
            self.history_clock += 1;
            page.used = self.history_clock;
        }
        if matches!(self.rows.get(ix).map(|row|&row.block),Some(MessageBlock::Interaction(chat_interactions::ChatInteraction::Status {title,status:chat_interactions::InteractionStatus::Completed,..})) if title=="Completed")
        {
            self.prefetch_completion_action(ix, cx);
        }
        let decode_source=self.rows.get(ix).and_then(|row|{
            if row.role.as_deref().is_some_and(|role|role.starts_with("user")){return None}
            match &row.block{MessageBlock::Paragraph(text)|MessageBlock::Heading(text)|MessageBlock::Quote(text)|MessageBlock::RichParagraph{text,..}|MessageBlock::ListItem{text,..}=>Some(text.clone()),_=>None}
        });
        let decoded=if unified::decode_enabled(self.motion && self.unified_text_morph)&&unified::workspace_frame(self.motion).content_visible(){decode_source.as_ref().map(|source|{
            let (frame,active)=self.unified_text_decode.frame(ix,source,Instant::now());if active{window.request_animation_frame();}frame
        })}else{None};
        let Some(row) = self.rows.get(ix) else {
            return div().into_any_element();
        };
        // Retain the app-owned boundary for exact prompt attribution, without
        // rendering a second Undo control above the completion card.
        if checkpoint_ui::row_checkpoint(row).is_some() {
            return div().h(px(0.)).into_any_element();
        }
        let entrance = self.transcript_reveal.as_mut().map(|reveal| {
            reveal.observe(ix..ix + 1);
            reveal.row(ix, reveal.elapsed())
        });
        let geometry_probe = self
            .transcript_reveal
            .as_ref()
            .and_then(|r| r.geometry_probe(ix));
        let link_owner = cx.weak_entity();
        let list_link_owner = link_owner.clone();
        let user = row.role.as_deref() == Some("user");
        let row_agent = agent_for_role(row.role.as_deref(), self.transcript_agent());
        let mut content = div()
            .debug_selector(move || format!("transcript-content-{ix}"))
            .w_full()
            .min_w(px(0.))
            .max_w(px(800.))
            .py(px(4.));
        if let Some(role) = &row.role
            && row_has_header(Some(role))
        {
            content = content.mt(px(24.)).child(
                div()
                    .debug_selector(move || {
                        format!(
                            "transcript-provider-{ix}-{}",
                            if user { "user" } else { row_agent.command() }
                        )
                    })
                    .flex()
                    .items_center()
                    .gap_2()
                    .mb_2()
                    .text_size(theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .when(role != "user", |s| {
                        s.child(icon(agent_icon(row_agent), 14., agent_color(row_agent)))
                    })
                    .child(if role == "user" {
                        "You"
                    } else {
                        agent_name(row_agent)
                    }),
            );
        }
        content = match &row.block {
            MessageBlock::Voice {
                speaker,
                text,
                highlights,
            } => content.child(
                div()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgba(0xebebeb16))
                    .bg(rgba(self.appearance.tint(GLASS_SURFACE)))
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .mb_1()
                            .text_size(theme::ui_px(10.))
                            .text_color(rgb(MUTED))
                            .child(icon("assets/kai/status/listening.svg", 16., MUTED))
                            .child(if *speaker == chat_interactions::Speaker::User {
                                "You"
                            } else {
                                agent_name(row_agent)
                            }),
                    )
                    .child(
                        div()
                            .text_size(theme::ui_px(14.))
                            .line_height(theme::ui_line_height(23.))
                            .child(rich_text(
                                ("voice", ix),
                                decoded.as_deref().unwrap_or(text),
                                highlights,
                                move |url, window, cx| {
                                    let _ = link_owner
                                        .update(cx, |this, cx| this.open_citation(url, window, cx));
                                },
                            )),
                    ),
            ),
            MessageBlock::Interaction(card) => {
                content.child(self.interaction_card(ix, card, row_agent, cx))
            }
            MessageBlock::Attachment { name, target } => {
                content.child(self.attachment_history_card(ix, name, target, cx))
            }
            MessageBlock::Paragraph(text) => content.child(
                div()
                    .text_size(theme::ui_px(14.))
                    .line_height(theme::ui_line_height(24.))
                    .text_color(rgb(TEXT))
                    .when(user, |s| {
                        s.bg(rgba(self.appearance.tint(GLASS_SURFACE)))
                            .rounded_lg()
                            .px_4()
                            .py_3()
                    })
                    .child(decoded.clone().unwrap_or_else(||text.clone())),
            ),
            MessageBlock::RichParagraph { text, highlights } => content.child(
                div()
                    .text_size(theme::ui_px(14.))
                    .line_height(theme::ui_line_height(23.))
                    .when(user, |s| {
                        s.bg(rgba(self.appearance.tint(GLASS_SURFACE)))
                            .rounded_lg()
                            .px_4()
                            .py_3()
                    })
                    .child(rich_text(
                        ("markdown", ix),
                        decoded.as_deref().unwrap_or(text),
                        highlights,
                        move |url, window, cx| {
                            let _ = link_owner
                                .update(cx, |this, cx| this.open_citation(url, window, cx));
                        },
                    )),
            ),
            MessageBlock::ListItem {
                marker,
                text,
                highlights,
            } => content.child(
                div()
                    .flex()
                    .gap_2()
                    .text_size(theme::ui_px(14.))
                    .line_height(theme::ui_line_height(23.))
                    .child(
                        div()
                            .w(px(24.))
                            .flex_none()
                            .text_color(rgb(MUTED))
                            .child(marker.clone()),
                    )
                    .child(div().flex_1().min_w(px(0.)).child(rich_text(
                        ("list-text", ix),
                        decoded.as_deref().unwrap_or(text),
                        highlights,
                        move |url, window, cx| {
                            let _ = list_link_owner
                                .update(cx, |this, cx| this.open_citation(url, window, cx));
                        },
                    ))),
            ),
            MessageBlock::TableRow { cells, header } => content.child(
                div()
                    .flex()
                    .w_full()
                    .border_b_1()
                    .border_color(rgba(0xebebeb12))
                    .when(*header, |s| {
                        s.font_weight(FontWeight::SEMIBOLD).bg(rgba(0xffffff08))
                    })
                    .children(cells.iter().map(|cell| {
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .px_3()
                            .py_2()
                            .text_size(theme::ui_px(13.))
                            .line_height(theme::ui_line_height(21.))
                            .child(cell.clone())
                    })),
            ),
            MessageBlock::Activity { label, details } => {
                let expanded = self.expanded_tools.contains(&ix);
                content.child(
                    div()
                        .rounded_md()
                        .border_1()
                        .border_color(rgba(0xebebeb12))
                        .bg(rgba(self.appearance.tint(GLASS_SURFACE)))
                        .child(
                            div()
                                .id(("activity", ix))
                                .debug_selector(move || format!("activity-{ix}"))
                                .h(px(34.))
                                .px_3()
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .text_size(theme::ui_px(12.))
                                .text_color(rgb(MUTED))
                                .child(icon(activity_icon(label), 14., MUTED))
                                .child(label.clone())
                                .child(div().flex_1())
                                .child(icon(
                                    if expanded {
                                        "chevron-down"
                                    } else {
                                        "chevron-right"
                                    },
                                    12.,
                                    MUTED,
                                ))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.expanded_tools.remove(&ix) {
                                        this.expanded_tools.insert(ix);
                                    }
                                    cx.notify();
                                })),
                        )
                        .when(expanded, |s| {
                            s.child(
                                div()
                                    .px_3()
                                    .pb_3()
                                    .font_family(theme::mono_font())
                                    .text_size(theme::mono_px(11.))
                                    .line_height(theme::mono_line_height(18.))
                                    .text_color(rgb(MUTED))
                                    .child(details.clone()),
                            )
                        }),
                )
            }
            MessageBlock::Heading(text) => content.child(
                div()
                    .text_size(theme::ui_px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(decoded.clone().unwrap_or_else(||text.clone())),
            ),
            MessageBlock::Quote(text) => content.child(
                div()
                    .border_l_2()
                    .border_color(rgb(0x565f50))
                    .pl_4()
                    .text_size(theme::ui_px(14.))
                    .text_color(rgb(MUTED))
                    .child(decoded.clone().unwrap_or_else(||text.clone())),
            ),
            MessageBlock::Code {
                language,
                text,
                highlights,
            } => {
                let copy = text.clone();
                content.child(
                    div()
                        .debug_selector(move || format!("code-card-{ix}"))
                        .w_full()
                        .min_w(px(0.))
                        .rounded_lg()
                        .overflow_hidden()
                        .bg(rgba(self.appearance.tint(GLASS_SURFACE)))
                        .border_1()
                        .border_color(rgba(0xebebeb12))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_4()
                                .h(px(32.))
                                .border_b_1()
                                .border_color(rgba(0xebebeb12))
                                .text_size(theme::ui_px(10.))
                                .text_color(rgb(MUTED))
                                .child(if language.is_empty() {
                                    "code".to_string()
                                } else {
                                    language.clone()
                                })
                                .child(
                                    div()
                                        .id(("copy", ix))
                                        .cursor_pointer()
                                        .child("Copy")
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy.clone(),
                                            ))
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .id(("code", ix))
                                .w_full()
                                .min_w(px(0.))
                                .overflow_x_scroll()
                                .px_4()
                                .py_3()
                                .font_family(theme::mono_font())
                                .text_size(theme::mono_px(12.))
                                .line_height(theme::mono_line_height(20.))
                                .text_color(rgb(TEXT))
                                .child(StyledText::new(text.clone()).with_highlights(
                                    highlights.iter().map(|(range, color)| {
                                        (
                                            range.clone(),
                                            HighlightStyle {
                                                color: Some(rgb(*color).into()),
                                                ..Default::default()
                                            },
                                        )
                                    }),
                                )),
                        ),
                )
            }
        };
        div()
            .debug_selector(move || format!("transcript-row-{ix}"))
            .flex()
            .justify_center()
            .w_full()
            .relative()
            .px_4()
            .when_some(geometry_probe, |root, visible| {
                root.child(
                    canvas(
                        move |bounds, window, _| {
                            if bounds.intersects(&window.content_mask().bounds) {
                                ListReveal::record(&visible, ix, ix);
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
            })
            .child(content.when_some(entrance, |content, (opacity, offset)| {
                content
                    .opacity(opacity)
                    .relative()
                    .left(px(8. * offset))
                    .top(px(5. * offset))
            }))
            .into_any_element()
    }
    fn composer(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let running = self
            .active
            .as_ref()
            .is_some_and(|id| self.runs.contains_key(id));
        let project = self
            .project
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        div()
            .flex_none()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .px_4()
            .pt_3()
            .pb_3()
            .gap_2()
            .when(self.owns_overlays(window), |s| {
                s.child(self.native_request_tray(cx))
            })
            .child(self.queue_strip(cx))
            .child(div().w_full().max_w(px(800.)).child(self.context_chips(cx)))
            .when(self.notice.is_some(), |s| {
                s.child(
                    div()
                        .w_full()
                        .max_w(px(800.))
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgba(0x8b742526))
                        .text_color(rgb(0xddc9ac))
                        .text_size(theme::ui_px(12.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div().flex_1().min_w(px(0.)).text_ellipsis().child(short(
                                &self
                                    .notice
                                    .as_deref()
                                    .unwrap_or_default()
                                    .replace(['\n', '\r'], " "),
                                180,
                            )),
                        )
                        .when(self.recently_removed.is_some(), |s| {
                            s.child(
                                div()
                                    .id("undo-project-removal")
                                    .debug_selector(|| "undo-project-removal".into())
                                    .cursor_pointer()
                                    .text_color(rgb(ACCENT))
                                    .child("Undo")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.restore_project(cx)),
                                    ),
                            )
                        })
                        .when(self.recently_deleted.is_some(), |s| {
                            s.child(
                                div()
                                    .id("undo-chat-deletion")
                                    .debug_selector(|| "undo-chat-deletion".into())
                                    .cursor_pointer()
                                    .text_color(rgb(ACCENT))
                                    .child("Undo")
                                    .on_click(cx.listener(|this, _, _, cx| this.restore_chat(cx))),
                            )
                        }),
                )
            })
            .child(
                div()
                    .w_full()
                    .max_w(px(800.))
                    .id("composer-card")
                    .debug_selector(|| "composer-card".into())
                    .drag_over::<ExternalPaths>(|s, _, _, _| {
                        s.border_color(rgb(ACCENT)).bg(rgba(0x459bf718))
                    })
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
                        this.add_attachments(
                            paths
                                .paths()
                                .iter()
                                .take(attachments::MAX_ATTACHMENTS + 1)
                                .cloned()
                                .collect(),
                            cx,
                        )
                    }))
                    .rounded_lg()
                    .bg(rgba(self.appearance.tint(0xebebeb08)))
                    .border_1()
                    .border_color(rgba(0xebebeb1a))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .h(px(30.))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(theme::ui_px(11.))
                            .text_color(rgb(MUTED))
                            .child(icon("branch", 14., MUTED))
                            .child(project),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .min_h(px(48.))
                            .max_h(px(160.))
                            .w_full()
                            .child(self.composer.clone()),
                    )
                    .child(self.attachment_strip(cx))
                    .child(
                        div()
                            .px_2()
                            .pb_2()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .id("choose-project")
                                    .size(px(26.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .bg(rgba(0xffffff14))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(0xebebeb24)))
                                    .child(icon("folder", 14., MUTED))
                                    .on_click(cx.listener(
                                        |this, event: &ClickEvent, window, cx| {
                                            this.toggle_panel_from_click(
                                                Modal::Projects,
                                                event.position() + point(px(-12.), px(-20.)),
                                                true,
                                                window,
                                                cx,
                                            )
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .id("choose-agent")
                                    .h(px(26.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .rounded_md()
                                    .bg(rgba(0xffffff14))
                                    .text_size(theme::ui_px(11.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(0xebebeb24)))
                                    .child(icon(
                                        agent_icon(self.agent),
                                        14.,
                                        agent_color(self.agent),
                                    ))
                                    .child(
                                        self.effective_model_label(),
                                    )
                                    .child(icon("chevron-down", 10., MUTED))
                                    .on_click(cx.listener(
                                        |this, event: &ClickEvent, window, cx| {
                                            this.toggle_agent_picker(event.position(), window, cx)
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .id("mode")
                                    .debug_selector(|| "mode".into())
                                    .h(px(26.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .rounded_md()
                                    .bg(rgba(0xffffff14))
                                    .text_size(theme::ui_px(11.))
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(0xebebeb24)))
                                    .child(icon(
                                        if self.allow_edits {
                                            "assets/kai/status/code.svg"
                                        } else {
                                            "assets/kai/status/plan.svg"
                                        },
                                        16.,
                                        MUTED,
                                    ))
                                    .child(if self.allow_edits {
                                        "Build"
                                    } else {
                                        "Read only"
                                    })
                                    .child(icon("chevron-down", 10., MUTED))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.show_modal(Modal::Permissions, window, cx);
                                    })),
                            )
                            .child(div().flex_1())
                            .when(runtime::supports_native_delegation(self.agent), |s| {
                                s.child(
                                    div()
                                        .id("delegate-task")
                                        .debug_selector(|| "delegate-task".into())
                                        .size(px(26.))
                                        .rounded_md()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                        .child(icon("assets/kai/status/agents.svg", 16., MUTED))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            let draft = this.composer.read(cx).text();
                                            if commands::name(draft.trim()) != Some("delegate") {
                                                this.composer.update(cx, |input, cx| {
                                                    input
                                                        .set_text(&format!("/delegate {draft}"), cx)
                                                });
                                            }
                                            this.composer.read(cx).focus_handle.focus(window);
                                            cx.notify();
                                        })),
                                )
                            })
                            .when(
                                self.agent == AgentKind::Codex || self.voice_call.is_some(),
                                |view| view.child(self.voice_button(cx)),
                            )
                            .child(self.attachment_button(cx))
                            .child(self.dictation_button(cx))
                            .child(
                                div()
                                    .id("send")
                                    .size(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .bg(gpui::rgb(self.resolved_theme.accent))
                                    .cursor_pointer()
                                    .hover(|s| s.opacity(0.88))
                                    .child(
                                        icon(
                                            if (running
                                                && self.composer.read(cx).text().trim().is_empty()
                                                && self.attachments.is_empty())
                                                || self.handoff_pending.is_some()
                                            {
                                                "stop"
                                            } else {
                                                "arrow"
                                            },
                                            16.,
                                            self.resolved_theme.on_accent,
                                        )
                                        .text_color(theme::on_accent()),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if !this.cancel_send_handoff(cx) {
                                            this.send(window, cx);
                                        }
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(800.))
                    .flex()
                    .justify_between()
                    .text_size(theme::ui_px(10.))
                    .text_color(rgb(0x707070))
                    .child(self.dictation_status.unwrap_or(if running {
                        "Working locally"
                    } else {
                        "Local session"
                    }))
                    .child(
                        div()
                            .id("context-usage")
                            .debug_selector(|| "context-usage".into())
                            .cursor_pointer()
                            .hover(|s| s.text_color(rgb(TEXT)))
                            .child("Status · context")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_modal(Modal::Status, window, cx)
                            })),
                    )
                    .child(if running {
                        "↵ Queue · /steer Send now"
                    } else {
                        "↵ Send · ⇧↵ New line"
                    }),
            )
    }
}
fn is_dark(appearance: WindowAppearance) -> bool {
    matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}
impl Orbit {
    fn advance_chat_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_sidebar_activity(cx);
        self.prepare_native_requests(window, cx);
        self.advance_voice(cx);
        self.advance_handoff(window, cx);
        self.advance_queue(window, cx);
        self.advance_teams(cx);
        self.tick_slash(window, cx);
        self.tick_context_picker(window, cx);
        self.refresh_slash(window, cx);
        if self.session_conflict_open {
            self.session_conflict_open = false;
            if self
                .session_conflict
                .as_ref()
                .is_some_and(|context| self.workspace_matches(context))
            {
                self.show_modal(Modal::SessionConflict, window, cx);
            }
        }
        if self.workspace_trust_open {
            self.workspace_trust_open = false;
            if self
                .workspace_trust
                .as_ref()
                .is_some_and(|trust| self.workspace_matches(trust))
            {
                self.show_modal(Modal::WorkspaceTrust, window, cx);
            }
        }
        if let Some(trust) = &self.workspace_retry {
            if !self.workspace_matches(trust) {
                self.workspace_retry = None;
                self.fork_next = None;
            } else if !self.finalizing.contains(&trust.session)
                && !self.starting
                && !self.loading
                && !self.loading_earlier
            {
                self.workspace_retry = None;
                self.send(window, cx);
            }
        }
    }
    fn effective_appearance(&self) -> appearance::Appearance {
        appearance::Appearance {
            blur_radius: if self.resolved_theme.native_transparency {
                self.appearance.blur_radius
            } else {
                0
            },
            ..self.appearance
        }
    }
    fn owns_overlays(&self, window: &Window) -> bool {
        self.overlay_window
            .is_none_or(|id| id == window.window_handle().window_id())
    }
    fn apply_personalization(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let resolved = self
            .theme
            .resolve(is_dark(window.appearance()), self.appearance);
        if self.resolved_theme != resolved {
            if self.resolved_theme.ui_font != resolved.ui_font
                || self.resolved_theme.mono_font != resolved.mono_font
                || self.resolved_theme.ui_font_size != resolved.ui_font_size
                || self.resolved_theme.mono_font_size != resolved.mono_font_size
            {
                let anchor = self.list.logical_scroll_top();
                self.list.reset(self.rows.len());
                self.list.scroll_to(anchor);
                if let Some(browser) = &self.file_browser {
                    browser.update(cx, |browser, cx| browser.invalidate_theme_metrics(cx));
                }
            }
            self.resolved_theme = resolved.clone();
            theme::install_resolved(resolved);
            // Cached text/virtualized rows need a new paint, not new parsing.
            cx.refresh_windows();
        }
        native::apply_appearance(window, &self.effective_appearance());
        native::apply_theme(window, &self.resolved_theme);
        cx.notify();
    }
}
impl Render for Orbit {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use workspace_motion::Part as WorkspacePart;
        let entrance=unified::workspace_frame(self.motion);
        if entrance.animating(){request_motion_frame(window,cx);}
        let broker_project = self
            .broker_panel
            .as_ref()
            .map(|p| p.project.clone())
            .unwrap_or_else(|| {
                self.execution_root()
                    .unwrap_or_else(|| self.project.clone())
            });
        if self.broker_project != broker_project {
            self.refresh_broker(cx);
        }
        self.restore_workspace(window, cx);
        if self.owns_overlays(window) {
            self.advance_chat_surface(window, cx);
        }
        self.track_workspace(cx);
        self.tick_list_reveals(window, cx);
        let project = self.project.clone();
        let empty = self.active.is_none();
        let active = window.is_window_active() && self.motion;
        let quitting = self.quit_pending;
        self.background.update(cx, |background, cx| {
            background.set_quitting(quitting, cx);
            background.set_project(project, cx);
            background.set_context(empty, active, cx);
        });
        #[cfg(test)]
        self.rendered_rows.set(0);
        self.tick_project_tree(window, cx);
        if let Some((started, from)) = self.sidebar_transition {
            let t = started.elapsed().as_secs_f32() / 0.65;
            let target = if self.sidebar {
                self.sidebar_target_width()
            } else {
                0.
            };
            if t >= 1. || !self.motion {
                self.sidebar_width = target;
                self.sidebar_transition = None;
            } else {
                self.sidebar_width = (from + (target - from) * presentation::spring(t))
                    .clamp(0., from.max(target) + 16.);
                request_motion_frame(window, cx);
            }
        }
        self.tick_composer_controls(window, cx);
        self.tick_panel_popover(window, cx);
        if let Some(picker) = &self.agent_picker {
            if picker.animating() && self.motion {
                request_motion_frame(window, cx);
            } else if picker.closing {
                self.agent_picker = None;
            }
        }
        if let Some(menu) = &self.action_menu {
            let duration = if menu.closing { 120 } else { 320 };
            if !self.motion || menu.started.elapsed() >= Duration::from_millis(duration) {
                if menu.closing {
                    self.action_menu = None;
                }
            } else {
                request_motion_frame(window, cx);
            }
        }
        if self
            .checkpoint_timeline
            .as_ref()
            .is_some_and(|timeline| timeline.animating())
            && self.motion
        {
            request_motion_frame(window, cx);
        }
        if self
            .broker_panel
            .as_ref()
            .is_some_and(|panel| panel.animating())
            && self.motion
        {
            request_motion_frame(window, cx);
        }
        if let Some(editor) = &self.lifecycle_editor
            && self.motion
            && editor.started.elapsed() < Duration::from_millis(320)
        {
            request_motion_frame(window, cx);
        }
        if let Some(started) = self.history_animation {
            if !self.motion || started.elapsed() >= Duration::from_millis(400) {
                self.history_animation = None;
            } else {
                request_motion_frame(window, cx);
            }
        }
        let title = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|s| &s.id == id))
            .map(|s| self.chat_label(s))
            .unwrap_or("New conversation".into());
        let status = self
            .active
            .as_ref()
            .and_then(|id| self.runs.get(id))
            .map(|r| r.status.clone());
        let listening = self.speech.is_some()
            && self
                .dictation_status
                .is_some_and(|status| status.starts_with("Listening"));
        let waiting_for_native = self.active.as_ref().and_then(|owner| {
            self.native_pending
                .values()
                .find(|request| &request.owner == owner)
        });
        let live_scene = if let Some(request) = waiting_for_native {
            request.indicator_scene()
        } else if let Some(call) = &self.voice_call {
            call.scene()
        } else if self.handoff_pending.is_some() {
            kai_icons::scene_for_status("Delegating")
        } else if self.speech.is_some() {
            if listening { "listening" } else { "permission" }
        } else if self.starting {
            "thinking"
        } else if let Some(status) = &status {
            kai_icons::scene_for_status(status)
        } else {
            self.indicator_terminal
                .as_ref()
                .filter(|(id, _)| self.active.as_ref() == Some(id))
                .map_or("waiting", |(_, scene)| *scene)
        };
        let looping = self.voice_call.is_some()
            || listening
            || self.starting
            || status.is_some()
            || self.handoff_pending.is_some();
        // Mount the single live player beside its status caption, where the
        // activity is shown. Mounting it only in the titlebar left that caption
        // without an indicator. Idle/terminal states return to the titlebar.
        let live_activity = self.voice_call.is_some()
            || self.speech.is_some()
            || self.starting
            || status.is_some()
            || self.handoff_pending.is_some();
        let mark = if self.voice_call.is_some() || self.speech.is_some() || self.starting {
            // Dictation and launch feedback take priority over a running tool's
            // brand: its opaque mark would otherwise hide the live waveform.
            None
        } else if let Some(status) = &status {
            if [
                "failure",
                "declined",
                "permission",
                "question",
                "warning",
                "success",
            ]
            .contains(&live_scene)
            {
                None
            } else {
                kai_icons::integration_for_label(status).map(kai_icons::integration_asset)
            }
        } else if !looping && live_scene == "waiting" {
            Some(provider_asset(self.agent))
        } else {
            None
        };
        let indicator_motion = self.motion && !native::reduced_motion();
        self.navigation.update(cx, |rail, cx| {
            rail.set_motion(indicator_motion && window.is_window_active(), cx)
        });
        let logo_allowed = self.active.is_none() || !live_activity;
        let logo_busy = logo_allowed
            && (self.discovering
                || self.loading
                || if self.active.is_none() {
                    self.project_reveal.is_some() || self.sessions_reveal.is_some()
                } else {
                    self.transcript_reveal.is_some()
                });
        self.loading_logo.update(cx, |logo, cx| {
            logo.configure(
                loading_logo::Config {
                    path: provider_asset(self.agent),
                    color: agent_color(self.agent),
                    size: if self.active.is_none() { 34 } else { 18 },
                    busy: logo_busy,
                    motion: indicator_motion && logo_allowed,
                    awake: window.is_window_active() && logo_allowed,
                },
                cx,
            )
        });
        let header_loading_logo = self.active.is_some()
            && logo_allowed
            && (logo_busy || self.loading_logo.read(cx).moving());
        let inspector_open =
            self.files_visible || self.diff_view.is_some() || self.broker_panel.is_some();
        let diff_progress = self.diff_started.map_or(1., |started| {
            let t = started.elapsed().as_secs_f32() / 0.32;
            if !indicator_motion || t >= 1. {
                self.diff_started = None;
                1.
            } else {
                request_motion_frame(window, cx);
                presentation::spring(t).clamp(0., 1.)
            }
        });
        self.live_indicator.update(cx, |indicator, cx| {
            indicator.set_awake(window.is_window_active() && indicator_motion, cx);
            indicator.set_scene(
                live_scene,
                indicator_motion,
                looping && indicator_motion,
                cx,
            );
            indicator.set_mark(mark, indicator_motion, cx);
            indicator.set_levels(
                self.speech
                    .as_ref()
                    .filter(|_| listening)
                    .map(|handle| handle.levels.clone())
                    .or_else(|| {
                        self.voice_call
                            .as_ref()
                            .filter(|call| call.active && !call.muted)
                            .map(|call| call.media.levels.clone())
                    }),
                cx,
            );
        });
        self.file_indicator.update(cx, |indicator, cx| {
            indicator.set_awake(window.is_window_active() && indicator_motion, cx);
            indicator.set_open(inspector_open, indicator_motion, cx);
        });
        div()
            .key_context("Orbit")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .relative()
            .font_family(theme::ui_font())
            .text_color(rgb(TEXT))
            .text_size(theme::ui_px(13.))
            .on_modifiers_changed(
                cx.listener(|this, event: &ModifiersChangedEvent, window, cx| {
                    if !window.is_window_active() {
                        this.dictation_hotkey_down = false;
                        return;
                    }
                    if composer_controls::dictation_hotkey(
                        &mut this.dictation_hotkey_down,
                        event.modifiers,
                    ) {
                        this.toggle_dictation(window, cx);
                        cx.stop_propagation();
                    }
                }),
            )
            .capture_any_mouse_down(cx.listener(|this, _, _, _| {
                // Outside-dismiss handlers run later in capture. Preserve the
                // panel for this click so its own trigger can still toggle it.
                this.panel_at_pointer_down = this.modal;
            }))
            .on_action(cx.listener(|this, _: &NewConversation, window, cx| {
                this.new_conversation(window, cx)
            }))
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                cx.stop_propagation();
                this.quit_after_save(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSearch, window, cx| {
                this.show_modal(Modal::Search, window, cx)
            }))
            .on_action(cx.listener(|this, _: &input::SaveFile, window, cx| {
                this.show_modal(Modal::Search, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.toggle_sidebar();
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &ToggleConversations, _, cx| this.toggle_conversations(cx)),
            )
            .on_action(cx.listener(|_, _: &ReturnToNotch, _, _| {
                unified::collapse();
            }))
            .on_action(cx.listener(|this, _: &ToggleSettings, window, cx| {
                this.show_modal(Modal::Settings, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleDiffs, _, cx| this.toggle_diffs(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleFiles, window, cx| this.toggle_files(window, cx)),
            )
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                if this.context_picker.take().is_some() || this.team_panel.take().is_some() {
                    this.composer.read(cx).focus_handle.focus(window);
                    cx.notify();
                    return;
                }
                if this.slash.as_ref().is_some_and(|p| !p.closing) {
                    this.dismiss_slash(true, cx);
                    return;
                }
                this.diff_view = None;
                this.diff_observer = None;
                this.diff_started = None;
                this.close_attachment_preview(cx);
                this.dismiss_agent_picker();
                this.dismiss_panel(window, cx);
                this.dismiss_action_menu();
                this.lifecycle_editor = None;
                this.checkpoint_timeline = None;
                this.composer.read(cx).focus_handle.focus(window);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Send, window, cx| this.send(window, cx)))
            .on_action(cx.listener(|this, _: &CloseWindow, window, cx| {
                if unified::active(){unified::dismiss();return}
                this.stop_dictation(cx);
                this.stop_voice(cx);
                this.window_state = Some(workspace_ui::capture_window(window));
                this.save_workspace_now(cx);
                if cx.has_global::<quick_chat::Controller>() {
                    native::hide_window(window);
                } else {
                    window.remove_window();
                }
            }))
            .on_action(cx.listener(|_, _: &MinimizeWindow, window, _| window.minimize_window()))
            .when(self.sidebar_width > 0., |s| {
                s.child(
                    entrance.container(WorkspacePart::Sidebar)
                        .w(px(self.sidebar_width))
                        .h_full()
                        .overflow_hidden()
                        .flex_none()
                        .child(self.sidebar(cx)),
                )
            })
            .when(
                (self.sidebar_split || self.sidebar_layout_transition.is_some())
                    && self.conversations_visible
                    && (!inspector_open || window.viewport_size().width >= px(1180.)),
                |s| {
                    s.child(
                        entrance.container(WorkspacePart::Sidebar)
                            .w(px(260. * self.sidebar_layout_progress()))
                            .opacity(self.sidebar_layout_progress())
                            .h_full()
                            .flex_none()
                            .overflow_hidden()
                            .child(self.workspace(cx)),
                    )
                },
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .debug_selector(|| "chat-pane".into())
                    .bg(gpui::rgba(self.resolved_theme.main_rgba()))
                    .relative()
                    .overflow_hidden()
                    .child(div().absolute().inset_0().child(self.background.clone()))
                    .child(entrance.container(WorkspacePart::Header).flex_none().child(self.tabs(cx)))
                    .child(
                        entrance.container(WorkspacePart::Header)
                            .h(px(36.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .px_4()
                            .gap_2()
                            .border_b_1()
                            .border_color(rgba(0xebebeb12))
                            .when(
                                !self.sidebar
                                    && (!self.sidebar_split
                                        || !self.conversations_visible
                                        || (inspector_open
                                            && window.viewport_size().width < px(1180.))),
                                |s| {
                                    s.child(
                                        div()
                                            .id("expand")
                                            .debug_selector(|| "expand".into())
                                            .cursor_pointer()
                                            .child(icon("sidebar", 16., MUTED))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.toggle_sidebar();
                                                cx.notify();
                                            })),
                                    )
                                },
                            )
                            .child(
                                div()
                                    .debug_selector(|| "live-agent-indicator".into())
                                    .w(px(if unified::active(){48.}else{28.}))
                                    .h(px(if unified::active(){34.}else{18.}))
                                    .flex_none()
                                    .when(unified::active(),|s|s.child(canvas(|bounds,_,_|unified::pet_slot(bounds),|_,_,_,_|{}).size_full()))
                                    .when(!unified::active() && header_loading_logo, |s| {
                                        s.flex()
                                            .items_center()
                                            .justify_center()
                                            .child(self.loading_logo.clone())
                                    })
                                    .when(!unified::active() && !live_activity && !header_loading_logo, |s| {
                                        s.child(self.live_indicator.clone())
                                    })
                                    .when(!unified::active() && live_activity, |s| {
                                        s.flex().items_center().justify_center().child(
                                            svg()
                                                .path(provider_asset(self.agent))
                                                .size(px(16.))
                                                .text_color(rgb(MUTED)),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(theme::ui_px(12.))
                                    .flex_1()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(title),
                            )
                            .when(unified::active(), |s| {
                                s.child(
                                    div()
                                        .id("return-to-notch")
                                        .debug_selector(|| "return-to-notch".into())
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                        .text_size(theme::ui_px(11.))
                                        .text_color(rgb(MUTED))
                                        .child("↗ Notch")
                                        .on_click(|_, _, _| unified::collapse()),
                                )
                            })
                            .child(self.workspace_action_buttons(cx))
                            .child(
                                div()
                                    .id("toggle-files")
                                    .debug_selector(|| "toggle-files".into())
                                    .cursor_pointer()
                                    .p_1()
                                    .rounded_md()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .child(self.file_indicator.clone())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_files(window, cx)
                                    })),
                            )
                            .child(
                                div()
                                    .id("chat-background")
                                    .debug_selector(|| "chat-background".into())
                                    .cursor_pointer()
                                    .p_1()
                                    .rounded_md()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .child(icon("file-image", 14., MUTED))
                                    .on_click(cx.listener(
                                        |this, event: &ClickEvent, window, cx| {
                                            this.toggle_panel_from_click(
                                                Modal::Background,
                                                event.position() + point(px(-324.), px(20.)),
                                                false,
                                                window,
                                                cx,
                                            )
                                        },
                                    )),
                            )
                            .child(
                                div()
                                    .id("motion")
                                    .cursor_pointer()
                                    .p_1()
                                    .rounded_md()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .child(icon(
                                        "sparkles",
                                        14.,
                                        if self.motion { MUTED } else { 0x555555 },
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.motion = !this.motion;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(
                        (self.has_older || self.has_newer) && status.is_none(),
                        |s| {
                            s.child(
                                div()
                                    .h(px(28.))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .gap_4()
                                    .text_size(theme::ui_px(11.))
                                    .text_color(rgb(MUTED))
                                    .when(self.has_older, |s| {
                                        s.child(
                                            div()
                                                .id("load-earlier")
                                                .debug_selector(|| "load-earlier".into())
                                                .flex()
                                                .items_center()
                                                .gap_1()
                                                .cursor_pointer()
                                                .hover(|s| s.text_color(rgb(TEXT)))
                                                .child(if self.loading_earlier && self.motion {
                                                    icon("refresh", 12., ACCENT)
                                                        .with_animation(
                                                            ("history-loading", self.generation),
                                                            Animation::new(Duration::from_millis(
                                                                900,
                                                            ))
                                                            .repeat(),
                                                            |s, p| {
                                                                s.opacity(
                                                                    0.4 + 0.6
                                                                        * (p
                                                                            * std::f32::consts::PI)
                                                                            .sin(),
                                                                )
                                                            },
                                                        )
                                                        .into_any_element()
                                                } else {
                                                    icon(
                                                        if self.history_animation.is_some() {
                                                            "check"
                                                        } else {
                                                            "clock"
                                                        },
                                                        12.,
                                                        if self.history_animation.is_some() {
                                                            ACCENT
                                                        } else {
                                                            MUTED
                                                        },
                                                    )
                                                    .into_any_element()
                                                })
                                                .child(if self.loading_earlier {
                                                    "Loading history…"
                                                } else {
                                                    "Load earlier messages"
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.load_history_page(true, cx)
                                                })),
                                        )
                                    })
                                    .when(self.has_newer, |s| {
                                        s.child(
                                            div()
                                                .id("load-newer")
                                                .debug_selector(|| "load-newer".into())
                                                .cursor_pointer()
                                                .hover(|s| s.text_color(rgb(TEXT)))
                                                .child("Jump to latest")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.load_history_page(false, cx)
                                                })),
                                        )
                                    }),
                            )
                        },
                    )
                    .when(self.active.is_none(), |s| s.child(entrance.container(WorkspacePart::Content).flex_1().min_h(px(0.)).child(self.home(cx))))
                    .when(self.active.is_some(), |s| {
                        s.child(
                            entrance.container(WorkspacePart::Content)
                                .flex_1()
                                .min_h(px(0.))
                                .w_full()
                                .when(self.loading, |s| {
                                    s.flex().items_center().justify_center().child(
                                        div().text_color(rgb(MUTED)).child("Opening conversation…"),
                                    )
                                })
                                .when(!self.loading, |s| {
                                    let transcript = list(
                                        self.list.clone(),
                                        cx.processor(|this, ix, window, cx| this.row(ix, window, cx)),
                                    )
                                    .size_full();
                                    s.relative()
                                        .pl(px(32.))
                                        .child(transcript)
                                        .child(self.chat_navigation())
                                }),
                        )
                    })
                    .when(live_activity, |s| {
                        s.child(
                            div()
                                .debug_selector(|| "live-agent-status".into())
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .h(px(30.))
                                .flex_none()
                                .child(
                                    div()
                                        .debug_selector(|| "live-status-indicator".into())
                                        .w(px(28.))
                                        .h(px(18.))
                                        .flex_none()
                                        .child(self.live_indicator.clone()),
                                )
                                .child(
                                    div()
                                        .debug_selector(|| "live-status-caption".into())
                                        .text_size(theme::ui_px(11.))
                                        .text_color(rgb(MUTED))
                                        .child(
                                            if let Some(status) = self
                                                .dictation_status
                                                .filter(|_| self.speech.is_some())
                                            {
                                                status.to_owned()
                                            } else if let Some(call) = self
                                                .voice_call
                                                .as_ref()
                                                .filter(|_| waiting_for_native.is_none())
                                            {
                                                call.caption().to_owned()
                                            } else if self.handoff_pending.is_some() {
                                                "Transferring context…".to_owned()
                                            } else if self.starting {
                                                "Starting agent…".to_owned()
                                            } else {
                                                activity_caption(
                                                    status.as_deref().unwrap_or_default(),
                                                )
                                            },
                                        ),
                                ),
                        )
                    })
                    .child(entrance.container(WorkspacePart::Composer).flex_none().child(self.composer(window, cx)))
                    .when(self.terminal_visible, |s| {
                        s.when_some(self.terminal_panel.clone(), |s, panel| s.child(panel))
                    }),
            )
            .when(self.files_visible, |s| {
                s.when_some(self.file_browser.clone(), |s, browser| {
                    s.child(
                        entrance.container(WorkspacePart::Inspector)
                            .debug_selector(|| "files-inspector".into())
                            .w(px(360.))
                            .h_full()
                            .flex_none()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .border_l_1()
                            .border_color(rgba(0xebebeb12))
                            .child(
                                div()
                                    .h(px(40.))
                                    .flex_none()
                                    .px_3()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .border_b_1()
                                    .border_color(rgba(0xebebeb12))
                                    .child("Files")
                                    .child(
                                        div()
                                            .id("close-files")
                                            .cursor_pointer()
                                            .p_1()
                                            .child(icon("close", 14., MUTED))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.toggle_files(window, cx)
                                            })),
                                    ),
                            )
                            .child(div().flex_1().min_h(px(0.)).w_full().child(browser)),
                    )
                })
            })
            .when(self.broker_panel.is_some(), |s| {
                s.child(self.children_panel(cx))
            })
            .when_some(self.diff_view.clone(), |s, review| {
                s.child(
                    entrance.container(WorkspacePart::Inspector)
                        .debug_selector(|| "changes-inspector".into())
                        .w(px(440. * diff_progress))
                        .h_full()
                        .flex_none()
                        .overflow_hidden()
                        .border_l_1()
                        .border_color(rgba(0xebebeb12))
                        .bg(rgba(self.appearance.tint(GLASS_SURFACE)))
                        .child(
                            div()
                                .w(px(440.))
                                .h_full()
                                .flex()
                                .flex_col()
                                .opacity(diff_progress)
                                .child(
                                    div()
                                        .h(px(40.))
                                        .flex_none()
                                        .px_3()
                                        .flex()
                                        .items_center()
                                        .justify_between()
                                        .border_b_1()
                                        .border_color(rgba(0xebebeb12))
                                        .text_size(theme::ui_px(12.))
                                        .child("Changes")
                                        .child(
                                            div()
                                                .id("close-changes")
                                                .debug_selector(|| "close-changes".into())
                                                .cursor_pointer()
                                                .rounded_md()
                                                .p_1()
                                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                                .child(icon("close", 14., MUTED))
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.diff_view = None;
                                                    this.diff_observer = None;
                                                    this.diff_started = None;
                                                    cx.notify();
                                                })),
                                        ),
                                )
                                .child(div().flex_1().min_h(px(0.)).child(review)),
                        ),
                )
            })
            .when(self.attachment_preview.is_some(), |s| {
                s.child(self.attachment_preview_view(window, cx))
            })
            .when_some(self.action_menu.as_ref(), |s, menu| {
                s.child(self.action_menu(menu, cx))
            })
            .when(self.lifecycle_editor.is_some(), |s| {
                s.child(self.lifecycle_editor_view(cx))
            })
            .when(self.checkpoint_timeline.is_some(), |s| {
                s.child(self.checkpoint_timeline_view(cx))
            })
            .when(self.prompt_library_picker.is_some(), |s| {
                s.child(self.prompt_library_view(window, cx))
            })
            .when(self.file_context_picker.is_some(), |s| {
                s.child(self.file_context_view(window, cx))
            })
            .when(
                self.agent_picker.is_some() && self.owns_overlays(window),
                |s| s.child(self.agent_picker_view(cx)),
            )
            .when(
                self.panel_popover.is_some() && self.owns_overlays(window),
                |s| s.child(self.panel_popover_view(window, cx)),
            )
            .when(self.slash.is_some() && self.owns_overlays(window), |s| {
                s.child(self.slash_view(window, cx))
            })
            .when(self.context_picker.is_some(), |s| {
                s.child(self.context_picker_view(cx))
            })
            .when(self.team_panel.is_some(), |s| {
                s.child(self.team_panel_view(window, cx))
            })
    }
}
// Render runs before GPUI pushes the view onto its element stack. Target our
// entity explicitly instead of relying on Window::current_view during layout.
fn request_motion_frame(window: &Window, cx: &Context<Orbit>) {
    let entity = cx.entity().downgrade();
    window.on_next_frame(move |_, cx| {
        if let Some(entity) = entity.upgrade() {
            cx.notify(entity.entity_id());
        }
    });
}

fn bind_app_keys(cx: &mut App) {
    input::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
        KeyBinding::new("cmd-n", NewConversation, None),
        KeyBinding::new("cmd-t", NewConversation, None),
        KeyBinding::new("cmd-e", ToggleConversations, None),
        KeyBinding::new("cmd-s", input::SaveFile, None),
        KeyBinding::new("cmd-alt-n", ReturnToNotch, None),
        KeyBinding::new("cmd-a", ToggleSettings, Some("Orbit && !TextInput")),
        KeyBinding::new("cmd-f", ToggleFiles, None),
        KeyBinding::new("cmd-d", ToggleDiffs, None),
        KeyBinding::new("cmd-k", ToggleSearch, None),
        KeyBinding::new("cmd-b", ToggleSidebar, None),
        KeyBinding::new("cmd-shift-e", ToggleFiles, None),
        KeyBinding::new("escape", Escape, None),
        KeyBinding::new("cmd-enter", Send, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}

fn main() {
    if std::env::args().any(|a|a=="--diagnose-native-voice"){if let Err(error)=voice::transport_diagnostic(){eprintln!("Native voice diagnostic: {error}");std::process::exit(1)}return}
    unified::prepare_environment();
    let application = Application::new().with_assets(Assets);
    application.on_reopen(|cx| {
        if unified::active(){unified::reveal();return}
        cx.activate(true);
        if let Some(window) = cx
            .windows()
            .into_iter()
            .find(|window| window.downcast::<Orbit>().is_some())
            && let Err(error) = window.update(cx, |_, window, _| {
                native::show_window(window);
                window.activate_window();
            })
        {
            eprintln!("Could not bring Code's window forward: {error}");
        }
    });
    application.run(|cx: &mut App| {
        bind_app_keys(cx);
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let data = std::env::var_os("ORBIT_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("Library/Application Support/Orbit"));
        let workspace_store = match workspace_state::StateStore::new(data.join("workspace.json")) {
            Ok(store) => {
                if let Some(error) = &store.load_error {
                    eprintln!("Previous Code workspace could not be restored: {error}");
                }
                Some(std::sync::Arc::new(store))
            }
            Err(error) => {
                eprintln!("Could not load Code workspace: {error}");
                None
            }
        };
        let bounds = workspace_ui::restored_bounds(
            workspace_store
                .as_ref()
                .and_then(|store| store.initial().window.as_ref()),
            cx,
        );
        let appearance = workspace_store
            .as_ref()
            .map_or_else(Default::default, |store| store.initial().appearance);
        let main_window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(bounds),
                    window_min_size: Some(size(px(940.), px(620.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Kai · Code".into()),
                        appears_transparent: true,
                        traffic_light_position: None,
                    }),
                    window_background: WindowBackgroundAppearance::Blurred,
                    app_id: Some("com.kai.unified.experimental".into()),
                    ..Default::default()
                },
                move |window, cx| {
                    native::configure(window);
                    native::apply_appearance(window, &appearance);
                    // Order the actual window after creation/restoration and its
                    // first frame, rather than relying on app activation alone.
                    // One launch callback only; background work never raises it.
                    window.on_next_frame(|window,cx|{if !unified::active(){cx.activate(true);window.activate_window();}});
                    cx.new(|cx| Orbit::new(window, cx, workspace_store))
                },
            )
            .expect("open Code window");
        if let Err(error) = unified::attach(main_window, cx) {
            eprintln!("Kai integration unavailable: {error}");
        }
        if let Ok(orbit) = main_window
            .read(cx)
            .map(|view| view.quick_shortcut.clone())
            .and_then(|shortcut| main_window.update(cx, |_, _, cx| (cx.entity(), shortcut)))
        {
            let (entity, shortcut) = orbit;
            cx.set_global(quick_chat::Controller::new(entity.clone()));
            let result = cx.update_global::<quick_chat::Controller, _>(|controller, cx| {
                controller.bind(shortcut, cx)
            });
            if let Err(error) = result {
                entity.update(cx, |app, cx| {
                    app.notice = Some(format!("Quick Chat shortcut: {error}"));
                    cx.notify();
                });
            }
        }
        cx.activate(true);
    });
}

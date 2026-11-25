use super::*;

#[derive(Clone, Copy)]
pub(super) enum Badge {
    Agent(AgentKind),
    AgentCli(AgentKind),
    Terminal,
    Tool(&'static str, u32),
}

pub(super) struct Label(pub String);
impl Render for Label {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_2().py_1().rounded_md().bg(rgba(0x25262eff)).text_color(rgb(TEXT)).child(self.0.clone())
    }
}

fn for_title(title: &str) -> Option<Badge> {
    let title = title.trim().to_ascii_lowercase();
    let name = title.split(|ch: char| ch.is_whitespace() || ch == ':').next()?;
    let name = name.rsplit('/').next()?;
    let (glyph, tint) = match name {
        "vi" | "vim" | "view" => ("V", 0x299a59),
        "nvim" | "neovim" => ("N", 0x58a237),
        "nano" | "pico" | "uw-pico" => ("η", 0x822db0),
        "sqlite" | "sqlite3" => ("▱", 0x008dcb),
        "lazygit" => ("⑂", 0xe47731),
        "git" | "tig" | "gitui" | "git-lfs" => ("⑂", 0xb64330),
        "emacs" | "emacsclient" => ("E", 0x743f98),
        "less" | "more" | "most" | "bat" => ("≡", 0x426291),
        "man" | "tldr" => ("?", 0x68518f),
        "ps" | "pstree" | "procs" => ("≋", 0x4a537b),
        "atop" | "glances" | "bottom" | "btm" | "asitop" => ("▥", 0xb34d5b),
        "iotop" | "iftop" | "nethogs" | "bandwhich" => ("↕", 0x4e5f8e),
        "mc" | "lf" | "vifm" | "broot" | "superfile" | "spf" => ("◫", 0x856331),
        "fd" | "rg" | "ripgrep" | "ag" | "grep" => ("⌕", 0xa44067),
        "jq" | "yq" | "fx" => ("{}", 0x625a82),
        "redis-cli" | "mongosh" | "duckdb" | "usql" | "lazysql" => ("▱", 0x456c78),
        "ssh" | "mosh" | "sftp" => ("↗", 0x356b64),
        "screen" | "byobu" => ("⊞", 0x23635d),
        "kubectl" | "helm" | "stern" => ("K", 0x365eb1),
        "podman" | "dive" | "ctop" => ("▦", 0x584d9b),
        "lazygit-tui" => ("⑂", 0xe47731),
        "python" | "python3" | "ipython" | "bpython" | "ptpython" => ("Py", 0x345e8c),
        "node" | "bun" | "deno" => ("JS", 0x39724a),
        "irb" | "pry" | "ruby" => ("Rb", 0xa64047),
        "lazynpm" | "npm" | "pnpm" | "yarn" => ("⬡", 0x9b413c),
        "curl" | "wget" | "http" | "https" | "xh" => ("⇣", 0x35677d),
        "rsync" | "rclone" => ("⇄", 0x475e86),
        "watch" => ("◷", 0x62667c),
        "gdu" | "ncdu" | "dust" | "dua" => ("◉", 0x8a593b),
        "task" | "taskwarrior-tui" => ("✓", 0x4d7457),
        "cmus" | "ncmpcpp" | "spotify_player" => ("♪", 0x3b7057),
        "zoxide" | "autojump" => ("↪", 0x5c6589),
        "atuin" => ("↶", 0x695981),
        "lazydocker" | "docker" => ("▦", 0x277ce4),
        "k9s" => ("K", 0x4560eb),
        "btop" => ("≋", 0xd24362),
        "htop" => ("▥", 0xa74772),
        "top" => ("▴", 0x7f528a),
        "yazi" => ("◫", 0xbd812d),
        "ranger" => ("◇", 0x887343),
        "nnn" => ("n", 0x637d35),
        "fzf" => ("⌕", 0xe15191),
        "television" => ("▣", 0x6159c0),
        "helix" | "hx" => ("⌘", 0x8f59cc),
        "tmux" | "zellij" => ("⊞", 0x148f8a),
        "psql" | "pgcli" => ("P", 0x386ac0),
        "mysql" | "mycli" => ("M", 0x1f859e),
        "gh" | "gh-dash" => ("◆", 0x525b78),
        "opencode" => return Some(Badge::Agent(AgentKind::OpenCode)),
        "codex" => return Some(Badge::Agent(AgentKind::Codex)),
        "claude" => return Some(Badge::Agent(AgentKind::Claude)),
        "gemini" => return Some(Badge::Agent(AgentKind::Gemini)),
        "cursor" | "cursor-agent" => return Some(Badge::Agent(AgentKind::Cursor)),
        "agy" | "antigravity" => return Some(Badge::Agent(AgentKind::Antigravity)),
        "grok" => return Some(Badge::Agent(AgentKind::Grok)),
        "ninja" => return Some(Badge::Agent(AgentKind::Ninja)),
        "foundation" | "foundation-models" => ("✣", 0x00a897),
        _ => return None,
    };
    Some(Badge::Tool(glyph, tint))
}

pub(super) fn for_panel(panel: &terminal_ui::TerminalPanel) -> Badge {
    panel.launcher_agent.map(Badge::Agent)
        .or_else(|| panel.foreground_program().and_then(for_title))
        .or_else(|| for_title(panel.title()))
        .unwrap_or(Badge::Terminal)
}

impl Orbit {
    pub(super) fn tab_badges(&self, session: &SessionSummary, cx: &App) -> Vec<Badge> {
        let cli = self.chat_clis.get(&session.id);
        let mut badges = vec![Badge::Agent(session.agent)];
        if let Some(cli) = cli.filter(|cli| cli.selected) {
            badges.push(if cli.shell { Badge::Terminal } else { Badge::AgentCli(cli.agent) });
        } else {
            let deck = self.parked_terminals.get(&workspace_ui::TerminalOwner::Chat(session.id.clone()));
            let panel = if self.active.as_deref() == Some(session.id.as_str()) {
                self.active_terminal_panel()
            } else {
                deck.and_then(|deck| deck.sessions.iter().find(|session| Some(session.panel.entity_id()) == deck.active))
                    .map(|session| &session.panel)
            };
            if let Some(panel) = panel { badges.push(for_panel(panel.read(cx))); }
        }
        badges
    }
}

fn agent_background(agent: AgentKind) -> u32 {
    match agent {
        AgentKind::Codex => 0xffffff,
        AgentKind::Antigravity | AgentKind::OpenCode => 0x000000,
        AgentKind::Claude => 0x291813,
        AgentKind::Gemini => 0x111524,
        AgentKind::Cursor | AgentKind::Grok => 0x121212,
        AgentKind::Ninja => 0x10251f,
    }
}

fn agent_mark(agent: AgentKind, size: f32) -> AnyElement {
    match agent {
        // Use the bundled silhouettes at tab size so their edges stay sharp.
        AgentKind::Antigravity => svg()
            .path(agent_icon(agent))
            .size(px(size))
            .text_color(gpui::rgb(0xffffff))
            .into_any_element(),
        _ => provider_icon(agent_icon(agent), size, 0xffffff).into_any_element(),
    }
}

fn tool_foreground(background: u32) -> u32 {
    let linear = |shift: u32| {
        let channel = ((background >> shift) & 0xff) as f32 / 255.;
        if channel <= 0.04045 { channel / 12.92 }
        else { ((channel + 0.055) / 1.055).powf(2.4) }
    };
    let luminance = 0.2126 * linear(16) + 0.7152 * linear(8) + 0.0722 * linear(0);
    if luminance > 0.179 { 0x000000 } else { 0xffffff }
}

pub(super) fn stack(badges: impl IntoIterator<Item = Badge>) -> Div {
    let badges: Vec<_> = badges.into_iter().collect();
    let width = 17. + 9. * badges.len().saturating_sub(1) as f32;
    let mut view = div().relative().w(px(width)).h(px(17.)).flex_none();
    for (index, badge) in badges.into_iter().enumerate().rev() {
        let (glyph, background) = match badge {
            Badge::Agent(agent) => (agent_mark(agent, 13.), agent_background(agent)),
            Badge::AgentCli(agent) => (
                div().relative().size(px(13.)).child(agent_mark(agent, 12.))
                    .child(div().absolute().right_0().bottom_0().rounded_sm()
                        .bg(gpui::rgb(0x101010)).text_color(gpui::rgb(0xffffff))
                        .text_size(px(7.)).line_height(px(8.)).child("›"))
                    .into_any_element(),
                agent_background(agent),
            ),
            Badge::Terminal => (svg().path("terminal").size(px(11.)).text_color(gpui::rgb(0xffffff)).into_any_element(), 0x22262e),
            Badge::Tool(mark, tint) => (div().text_size(px(11.)).font_weight(FontWeight::BOLD)
                .text_color(gpui::rgb(tool_foreground(tint))).child(mark).into_any_element(), tint),
        };
        let border = if background == 0xffffff { gpui::rgba(0x00000038) } else { gpui::rgba(0xffffff60) };
        view = view.child(div().absolute().left(px(index as f32 * 9.)).top_0().size(px(17.))
            .rounded(px(5.)).border_1().border_color(border)
            .bg(gpui::rgb(background)).flex().items_center().justify_center()
            .child(glyph));
    }
    view
}

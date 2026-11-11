//! Native workspace explorer. Every directory and preview read runs off the UI thread.
use crate::presentation::{self, MessageBlock};
use crate::syntax::{Language, Lexer};
use crate::theme::{rgb, rgba};
use gpui::{prelude::*, *};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, ReadDir},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

const DIRECTORY_PAGE: usize = 200;
const TEXT_PAGE: usize = 64 * 1024;
const IMAGE_LIMIT: usize = 8 * 1024 * 1024;
const IMAGE_PIXELS: u64 = 4_194_304;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Code,
    Document,
    Data,
    Image,
    Audio,
    Video,
    Pdf,
    Archive,
    Model,
    Other,
}
impl Kind {
    fn for_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        match ext.as_str() {
            "rs" | "py" | "pyi" | "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" | "css" | "scss"
            | "html" | "htm" | "sh" | "bash" | "zsh" | "c" | "cpp" | "cc" | "cxx" | "h" | "hpp"
            | "hxx" | "swift" | "go" | "java" | "kt" | "kts" | "rb" | "toml" | "yaml" | "yml" => {
                Self::Code
            }
            "txt" | "md" | "markdown" | "rst" | "log" | "tex" | "csv" | "rtf" | "doc" | "docx"
            | "odt" | "pages" | "xlsx" | "pptx" => Self::Document,
            "json" | "jsonl" | "jsonc" | "xml" | "sql" => Self::Data,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "svg" | "tiff" | "bmp" => {
                Self::Image
            }
            "mp3" | "m4a" | "wav" | "ogg" | "flac" | "aac" => Self::Audio,
            "mp4" | "mov" | "mkv" | "webm" | "avi" => Self::Video,
            "pdf" => Self::Pdf,
            "zip" | "gz" | "tar" | "tgz" | "7z" | "rar" | "xz" | "bz2" => Self::Archive,
            "usd" | "usda" | "usdc" | "usdz" | "obj" | "stl" | "glb" | "gltf" | "fbx" | "dae" | "blend" => Self::Model,
            _ => Self::Other,
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Code => "file-code",
            Self::Document => "file-document",
            Self::Data => "file-data",
            Self::Image => "file-image",
            Self::Audio => "file-audio",
            Self::Video => "file-video",
            Self::Pdf => "file-pdf",
            Self::Archive => "file-archive",
            Self::Model | Self::Other => "file",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Code => "Source",
            Self::Document => "Document",
            Self::Data => "Data",
            Self::Image => "Image",
            Self::Audio => "Audio",
            Self::Video => "Video",
            Self::Pdf => "PDF",
            Self::Archive => "Archive",
            Self::Model => "3D model",
            Self::Other => "File",
        }
    }
}
pub(crate) fn icon_for_path(path: &Path) -> &'static str {
    Kind::for_path(path).icon()
}
#[derive(Clone)]
struct Entry {
    path: PathBuf,
    name: String,
    directory: bool,
}
struct Directory {
    request: u64,
    entries: Vec<Entry>,
    cursor: Option<ReadDir>,
    loading: bool,
    complete: bool,
    error: Option<String>,
    /// Only directly revealed entries need duplicate suppression when their
    /// natural directory page arrives. Normal pages share one ReadDir cursor.
    revealed: HashSet<PathBuf>,
}
#[derive(Clone)]
pub(super) struct FileDrag(pub PathBuf);

impl Render for FileDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().rounded_md().p_2().bg(rgba(0x242424ee))
            .child(self.0.file_name().unwrap_or_default().to_string_lossy().into_owned())
    }
}

#[derive(Clone)]
enum TreeRow {
    Entry(Entry, usize),
    More(PathBuf, usize),
    Status(String, usize),
}
struct DirectoryPage {
    entries: Vec<Entry>,
    cursor: Option<ReadDir>,
}
struct Preview {
    path: PathBuf,
    kind: Kind,
    size: u64,
    offset: u64,
    next: Option<u64>,
    blocks: Vec<MessageBlock>,
    image: Option<Arc<Image>>,
    note: Option<String>,
    pdf_pages: usize,
    source_start: Option<Lexer>,
    source_end: Option<Lexer>,
    modified: Option<SystemTime>,
}

#[derive(Clone)]
struct SyntaxCheckpoint {
    size: u64,
    modified: Option<SystemTime>,
    lexer: Lexer,
}

pub struct FileBrowser {
    appearance: crate::appearance::Appearance,
    editor: Option<Entity<crate::editor_ui::FileEditor>>,
    editor_observer: Option<Subscription>,
    editor_closed: Option<Subscription>,
    pending_editor: Option<crate::editor::Draft>,
    editor_loading: bool,
    visible: bool,
    suspended_selection: Option<(PathBuf, u64, ListOffset)>,
    restoring_preview: Option<(u64, ListOffset)>,
    root: PathBuf,
    generation: u64,
    preview_generation: u64,
    directory_request: u64,
    directories: HashMap<PathBuf, Directory>,
    expanded: HashSet<PathBuf>,
    rows: Vec<TreeRow>,
    selected: Option<PathBuf>,
    preview: Option<Preview>,
    previous_offsets: Vec<u64>,
    loading_preview: bool,
    preview_error: Option<String>,
    preview_list: ListState,
    audio: Option<AudioPlayback>,
    preview_job: Option<PreviewJob>,
    tree_scroll: UniformListScrollHandle,
    /// Only tiny lexer checkpoints are retained; source text stays one page.
    syntax_pages: HashMap<u64, SyntaxCheckpoint>,
}
impl FileBrowser {
    pub fn new(root: PathBuf, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            appearance: Default::default(),
            editor: None,
            editor_observer: None,
            editor_closed: None,
            pending_editor: None,
            editor_loading: false,
            visible: true,
            suspended_selection: None,
            restoring_preview: None,
            root: root.clone(),
            generation: 0,
            preview_generation: 0,
            directory_request: 0,
            directories: HashMap::new(),
            expanded: HashSet::from([root.clone()]),
            rows: vec![],
            selected: None,
            preview: None,
            previous_offsets: vec![],
            loading_preview: false,
            preview_error: None,
            preview_list: ListState::new(0, ListAlignment::Top, px(200.)),
            audio: None,
            preview_job: None,
            tree_scroll: UniformListScrollHandle::new(),
            syntax_pages: HashMap::new(),
        };
        this.preview_list
            .set_scroll_handler(cx.listener(|_, _, _, cx| cx.notify()));
        cx.on_release(|this, cx| this.clear_preview(cx)).detach();
        this.load_directory(root, cx);
        this
    }
    #[cfg(test)]
    pub(super) fn test_editor(&self) -> Option<Entity<crate::editor_ui::FileEditor>> {
        self.editor.clone()
    }
    #[cfg(test)]
    pub(super) fn is_suspended_and_lightweight(&self) -> bool {
        !self.visible
            && self.preview.is_none()
            && self.audio.is_none()
            && self.preview_job.is_none()
            && self.rows.is_empty()
            && self.directories.is_empty()
            && self.syntax_pages.is_empty()
    }
    pub fn workspace_edit(&self, cx: &App) -> Option<crate::editor::Draft> {
        self.editor
            .as_ref()
            .map(|editor| editor.read(cx).draft(cx))
            .or_else(|| self.pending_editor.clone())
    }
    pub fn editor_revision(&self, cx: &App) -> u64 {
        self.editor
            .as_ref()
            .map_or(0, |editor| editor.read(cx).revision())
    }
    pub fn editor_busy(&self, cx: &App) -> bool {
        self.editor_loading
            || self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.read(cx).busy())
    }
    pub fn restore_edit(&mut self, draft: crate::editor::Draft, cx: &mut Context<Self>) {
        if draft.valid() {
            self.pending_editor = Some(draft);
            cx.notify();
        }
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if !visible {
            self.suspended_selection = self.workspace_selection();
            self.generation = self.generation.wrapping_add(1);
            self.preview_generation = self.preview_generation.wrapping_add(1);
            self.clear_preview(cx);
            self.preview_list = ListState::new(0, ListAlignment::Top, px(200.));
            self.directories.clear();
            self.rows.clear();
            self.expanded.clear();
            self.syntax_pages.clear();
            self.previous_offsets.clear();
            self.preview_error = None;
            self.loading_preview = false;
        } else {
            self.expanded.insert(self.root.clone());
            self.load_directory(self.root.clone(), cx);
            if let Some((path, offset, row)) = self
                .suspended_selection
                .take()
                .filter(|(path, _, _)| path.starts_with(&self.root))
            {
                self.restore_selection(path, offset, row, cx);
            }
        }
        cx.notify();
    }
    fn start_edit(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.editor_loading
            || self
                .editor
                .as_ref()
                .is_some_and(|editor| editor.read(cx).busy() || editor.read(cx).dirty())
        {
            self.preview_error = Some(
                "Save or explicitly discard the current draft before editing another file.".into(),
            );
            cx.notify();
            return;
        }
        self.editor_loading = true;
        let root = self.root.clone();
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { crate::editor::load(&root, &path).map_err(|e| e.to_string()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.editor_loading = false;
                if this.generation != generation {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(document) => {
                        let text = document.text.to_string();
                        this.pending_editor = Some(crate::editor::Draft { document, text });
                    }
                    Err(error) => this.preview_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn materialize_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.pending_editor.take() else {
            return;
        };
        let editor = cx.new(|cx| {
            crate::editor_ui::FileEditor::new(draft.document, Some(draft.text), true, window, cx)
        });
        self.editor_observer = Some(cx.observe(&editor, |_, _, cx| cx.notify()));
        self.editor_closed = Some(cx.subscribe(
            &editor,
            |this, _, _: &crate::editor_ui::Close, cx| {
                this.editor = None;
                this.editor_observer = None;
                this.editor_closed = None;
                cx.notify();
            },
        ));
        self.editor = Some(editor);
    }
    pub fn set_appearance(
        &mut self,
        appearance: crate::appearance::Appearance,
        cx: &mut Context<Self>,
    ) {
        if self.appearance != appearance {
            self.appearance = appearance;
            cx.notify();
        }
    }
    /// Theme font changes invalidate measured Markdown/text row heights, but
    /// retain the same source page and logical viewport. No I/O or parsing.
    pub fn invalidate_theme_metrics(&mut self, cx: &mut Context<Self>) {
        let anchor = self.preview_list.logical_scroll_top();
        let count = self
            .preview
            .as_ref()
            .map_or(0, |preview| preview.blocks.len());
        self.preview_list.reset(count);
        if count > 0 {
            self.preview_list.scroll_to(ListOffset {
                item_ix: anchor.item_ix.min(count - 1),
                offset_in_item: anchor.offset_in_item,
            });
        }
        cx.notify();
    }
    pub fn selected_key(&self) -> Option<&Path> {
        self.selected.as_deref()
    }
    pub fn workspace_selection(&self) -> Option<(PathBuf, u64, ListOffset)> {
        if !self.visible {
            return self.suspended_selection.clone();
        }
        let path = self.selected.clone()?;
        let (offset, row) = if let Some(restoring) = self.restoring_preview {
            restoring
        } else {
            (
                self.preview.as_ref().map_or(0, |preview| preview.offset),
                self.preview_list.logical_scroll_top(),
            )
        };
        Some((path, offset, row))
    }
    pub fn restore_selection(
        &mut self,
        path: PathBuf,
        offset: u64,
        row: ListOffset,
        cx: &mut Context<Self>,
    ) {
        let citation = crate::citations::FileCitation {
            path: path.clone(),
            line: None,
            end_line: None,
        };
        self.open_citation_at(citation, Some((offset, row)), cx);
        self.selected = Some(path);
    }
    pub fn set_root(&mut self, root: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        if self.root == root {
            return;
        }
        self.root = root.clone();
        self.suspended_selection = None;
        self.generation += 1;
        self.preview_generation += 1;
        self.directories.clear();
        self.expanded = HashSet::from([root.clone()]);
        self.rows.clear();
        self.selected = None;
        self.clear_preview(cx);
        self.preview_error = None;
        self.previous_offsets.clear();
        self.syntax_pages.clear();
        self.loading_preview = false;
        self.load_directory(root, cx);
        cx.notify();
    }
    /// Reveal a file without walking or loading all the directories before it.
    /// Absolute paths and symlinks still obey the project confinement boundary.
    pub fn open_citation(
        &mut self,
        citation: crate::citations::FileCitation,
        cx: &mut Context<Self>,
    ) {
        self.open_citation_at(citation, None, cx);
    }
    fn open_citation_at(
        &mut self,
        citation: crate::citations::FileCitation,
        restoring: Option<(u64, ListOffset)>,
        cx: &mut Context<Self>,
    ) {
        self.clear_preview(cx);
        self.restoring_preview = restoring;
        self.previous_offsets.clear();
        self.syntax_pages.clear();
        self.preview_generation += 1;
        let generation = self.preview_generation;
        let root_generation = self.generation;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.preview_job = Some(PreviewJob(cancelled.clone()));
        self.loading_preview = true;
        self.preview_error = None;
        let root = self.root.clone();
        let job = cx.background_executor().spawn(async move {
            read_citation_at(
                &root,
                &citation,
                &cancelled,
                restoring.map(|(offset, _)| offset),
            )
            .map_err(|error| error.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if !preview_request_matches(this.generation, this.preview_generation, root_generation, generation) {
                    return;
                }
                this.preview_job = None;
                this.loading_preview = false;
                match result {
                    Ok(reveal) => {
                        for (parent, entry) in reveal.ancestors {
                            this.expanded.insert(parent.clone());
                            let directory = this.directories.entry(parent).or_insert(Directory {
                                request: 0, entries: vec![], cursor: None,
                                loading: false, complete: false, error: None, revealed: HashSet::new(),
                            });
                            if !directory.entries.iter().any(|existing| existing.path == entry.path) {
                                directory.revealed.insert(entry.path.clone());
                                directory.entries.push(entry);
                            }
                        }
                        this.selected = Some(reveal.path.clone());
                        this.preview = reveal.preview;
                        if let Some(preview) = &this.preview {
                            remember_syntax_page(&mut this.syntax_pages, preview);
                        }
                        if let Some(preview) = &this.preview
                            && preview.offset > 0
                            && !matches!(preview.kind, Kind::Pdf | Kind::Audio | Kind::Video | Kind::Image)
                        {
                            this.previous_offsets.push(0);
                        }
                        this.preview_list.reset(this.preview.as_ref().map_or(0, |preview| preview.blocks.len()));
                        if let Some((_, row)) = this.restoring_preview.take() {
                            let len = this.preview.as_ref().map_or(0, |preview| preview.blocks.len());
                            this.preview_list.scroll_to(ListOffset { item_ix: row.item_ix.min(len.saturating_sub(1)), offset_in_item: row.offset_in_item });
                        }
                        this.rebuild_rows();
                        if let Some(ix) = this.rows.iter().position(|row| matches!(row, TreeRow::Entry(entry, _) if entry.path == reveal.path)) {
                            this.tree_scroll.scroll_to_item(ix, ScrollStrategy::Center);
                        }
                    }
                    Err(error) => this.preview_error = Some(error),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }
    fn load_directory(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.directory_request += 1;
        let request = self.directory_request;
        let state = self.directories.entry(path.clone()).or_insert(Directory {
            request,
            entries: vec![],
            cursor: None,
            loading: false,
            complete: false,
            error: None,
            revealed: HashSet::new(),
        });
        if state.loading || state.complete {
            return;
        }
        state.request = request;
        state.loading = true;
        state.error = None;
        let cursor = state.cursor.take();
        let root = self.root.clone();
        let generation = self.generation;
        self.rebuild_rows();
        let job = cx.background_executor().spawn(async move {
            read_directory_page(&root, &path, cursor)
                .map(|page| (path.clone(), page))
                .map_err(|error| (path, error.to_string()))
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match result {
                    Ok((path, page)) => {
                        if let Some(state) = this.directories.get_mut(&path)
                            && state.request == request
                        {
                            // A citation may have revealed a far-paged entry directly.
                            state.entries.extend(
                                page.entries
                                    .into_iter()
                                    .filter(|entry| !state.revealed.remove(&entry.path)),
                            );
                            state.complete = page.cursor.is_none();
                            state.cursor = page.cursor;
                            state.loading = false;
                        }
                    }
                    Err((path, error)) => {
                        if let Some(state) = this.directories.get_mut(&path)
                            && state.request == request
                        {
                            state.loading = false;
                            state.error = Some(error);
                        }
                    }
                }
                this.rebuild_rows();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn rebuild_rows(&mut self) {
        fn visit(
            path: &Path,
            depth: usize,
            directories: &HashMap<PathBuf, Directory>,
            expanded: &HashSet<PathBuf>,
            rows: &mut Vec<TreeRow>,
        ) {
            let Some(dir) = directories.get(path) else {
                return;
            };
            for entry in &dir.entries {
                rows.push(TreeRow::Entry(entry.clone(), depth));
                if entry.directory && expanded.contains(&entry.path) {
                    visit(&entry.path, depth + 1, directories, expanded, rows);
                }
            }
            if dir.loading {
                rows.push(TreeRow::Status("Loading…".into(), depth));
            } else if let Some(error) = &dir.error {
                rows.push(TreeRow::Status(error.clone(), depth));
            } else if !dir.complete {
                rows.push(TreeRow::More(path.to_path_buf(), depth));
            } else if dir.entries.is_empty() {
                rows.push(TreeRow::Status("Empty folder".into(), depth));
            }
        }
        self.rows.clear();
        visit(
            &self.root,
            0,
            &self.directories,
            &self.expanded,
            &mut self.rows,
        );
    }
    fn clear_preview(&mut self, cx: &mut App) {
        self.restoring_preview = None;
        self.audio = None;
        self.preview_job = None;
        if let Some(preview) = self.preview.take()
            && let Some(image) = preview.image
        {
            image.remove_asset(cx);
        }
    }
    fn select(&mut self, entry: Entry, cx: &mut Context<Self>) {
        if entry.directory {
            if self.expanded.remove(&entry.path) {
                if self
                    .selected
                    .as_ref()
                    .is_some_and(|path| path.starts_with(&entry.path))
                {
                    self.audio = None;
                }
                self.directories
                    .retain(|path, _| !path.starts_with(&entry.path));
                self.expanded.retain(|path| !path.starts_with(&entry.path));
            } else {
                self.expanded.insert(entry.path.clone());
                self.load_directory(entry.path, cx);
            }
            self.rebuild_rows();
            cx.notify();
        } else {
            self.selected = Some(entry.path.clone());
            self.clear_preview(cx);
            self.previous_offsets.clear();
            self.syntax_pages.clear();
            self.load_preview(entry.path, 0, cx);
        }
    }
    fn load_preview(&mut self, path: PathBuf, offset: u64, cx: &mut Context<Self>) {
        self.preview_job = None;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.preview_job = Some(PreviewJob(cancelled.clone()));
        self.preview_generation += 1;
        let generation = self.preview_generation;
        let root_generation = self.generation;
        self.loading_preview = true;
        self.preview_error = None;
        let root = self.root.clone();
        let lexer = self.syntax_pages.get(&offset).cloned();
        let job = cx.background_executor().spawn(async move {
            read_preview_with_lexer(&root, &path, offset, &cancelled, lexer)
                .map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if !preview_request_matches(
                    this.generation,
                    this.preview_generation,
                    root_generation,
                    generation,
                ) {
                    return;
                }
                this.loading_preview = false;
                this.preview_job = None;
                match result {
                    Ok(preview) => {
                        remember_syntax_page(&mut this.syntax_pages, &preview);
                        this.clear_preview(cx);
                        this.preview_list.reset(preview.blocks.len());
                        this.preview = Some(preview);
                    }
                    Err(error) => this.preview_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn toggle_audio(&mut self, cx: &mut Context<Self>) {
        if self.audio.take().is_some() {
            cx.notify();
            return;
        }
        let Some(preview) = &self.preview else {
            return;
        };
        if preview.kind != Kind::Audio {
            return;
        }
        let root = self.root.clone();
        let path = preview.path.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.audio = Some(AudioPlayback {
            cancelled: cancelled.clone(),
        });
        let generation = self.preview_generation;
        let root_generation = self.generation;
        let worker_cancel = cancelled.clone();
        let job = cx
            .background_executor()
            .spawn(async move { play_audio(&root, &path, &worker_cancel) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if !preview_request_matches(
                    this.generation,
                    this.preview_generation,
                    root_generation,
                    generation,
                ) || !this
                    .audio
                    .as_ref()
                    .is_some_and(|audio| Arc::ptr_eq(&audio.cancelled, &cancelled))
                {
                    return;
                }
                this.audio = None;
                if let Err(error) = result {
                    this.preview_error = Some(format!("Audio playback: {error}"));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn tree_row(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let row = self.rows[ix].clone();
        let (depth, label, symbol) = match &row {
            TreeRow::Entry(entry, depth) => (
                *depth,
                entry.name.clone(),
                if entry.directory {
                    "folder"
                } else {
                    icon_for_path(&entry.path)
                },
            ),
            TreeRow::More(_, depth) => (*depth, "Load more files".into(), "plus"),
            TreeRow::Status(label, depth) => (*depth, label.clone(), "clock"),
        };
        let selected = matches!(&row, TreeRow::Entry(e,_) if self.selected.as_ref()==Some(&e.path));
        div()
            .id(("file-row", ix))
            .w_full()
            .h(px(30.))
            .min_w(px(0.))
            .pl(px(10. + depth as f32 * 14.))
            .pr_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .overflow_hidden()
            .cursor_pointer()
            .when(selected, |s| s.bg(rgba(0xebebeb14)))
            .hover(|s| s.bg(rgba(0xebebeb0a)))
            .when(matches!(&row,TreeRow::Entry(e,_) if e.directory), |s| {
                s.child(crate::icon(
                    if matches!(&row,TreeRow::Entry(e,_) if self.expanded.contains(&e.path)) {
                        "chevron-down"
                    } else {
                        "chevron-right"
                    },
                    10.,
                    0x929292,
                ))
            })
            .child(crate::icon(symbol, 14., if selected { 0xebebeb } else { 0x929292 }).flex_none())
            .child(div().flex_1().min_w(px(0.)).text_ellipsis().child(label))
            .when_some(match &row {
                TreeRow::Entry(entry, _) if !entry.directory => Some(FileDrag(entry.path.clone())),
                _ => None,
            }, |row, file| row.on_drag(file, |file, _, _, cx| cx.new(|_| file.clone())))
            .on_click(cx.listener(move |this, _, _, cx| match row.clone() {
                TreeRow::Entry(entry, _) => this.select(entry, cx),
                TreeRow::More(path, _) => this.load_directory(path, cx),
                TreeRow::Status(_, _) => {}
            }))
            .into_any_element()
    }
    fn follow_link(&mut self, target: &str, cx: &mut Context<Self>) {
        match crate::citations::classify(target) {
            Ok(crate::citations::Citation::File(citation)) => self.open_citation(citation, cx),
            Ok(crate::citations::Citation::Web(url)) => {
                let generation = self.preview_generation;
                let root_generation = self.generation;
                let job = cx
                    .background_executor()
                    .spawn(async move { crate::native::open_web_url(&url) });
                cx.spawn(async move |this, cx| {
                    if let Err(error) = job.await {
                        let _ = this.update(cx, |this, cx| {
                            if preview_request_matches(
                                this.generation,
                                this.preview_generation,
                                root_generation,
                                generation,
                            ) {
                                this.preview_error = Some(error);
                                cx.notify();
                            }
                        });
                    }
                })
                .detach();
            }
            Err(error) => {
                self.preview_error = Some(error);
                cx.notify();
            }
        }
    }
    fn preview_block(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let block = &self.preview.as_ref().unwrap().blocks[ix];
        let weak = cx.weak_entity();
        let on_link = move |url: &str, _: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| this.follow_link(url, cx));
        };
        let base = div()
            .w_full()
            .min_w(px(0.))
            .px_3()
            .py_1()
            .text_size(crate::theme::ui_px(13.))
            .line_height(crate::theme::ui_line_height(21.));
        match block {
            MessageBlock::Paragraph(text) => base.child(text.clone()).into_any_element(),
            MessageBlock::Heading(text) => base
                .py_2()
                .font_weight(FontWeight::SEMIBOLD)
                .text_size(crate::theme::ui_px(16.))
                .child(text.clone())
                .into_any_element(),
            MessageBlock::Voice {
                text, highlights, ..
            }
            | MessageBlock::RichParagraph { text, highlights } => base
                .child(crate::rich_text(
                    ("file-text", ix),
                    text,
                    highlights,
                    on_link,
                ))
                .into_any_element(),
            MessageBlock::ListItem {
                marker,
                text,
                highlights,
            } => base
                .flex()
                .gap_2()
                .child(marker.clone())
                .child(crate::rich_text(
                    ("file-list", ix),
                    text,
                    highlights,
                    on_link,
                ))
                .into_any_element(),
            MessageBlock::Quote(text) => base
                .border_l_2()
                .border_color(rgba(0xebebeb30))
                .text_color(rgb(0x929292))
                .child(text.clone())
                .into_any_element(),
            MessageBlock::TableRow { cells, header } => base
                .bg(rgba(0xebebeb06))
                .when(*header, |s| s.font_weight(FontWeight::SEMIBOLD))
                .child(cells.join("   │   "))
                .into_any_element(),
            MessageBlock::Code {
                text, highlights, ..
            } => base
                .bg(rgba(0xebebeb0a))
                .font_family(crate::theme::mono_font())
                .text_size(crate::theme::mono_px(11.))
                .line_height(crate::theme::mono_line_height(21.))
                .child(
                    StyledText::new(text.clone()).with_highlights(highlights.iter().map(
                        |(range, color)| {
                            (
                                range.clone(),
                                HighlightStyle {
                                    color: Some(rgb(*color).into()),
                                    ..Default::default()
                                },
                            )
                        },
                    )),
                )
                .into_any_element(),
            MessageBlock::Attachment { name, target } => {
                let target = target.clone();
                base.id(("preview-attachment", ix))
                    .cursor_pointer()
                    .rounded_md()
                    .bg(rgba(0xebebeb0a))
                    .child(crate::icon("file-image", 14., 0x929292))
                    .child(name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.follow_link(&target, cx)))
                    .into_any_element()
            }
            MessageBlock::Interaction(card) => {
                let label = match card {
                    crate::chat_interactions::ChatInteraction::NativeRequest(request) => {
                        request.title.clone()
                    }
                    crate::chat_interactions::ChatInteraction::Speaker { text, .. } => text.clone(),
                    crate::chat_interactions::ChatInteraction::Delegation { title, .. }
                    | crate::chat_interactions::ChatInteraction::Status { title, .. } => {
                        title.clone()
                    }
                    crate::chat_interactions::ChatInteraction::Question(q) => q.prompt.clone(),
                    crate::chat_interactions::ChatInteraction::Answer { question, answer } => {
                        format!("{question}\n{answer}")
                    }
                };
                base.rounded_md()
                    .bg(rgba(0xebebeb0a))
                    .child(label)
                    .into_any_element()
            }
            MessageBlock::Activity { label, details } => {
                base.child(format!("{label}\n{details}")).into_any_element()
            }
        }
    }
    pub(super) fn shows_inline_preview(&self) -> bool {
        self.editor.is_none() && self.preview.as_ref().is_some_and(|preview| {
            preview.note.is_some() && preview.image.is_none() && preview.blocks.is_empty()
        })
    }

    fn preview_panel(&self, cx: &Context<Self>) -> AnyElement {
        if let Some(editor) = &self.editor {
            return div()
                .flex_1()
                .min_h(px(0.))
                .child(editor.clone())
                .into_any_element();
        }
        let mut panel = div()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgba(0xebebeb12));
        if let Some(error) = &self.preview_error {
            panel = panel.child(div().p_3().text_color(rgb(0xb5b5b5)).child(error.clone()));
        }
        if let Some(preview) = &self.preview {
            let path = preview.path.clone();
            panel = panel.child(
                div()
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(crate::icon(preview.kind.icon(), 14., 0x929292))
                    .child(
                        div().flex_1().min_w(px(0.)).text_ellipsis().child(
                            preview
                                .path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                        ),
                    )
                    .when(
                        matches!(preview.kind, Kind::Code | Kind::Document | Kind::Data)
                            && preview.size <= crate::editor::LIMIT as u64,
                        |s| {
                            let edit_path = self
                                .selected
                                .clone()
                                .unwrap_or_else(|| preview.path.clone());
                            s.child(
                                div()
                                    .id("edit-preview-file")
                                    .debug_selector(|| "edit-preview-file".into())
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|s| s.bg(rgba(0xffffff12)))
                                    .child(if self.editor_loading {
                                        "Opening…"
                                    } else {
                                        "Edit"
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.start_edit(edit_path.clone(), cx)
                                    })),
                            )
                        },
                    )
                    .child(
                        div()
                            .id("open-file")
                            .cursor_pointer()
                            .child(crate::icon("external", 14., 0x929292))
                            .on_click(move |_, _, cx| cx.open_url(&file_url(&path))),
                    ),
            );
            panel = panel.child(
                div()
                    .px_3()
                    .pb_2()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(0x929292))
                    .child(format!(
                        "{} · {}",
                        preview.kind.label(),
                        byte_label(preview.size)
                    )),
            );
            if preview.kind == Kind::Video && preview.image.is_some() {
                let path = preview.path.clone();
                panel = panel.child(
                    div()
                        .id("video-open")
                        .mx_3()
                        .my_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgba(0xebebeb14))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .child(crate::icon("play", 16., 0xebebeb))
                        .child("Play in native player")
                        .on_click(move |_, _, cx| cx.open_url(&file_url(&path))),
                );
            }
            if preview.kind == Kind::Audio {
                panel = panel.child(
                    div()
                        .id("audio-play")
                        .mx_3()
                        .my_2()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(rgba(0xebebeb14))
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .child(crate::icon(
                            if self.audio.is_some() { "stop" } else { "play" },
                            16.,
                            0xebebeb,
                        ))
                        .child(if self.audio.is_some() {
                            "Stop playback"
                        } else {
                            "Play audio"
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_audio(cx))),
                );
            }
            if preview.pdf_pages > 0 {
                panel = panel.child(
                    div()
                        .px_3()
                        .py_2()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(crate::theme::ui_px(11.))
                        .text_color(rgb(0x929292))
                        .child(format!(
                            "Page {} of {}",
                            preview.offset + 1,
                            preview.pdf_pages
                        ))
                        .when(preview.offset > 0, |s| {
                            s.child(
                                div()
                                    .id("pdf-previous")
                                    .cursor_pointer()
                                    .child("Previous")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.loading_preview
                                            && let Some(preview) = &this.preview
                                            && preview.offset > 0
                                        {
                                            let path = preview.path.clone();
                                            let page = preview.offset - 1;
                                            this.load_preview(path, page, cx);
                                        }
                                    })),
                            )
                        })
                        .when(preview.offset + 1 < preview.pdf_pages as u64, |s| {
                            s.child(
                                div()
                                    .id("pdf-next")
                                    .cursor_pointer()
                                    .child(if self.loading_preview {
                                        "Loading…"
                                    } else {
                                        "Next page"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.loading_preview
                                            && let Some(preview) = &this.preview
                                        {
                                            let path = preview.path.clone();
                                            let page = preview.offset + 1;
                                            this.load_preview(path, page, cx);
                                        }
                                    })),
                            )
                        }),
                );
            }
            if let Some(image) = &preview.image {
                panel = panel.child(
                    div().flex_1().min_h(px(0.)).p_3().child(
                        img(image.clone())
                            .w_full()
                            .h_full()
                            .object_fit(ObjectFit::Contain),
                    ),
                );
            } else if !preview.blocks.is_empty() {
                panel = panel.child(
                    list(
                        self.preview_list.clone(),
                        cx.processor(|this, ix, _, cx| this.preview_block(ix, cx)),
                    )
                    .flex_1()
                    .min_h(px(0.))
                    .w_full(),
                );
            }
            if matches!(preview.kind, Kind::Code | Kind::Document | Kind::Data)
                && preview.size > crate::editor::LIMIT as u64
            {
                panel = panel.child(
                    div()
                        .px_3()
                        .pb_2()
                        .text_size(crate::theme::ui_px(10.))
                        .text_color(rgb(0x929292))
                        .child("Paged preview · native editing supports files up to 64 KiB"),
                );
            }
            if self.shows_inline_preview() && crate::unified::active() {
                panel = panel.child(crate::unified::inline_preview("files", &preview.path)
                    .flex_1().min_h(px(100.)).w_full());
            }
            if let Some(note) = &preview.note {
                let path = preview.path.clone();
                panel = panel.child(
                    div()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_lg()
                        .bg(rgba(0xebebeb0a))
                        .child(note.clone())
                        .child(
                            div()
                                .id("open-media")
                                .px_3()
                                .py_2()
                                .rounded_md()
                                .bg(rgba(0xebebeb10))
                                .cursor_pointer()
                                .child("Quick Look")
                                .on_click(move |_, _, cx| {
                                    if crate::unified::active() { crate::unified::quick_look(path.clone()); }
                                    else { cx.open_url(&file_url(&path)); }
                                }),
                        ),
                );
            }
            if !matches!(
                preview.kind,
                Kind::Image | Kind::Audio | Kind::Video | Kind::Pdf | Kind::Archive
            ) {
                panel = panel.child(
                    div()
                        .px_3()
                        .py_2()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(crate::theme::ui_px(11.))
                        .text_color(rgb(0x929292))
                        .child(if preview.offset == 0 {
                            "Beginning".to_string()
                        } else {
                            format!("From {}", byte_label(preview.offset))
                        })
                        .when(!self.previous_offsets.is_empty(), |s| {
                            s.child(
                                div()
                                    .id("file-previous")
                                    .cursor_pointer()
                                    .child("Previous")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.loading_preview
                                            && let Some(offset) = this.previous_offsets.pop()
                                            && let Some(path) = this.selected.clone()
                                        {
                                            this.load_preview(path, offset, cx);
                                        }
                                    })),
                            )
                        })
                        .when(preview.next.is_some(), |s| {
                            s.child(
                                div()
                                    .id("file-more")
                                    .cursor_pointer()
                                    .child(if self.loading_preview {
                                        "Loading…"
                                    } else {
                                        "Load more"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if !this.loading_preview
                                            && let Some(preview) = &this.preview
                                            && let Some(next) = preview.next
                                        {
                                            let offset = preview.offset;
                                            let path = preview.path.clone();
                                            this.previous_offsets.push(offset);
                                            this.load_preview(path, next, cx);
                                        }
                                    })),
                            )
                        }),
                );
            }
        } else {
            panel = panel.child(div().p_3().text_color(rgb(0x929292)).child(
                if self.loading_preview {
                    "Loading preview…"
                } else {
                    "Select a file to preview"
                },
            ));
        }
        panel.into_any_element()
    }
}
impl Render for FileBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.materialize_editor(window, cx);
        if !self.shows_inline_preview() { crate::unified::hide_inline_preview("files"); }
        div()
            .id("workspace-files")
            .size_full()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .bg(rgba(self.appearance.tint(0x17171766)))
            .text_color(rgb(0xebebeb))
            .text_size(crate::theme::ui_px(13.))
            .line_height(crate::theme::ui_line_height(20.))
            .child(
                div()
                    .h(px(36.))
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgba(0xebebeb12))
                    .child(crate::icon("folder", 14., 0x929292))
                    .child(
                        div().min_w(px(0.)).text_ellipsis().child(
                            self.root
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned(),
                        ),
                    ),
            )
            .child(
                uniform_list(
                    "workspace-tree",
                    self.rows.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|ix| this.tree_row(ix, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(self.tree_scroll.clone())
                .w_full()
                .h(relative(0.45))
                .min_h(px(80.))
                .px_2()
                .py_2(),
            )
            .child(self.preview_panel(cx))
    }
}

fn confined(root: &Path, path: &Path) -> std::io::Result<PathBuf> {
    let root = root.canonicalize()?;
    let path = path.canonicalize()?;
    if !path.starts_with(root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "This link points outside the project",
        ));
    }
    Ok(path)
}
struct CitationReveal {
    path: PathBuf,
    ancestors: Vec<(PathBuf, Entry)>,
    preview: Option<Preview>,
}

#[cfg(test)]
fn read_citation(
    root: &Path,
    citation: &crate::citations::FileCitation,
    cancelled: &AtomicBool,
) -> std::io::Result<CitationReveal> {
    read_citation_at(root, citation, cancelled, None)
}
fn read_citation_at(
    root: &Path,
    citation: &crate::citations::FileCitation,
    cancelled: &AtomicBool,
    restoring_offset: Option<u64>,
) -> std::io::Result<CitationReveal> {
    let requested = if citation.path.is_absolute() {
        citation.path.clone()
    } else {
        root.join(&citation.path)
    };
    let canonical_root = root.canonicalize()?;
    let canonical = confined(&canonical_root, &requested)?;
    let relative = canonical
        .strip_prefix(&canonical_root)
        .map_err(std::io::Error::other)?;
    if relative.components().count() > 128 {
        return Err(std::io::Error::other(
            "This file path is too deeply nested to reveal",
        ));
    }
    let mut ancestors = vec![];
    let mut parent = root.to_path_buf();
    for component in relative.components() {
        if cancelled.load(Ordering::Acquire) {
            return Err(std::io::Error::other("Citation cancelled"));
        }
        let path = parent.join(component);
        let directory = fs::metadata(&path)?.is_dir();
        ancestors.push((
            parent,
            Entry {
                name: component.as_os_str().to_string_lossy().into_owned(),
                path: path.clone(),
                directory,
            },
        ));
        parent = path;
    }
    let mut preview = None;
    if canonical.is_file() {
        let formatted = canonical.extension().is_some_and(|extension| {
            matches!(
                extension.to_string_lossy().to_ascii_lowercase().as_str(),
                "rtf" | "doc" | "docx" | "odt" | "pages" | "xlsx" | "pptx"
            )
        });
        let (offset, note) = if let Some(offset) = restoring_offset {
            let size = fs::metadata(&canonical)?.len();
            (
                if Kind::for_path(&canonical) == Kind::Pdf {
                    offset
                } else {
                    offset.min(size)
                },
                None,
            )
        } else if let Some(line) = citation.line {
            if matches!(
                Kind::for_path(&canonical),
                Kind::Code | Kind::Data | Kind::Document | Kind::Other
            ) && !formatted
            {
                citation_line_offset(&canonical, line, cancelled)?
            } else {
                (0, None)
            }
        } else {
            (0, None)
        };
        let mut page = read_preview_cancellable(&canonical_root, &canonical, offset, cancelled)?;
        if let Some(mut note) = note {
            if let (Some(line), Some(end)) = (citation.line, citation.end_line)
                && note == format!("Line {line}")
            {
                note = format!("Lines {line}–{end}");
            }
            page.note = Some(
                page.note
                    .map_or(note.clone(), |existing| format!("{note}. {existing}")),
            );
        }
        preview = Some(page);
    }
    Ok(CitationReveal {
        path: parent,
        ancestors,
        preview,
    })
}

const CITATION_SCAN_BYTES: u64 = 64 * 1024 * 1024;
fn citation_line_offset(
    path: &Path,
    target: u64,
    cancelled: &AtomicBool,
) -> std::io::Result<(u64, Option<String>)> {
    if target <= 1 {
        return Ok((0, (target == 1).then(|| "Line 1".into())));
    }
    let mut file = File::open(path)?;
    let mut bytes = [0; TEXT_PAGE];
    let mut offset = 0;
    let mut line = 1;
    while offset < CITATION_SCAN_BYTES {
        if cancelled.load(Ordering::Acquire) {
            return Err(std::io::Error::other("Citation cancelled"));
        }
        let count = file.read(&mut bytes)?;
        if count == 0 {
            return Ok((
                0,
                Some(format!(
                    "Line {target} is beyond the end of this file. Showing the first page."
                )),
            ));
        }
        for (ix, byte) in bytes[..count].iter().enumerate() {
            if *byte == b'\n' {
                line += 1;
                if line == target {
                    return Ok((offset + ix as u64 + 1, Some(format!("Line {target}"))));
                }
            }
        }
        offset += count as u64;
    }
    Ok((
        0,
        Some(format!(
            "Line {target} is beyond the 64 MB citation scan budget. Use preview pages to browse further."
        )),
    ))
}

fn read_directory_page(
    root: &Path,
    path: &Path,
    cursor: Option<ReadDir>,
) -> std::io::Result<DirectoryPage> {
    let mut cursor = match cursor {
        Some(cursor) => cursor,
        None => fs::read_dir(confined(root, path)?)?,
    };
    let mut entries = Vec::with_capacity(DIRECTORY_PAGE);
    let mut complete = false;
    for _ in 0..DIRECTORY_PAGE {
        let Some(entry) = cursor.next() else {
            complete = true;
            break;
        };
        let entry = entry?;
        let file_type = entry.file_type()?;
        entries.push(Entry {
            path: path.join(entry.file_name()),
            name: entry.file_name().to_string_lossy().into_owned(),
            directory: file_type.is_dir() || (file_type.is_symlink() && entry.path().is_dir()),
        });
    }
    entries.sort_by_cached_key(|entry| (!entry.directory, entry.name.to_lowercase()));
    Ok(DirectoryPage {
        entries,
        cursor: (!complete).then_some(cursor),
    })
}
#[cfg(test)]
fn read_preview(root: &Path, path: &Path, offset: u64) -> std::io::Result<Preview> {
    read_preview_cancellable(root, path, offset, &AtomicBool::new(false))
}
fn read_preview_cancellable(
    root: &Path,
    path: &Path,
    offset: u64,
    cancelled: &AtomicBool,
) -> std::io::Result<Preview> {
    read_preview_with_lexer(root, path, offset, cancelled, None)
}
fn read_preview_with_lexer(
    root: &Path,
    path: &Path,
    offset: u64,
    cancelled: &AtomicBool,
    checkpoint: Option<SyntaxCheckpoint>,
) -> std::io::Result<Preview> {
    if cancelled.load(Ordering::Acquire) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "Preview cancelled",
        ));
    }
    let path = confined(root, path)?;
    let mut file = File::open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::other("Not a regular file"));
    }
    let size = metadata.len();
    let kind = Kind::for_path(&path);
    let mut preview = Preview {
        path: path.clone(),
        kind,
        size,
        offset,
        next: None,
        blocks: vec![],
        image: None,
        note: None,
        pdf_pages: 0,
        source_start: None,
        source_end: None,
        modified: metadata.modified().ok(),
    };
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    if matches!(
        extension.as_str(),
        "rtf" | "doc" | "docx" | "odt" | "pages" | "xlsx" | "pptx"
    ) {
        preview.note = Some(
            "Open this document in its native application for formatted pages and embedded media."
                .into(),
        );
        return Ok(preview);
    }
    if kind == Kind::Model {
        preview.note = Some("Quick Look previews 3D formats supported by macOS.".into());
        return Ok(preview);
    }
    if kind == Kind::Archive {
        preview.note = Some("Open this archive with your native archive application.".into());
        return Ok(preview);
    }
    if kind == Kind::Pdf {
        match pdf::render_page(&path, offset as usize) {
            Ok((bytes, pages)) => {
                preview.image = Some(Arc::new(Image::from_bytes(ImageFormat::Png, bytes)));
                preview.pdf_pages = pages;
            }
            Err(error) => {
                preview.note = Some(format!(
                    "PDF preview unavailable: {error}. Open in Preview for full document controls."
                ));
            }
        }
        return Ok(preview);
    }
    if kind == Kind::Video {
        match video_poster(&path, cancelled) {
            Ok(bytes) => preview.image = Some(Arc::new(Image::from_bytes(ImageFormat::Png, bytes))),
            Err(_) => {
                preview.note =
                    Some("Poster unavailable. Open this video in its native player.".into())
            }
        }
        return Ok(preview);
    }
    if kind == Kind::Image && size <= IMAGE_LIMIT as u64 {
        let mut bytes = vec![];
        file.by_ref()
            .take((IMAGE_LIMIT + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() <= IMAGE_LIMIT
            && let Some((format, width, height)) = image_dimensions(&bytes)
            && width > 0
            && height > 0
            && width <= 2048
            && height <= 2048
            && u64::from(width) * u64::from(height) <= IMAGE_PIXELS
        {
            preview.image = Some(Arc::new(Image::from_bytes(format, bytes)));
            return Ok(preview);
        }
    }
    if matches!(kind, Kind::Image | Kind::Audio | Kind::Video | Kind::Pdf) {
        preview.note=Some(match kind{Kind::Pdf=>"Open this PDF in Preview for pages, search, and annotations.",Kind::Audio=>"Play this audio locally, or open it in your native player.",Kind::Video=>"Play this video in your native media player.",_=>"Open this image in Preview. Inline previews support static PNG and JPEG up to 8 MB and 2048 × 2048 pixels."}.into());
        return Ok(preview);
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![];
    file.take((TEXT_PAGE + 4) as u64).read_to_end(&mut bytes)?;
    if bytes.contains(&0) {
        preview.note = Some("Binary document. Open it with its native application.".into());
        return Ok(preview);
    }
    let mut end = bytes.len().min(TEXT_PAGE);
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
        if bytes.len().min(TEXT_PAGE) - end > 3 {
            preview.note = Some(
                "This document uses an encoding other than UTF-8. Open it in its native editor."
                    .into(),
            );
            return Ok(preview);
        }
    }
    // Prefer a real line boundary so pages do not split normal identifiers or
    // comment delimiters. A huge single line still respects the 64 KiB budget.
    if offset + (end as u64) < size
        && let Some(newline) = bytes[..end].iter().rposition(|byte| *byte == b'\n')
        && newline + 1 >= TEXT_PAGE / 2
    {
        end = newline + 1;
    }
    let text = std::str::from_utf8(&bytes[..end]).unwrap_or_default();
    if offset + (end as u64) < size {
        preview.next = Some(offset + end as u64);
    }
    let ext = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    preview.blocks = if matches!(ext.as_str(), "md" | "markdown") {
        presentation::blocks(text)
    } else if let Some(language) = Language::for_path(&path) {
        let initial = checkpoint
            .filter(|checkpoint| {
                checkpoint.size == preview.size
                    && checkpoint.modified == preview.modified
                    && checkpoint.lexer.language() == language
            })
            .map(|checkpoint| checkpoint.lexer);
        // Direct citations can begin beyond any retained checkpoint. Decode
        // just that page; do not scan an arbitrarily large prefix for colors.
        if offset > 0 && initial.is_none() {
            preview.note = Some("Preview starts partway through the file.".into());
        }
        let mut lexer = initial.unwrap_or_else(|| Lexer::new(language));
        preview.source_start = Some(lexer.clone());
        let blocks = source_blocks_with_lexer(text, &mut lexer);
        preview.source_end = Some(lexer);
        blocks
    } else if kind == Kind::Code || kind == Kind::Data {
        source_blocks(text, &ext)
    } else {
        text.lines()
            .flat_map(bounded_lines)
            .map(|line| {
                MessageBlock::Paragraph(if line.is_empty() {
                    " ".into()
                } else {
                    line.into()
                })
            })
            .collect()
    };
    Ok(preview)
}
fn image_dimensions(bytes: &[u8]) -> Option<(ImageFormat, u32, u32)> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.len() >= 24 {
        // Read chunk headers, not arbitrary compressed pixel bytes. APNG's acTL
        // must precede IDAT; static PNGs can contain these letters in their data.
        let mut at = 8;
        while at + 8 <= bytes.len() {
            let length = u32::from_be_bytes(bytes[at..at + 4].try_into().ok()?) as usize;
            let kind = &bytes[at + 4..at + 8];
            if kind == b"acTL" {
                return None;
            }
            if kind == b"IDAT" || kind == b"IEND" {
                break;
            }
            at = at.checked_add(length)?.checked_add(12)?;
        }
        return Some((
            ImageFormat::Png,
            u32::from_be_bytes(bytes[16..20].try_into().ok()?),
            u32::from_be_bytes(bytes[20..24].try_into().ok()?),
        ));
    }
    if bytes.starts_with(&[0xff, 0xd8]) {
        let mut at = 2;
        while at + 4 < bytes.len() {
            if bytes[at] != 0xff {
                return None;
            }
            let marker = bytes[at + 1];
            at += 2;
            if marker == 0xd9 || marker == 0xda {
                break;
            }
            if marker == 0x01 || (0xd0..=0xd7).contains(&marker) {
                continue;
            }
            let len = u16::from_be_bytes(bytes[at..at + 2].try_into().ok()?) as usize;
            if len < 2 || at + len > bytes.len() {
                return None;
            }
            if matches!(marker,0xc0..=0xc3|0xc5..=0xc7|0xc9..=0xcb|0xcd..=0xcf) && len >= 7 {
                return Some((
                    ImageFormat::Jpeg,
                    u16::from_be_bytes(bytes[at + 5..at + 7].try_into().ok()?) as u32,
                    u16::from_be_bytes(bytes[at + 3..at + 5].try_into().ok()?) as u32,
                ));
            }
            at += len;
        }
    }
    None
}
fn source_blocks(text: &str, language: &str) -> Vec<MessageBlock> {
    if let Some(language) = Language::for_name(language) {
        return source_blocks_with_lexer(text, &mut Lexer::new(language));
    }
    // Unknown data formats stay literal, without fabricated syntax colors.
    text.lines()
        .flat_map(bounded_lines)
        .map(|line| MessageBlock::Code {
            language: language.into(),
            text: line.into(),
            highlights: vec![],
        })
        .collect()
}
fn source_blocks_with_lexer(text: &str, lexer: &mut Lexer) -> Vec<MessageBlock> {
    let language = lexer.language().name();
    let mut blocks = vec![];
    for line in text.split_inclusive('\n') {
        let body = line.strip_suffix('\n').unwrap_or(line);
        for chunk in bounded_lines(body) {
            blocks.push(MessageBlock::Code {
                language: language.into(),
                text: if chunk.is_empty() {
                    " ".into()
                } else {
                    chunk.into()
                },
                highlights: lexer.highlight(chunk),
            });
        }
        if line.ends_with('\n') {
            // Advance comment/string state while keeping the line terminator
            // outside the rendered row, including for 1500-byte split lines.
            lexer.end_line();
        }
    }
    blocks
}
fn remember_syntax_page(cache: &mut HashMap<u64, SyntaxCheckpoint>, preview: &Preview) {
    if cache.len() >= 31 {
        cache.retain(|offset, _| *offset == preview.offset);
    }
    let mut store = |offset, lexer: &Lexer| {
        cache.insert(
            offset,
            SyntaxCheckpoint {
                size: preview.size,
                modified: preview.modified,
                lexer: lexer.clone(),
            },
        );
    };
    if let Some(start) = &preview.source_start {
        store(preview.offset, start);
    }
    if let (Some(next), Some(end)) = (preview.next, &preview.source_end) {
        store(next, end);
    }
}
fn bounded_lines(line: &str) -> Vec<&str> {
    let mut chunks = vec![];
    let mut start = 0;
    while start < line.len() {
        let mut end = (start + 1500).min(line.len());
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&line[start..end]);
        start = end;
    }
    if chunks.is_empty() {
        chunks.push("");
    }
    chunks
}
fn preview_request_matches(
    root: u64,
    preview: u64,
    requested_root: u64,
    requested_preview: u64,
) -> bool {
    root == requested_root && preview == requested_preview
}
fn byte_label(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024. * 1024.))
    } else if size >= 1024 {
        format!("{:.1} KB", size as f64 / 1024.)
    } else {
        format!("{size} bytes")
    }
}
pub(super) fn file_url(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut url = String::from("file://");
    for byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
            url.push(*byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::{
        DIRECTORY_PAGE, Kind, TEXT_PAGE, bounded_lines, file_url, image_dimensions,
        read_directory_page, read_preview,
    };
    use crate::presentation::MessageBlock;
    use gpui::{ImageFormat, IntoElement};
    use std::{
        fs,
        path::{Path, PathBuf},
    };
    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!("orbit-files-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }
    #[test]
    fn directory_pages_preserve_cursor_and_do_not_recurse() {
        let root = fixture();
        for ix in 0..403 {
            fs::write(root.join(format!("file-{ix}.txt")), "hi").unwrap();
        }
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested/inside"), "hi").unwrap();
        let first = read_directory_page(&root, &root, None).unwrap();
        assert_eq!(first.entries.len(), DIRECTORY_PAGE);
        assert!(!first.entries.iter().any(|e| e.name == "inside"));
        let second = read_directory_page(&root, &root, first.cursor).unwrap();
        let third = read_directory_page(&root, &root, second.cursor).unwrap();
        assert_eq!(second.entries.len(), 200);
        assert_eq!(third.entries.len(), 4);
        assert!(third.cursor.is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn citation_reveal_seeks_unicode_lines_and_reads_only_ancestor_metadata() {
        let root = fixture();
        let nested = root.join("src/deep");
        fs::create_dir_all(&nested).unwrap();
        for ix in 0..500 {
            fs::write(nested.join(format!("sibling-{ix}")), "other").unwrap();
        }
        let path = nested.join("citation.txt");
        let prefix = "😀abc\n".repeat(20_000);
        fs::write(
            &path,
            format!("{prefix}TARGET LINE\n{}", "later\n".repeat(20_000)),
        )
        .unwrap();
        let citation = crate::citations::FileCitation {
            path: "src/deep/citation.txt".into(),
            line: Some(20_001),
            end_line: None,
        };
        let reveal =
            super::read_citation(&root, &citation, &std::sync::atomic::AtomicBool::new(false))
                .unwrap();
        assert_eq!(reveal.ancestors.len(), 3);
        assert_eq!(reveal.path, path);
        let preview = reveal.preview.unwrap();
        assert_eq!(preview.offset, prefix.len() as u64);
        assert_eq!(preview.note.as_deref(), Some("Line 20001"));
        assert!(
            matches!(&preview.blocks[0], MessageBlock::Paragraph(text) if text == "TARGET LINE")
        );
        assert!(preview.next.is_some());
        assert!(
            preview
                .blocks
                .iter()
                .map(|block| match block {
                    MessageBlock::Paragraph(text) => text.len(),
                    _ => 0,
                })
                .sum::<usize>()
                <= TEXT_PAGE
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn citation_reveal_rejects_external_paths_symlinks_and_cancelled_work() {
        use std::{os::unix::fs::symlink, sync::atomic::AtomicBool};
        let root = fixture();
        let outside = fixture();
        fs::write(outside.join("outside.txt"), "private").unwrap();
        symlink(&outside, root.join("escape")).unwrap();
        for path in [
            outside.join("outside.txt"),
            root.join("escape/outside.txt"),
            root.join("../")
                .join(outside.file_name().unwrap())
                .join("outside.txt"),
        ] {
            let citation = crate::citations::FileCitation {
                path,
                line: None,
                end_line: None,
            };
            assert_eq!(
                super::read_citation(&root, &citation, &AtomicBool::new(false))
                    .err()
                    .unwrap()
                    .kind(),
                std::io::ErrorKind::PermissionDenied
            );
        }
        fs::write(root.join("inside.txt"), "hello").unwrap();
        let citation = crate::citations::FileCitation {
            path: "inside.txt".into(),
            line: Some(2),
            end_line: None,
        };
        assert!(super::read_citation(&root, &citation, &AtomicBool::new(true)).is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
    #[test]
    fn impossible_citation_lines_return_visible_bounded_fallback() {
        let root = fixture();
        let path = root.join("sparse.txt");
        let file = fs::File::create(&path).unwrap();
        file.set_len(super::CITATION_SCAN_BYTES + 100).unwrap();
        let (offset, note) =
            super::citation_line_offset(&path, 2, &std::sync::atomic::AtomicBool::new(false))
                .unwrap();
        assert_eq!(offset, 0);
        assert!(note.unwrap().contains("64 MB"));
        fs::write(&path, "short\n").unwrap();
        let (offset, note) =
            super::citation_line_offset(&path, 50, &std::sync::atomic::AtomicBool::new(false))
                .unwrap();
        assert_eq!(offset, 0);
        assert!(note.unwrap().contains("beyond the end"));
        fs::remove_dir_all(root).unwrap();
    }
    #[gpui::test]
    fn revealing_unloaded_ancestors_keeps_directory_pages_reachable_and_deduplicated(
        cx: &mut gpui::TestAppContext,
    ) {
        let root = fixture();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        for ix in 0..501 {
            fs::write(nested.join(format!("file-{ix}.txt")), "other").unwrap();
        }
        let target = nested.join("file-500.txt");
        fs::write(&target, "citation content").unwrap();
        let (browser, cx) = cx.add_window_view({
            let root = root.clone();
            move |window, cx| super::FileBrowser::new(root, window, cx)
        });
        cx.update(|_, cx| {
            browser.update(cx, |this, cx| {
                this.open_citation(
                    crate::citations::FileCitation {
                        path: "nested/file-500.txt".into(),
                        line: None,
                        end_line: None,
                    },
                    cx,
                )
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let this = browser.read(cx);
            assert_eq!(this.selected.as_ref(), Some(&target));
            assert!(this.expanded.contains(&nested));
            assert_eq!(this.directories[&nested].entries.len(), 1);
            assert!(
                this.rows
                    .iter()
                    .any(|row| matches!(row,super::TreeRow::More(path,_) if path == &nested))
            );
        });
        for _ in 0..3 {
            cx.update(|_, cx| {
                browser.update(cx, |this, cx| this.load_directory(nested.clone(), cx))
            });
            cx.run_until_parked();
        }
        cx.update(|_, cx| {
            let this = browser.read(cx);
            assert_eq!(this.directories[&nested].entries.len(), 501);
            assert!(this.directories[&nested].complete);
            assert_eq!(
                this.rows
                    .iter()
                    .filter(
                        |row| matches!(row,super::TreeRow::Entry(entry,_) if entry.path == target)
                    )
                    .count(),
                1
            );
            assert_eq!(
                this.preview.as_ref().unwrap().path,
                target.canonicalize().unwrap()
            );
        });
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn preview_is_bounded_and_all_unicode_text_remains_reachable() {
        let root = fixture();
        let path = root.join("large.txt");
        let text = "😀abc\n".repeat(30000);
        fs::write(&path, &text).unwrap();
        let mut offset = 0;
        let mut total = 0;
        loop {
            let p = read_preview(&root, &path, offset).unwrap();
            assert!(
                p.blocks
                    .iter()
                    .map(|b| match b {
                        MessageBlock::Paragraph(s) => s.len(),
                        _ => 0,
                    })
                    .sum::<usize>()
                    <= TEXT_PAGE
            );
            let next = p.next.unwrap_or(text.len() as u64);
            assert!(next > offset);
            total += (next - offset) as usize;
            if p.next.is_none() {
                break;
            }
            offset = next;
        }
        assert_eq!(total, text.len());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn symlinks_cannot_escape_the_project() {
        use std::os::unix::fs::symlink;
        let root = fixture();
        let outside = fixture();
        fs::write(outside.join("private.txt"), "secret").unwrap();
        symlink(outside.join("private.txt"), root.join("link.txt")).unwrap();
        assert!(read_preview(&root, &root.join("link.txt"), 0).is_err());
        symlink(&outside, root.join("external")).unwrap();
        assert!(read_directory_page(&root, &root.join("external"), None).is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
    #[test]
    fn image_headers_and_binary_previews_are_bounded() {
        let root = fixture();
        let path = root.join("data.bin");
        fs::write(&path, [0u8; 100]).unwrap();
        assert!(read_preview(&root, &path, 0).unwrap().note.is_some());
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0; 8]);
        png.extend_from_slice(&100u32.to_be_bytes());
        png.extend_from_slice(&200u32.to_be_bytes());
        assert!(matches!(
            image_dimensions(&png),
            Some((ImageFormat::Png, 100, 200))
        ));
        assert!(image_dimensions(&[0xff, 0xd8, 0xff, 0xc0, 0xff, 0xff]).is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn code_documents_preserve_embedded_markdown_fences() {
        let text = "const example = `\n````\n# literal heading\n~~~~\n`;\n";
        let blocks = super::source_blocks(text, "javascript");
        assert!(
            blocks
                .iter()
                .all(|block| matches!(block, MessageBlock::Code { .. }))
        );
        let rendered = blocks
            .iter()
            .filter_map(|block| match block {
                MessageBlock::Code { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            rendered.contains("````")
                && rendered.contains("# literal heading")
                && rendered.contains("~~~~")
        );
    }
    #[test]
    fn source_preview_uses_extension_colors_and_keeps_comments_across_chunks() {
        let root = fixture();
        let path = root.join("unicode.rs");
        let text = format!(
            "pub fn café<'a>() {{\n// {}\nlet color = \"🦀\";\n}}",
            "let fn class 🦀 ".repeat(400)
        );
        fs::write(&path, text).unwrap();
        let preview = read_preview(&root, &path, 0).unwrap();
        let mut comments = 0;
        let mut keyword_let = 0;
        for block in &preview.blocks {
            let MessageBlock::Code {
                language,
                text,
                highlights,
            } = block
            else {
                panic!("source not code");
            };
            assert_eq!(language, "rust");
            assert!(text.len() <= 1500);
            for (range, color) in highlights {
                assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
                if *color == crate::syntax::COMMENT {
                    comments += 1;
                }
                if *color == crate::syntax::KEYWORD && &text[range.clone()] == "let" {
                    keyword_let += 1;
                }
            }
        }
        assert!(
            comments > 2,
            "long comment must stay comment across virtual rows"
        );
        assert_eq!(keyword_let, 1);
        let plain = root.join("plain.txt");
        fs::write(&plain, "pub fn let false").unwrap();
        assert!(
            read_preview(&root, &plain, 0)
                .unwrap()
                .blocks
                .iter()
                .all(|block| matches!(block, MessageBlock::Paragraph(_)))
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn source_page_checkpoints_continue_comments_without_rescanning_prefix() {
        let root = fixture();
        let path = root.join("large.rs");
        fs::write(
            &path,
            format!(
                "/*\n{}*/\nconst READY: bool = true;\n",
                "let fn 😀 comment\n".repeat(5000)
            ),
        )
        .unwrap();
        let first = read_preview(&root, &path, 0).unwrap();
        let next = first.next.unwrap();
        assert!(next <= TEXT_PAGE as u64);
        let mut cache = std::collections::HashMap::new();
        super::remember_syntax_page(&mut cache, &first);
        let second = super::read_preview_with_lexer(
            &root,
            &path,
            next,
            &std::sync::atomic::AtomicBool::new(false),
            cache.get(&next).cloned(),
        )
        .unwrap();
        assert!(second.note.is_none());
        let MessageBlock::Code {
            text, highlights, ..
        } = &second.blocks[0]
        else {
            panic!()
        };
        assert_eq!(highlights, &vec![(0..text.len(), crate::syntax::COMMENT)]);
        let final_row = second.blocks.iter().find(|block| matches!(block, MessageBlock::Code {text,..} if text.starts_with("const READY"))).unwrap();
        assert!(
            matches!(final_row,MessageBlock::Code { highlights,.. } if highlights.iter().any(|(_,color)|*color==crate::syntax::KEYWORD))
        );
        let direct = read_preview(&root, &path, next).unwrap();
        assert_eq!(
            direct.note.as_deref(),
            Some("Preview starts partway through the file.")
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn edited_files_invalidate_lexer_checkpoints() {
        let root = fixture();
        let path = root.join("edited.rs");
        fs::write(&path, format!("/*\n{}", "comment\n".repeat(10000))).unwrap();
        let first = read_preview(&root, &path, 0).unwrap();
        let next = first.next.unwrap();
        let mut cache = std::collections::HashMap::new();
        super::remember_syntax_page(&mut cache, &first);
        fs::write(
            &path,
            format!("{}\nconst CHANGED: bool = true;", " ".repeat(next as usize)),
        )
        .unwrap();
        let page = super::read_preview_with_lexer(
            &root,
            &path,
            next,
            &std::sync::atomic::AtomicBool::new(false),
            cache.get(&next).cloned(),
        )
        .unwrap();
        assert!(
            page.note.is_some(),
            "stale checkpoint must not color new file as comment"
        );
        assert!(page.blocks.iter().any(|block| matches!(block,MessageBlock::Code {text,highlights,..} if text.starts_with("const") && highlights.iter().any(|(_,color)|*color==crate::syntax::KEYWORD))));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn png_animation_detection_reads_chunks_not_pixel_payloads() {
        fn chunk(kind: &[u8; 4], data: &[u8], bytes: &mut Vec<u8>) {
            bytes.extend_from_slice(&(data.len() as u32).to_be_bytes());
            bytes.extend_from_slice(kind);
            bytes.extend_from_slice(data);
            bytes.extend_from_slice(&[0; 4]);
        }
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut header = vec![];
        header.extend_from_slice(&10u32.to_be_bytes());
        header.extend_from_slice(&20u32.to_be_bytes());
        header.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(b"IHDR", &header, &mut png);
        let mut animated = png.clone();
        chunk(b"acTL", &[0; 8], &mut animated);
        chunk(
            b"IDAT",
            b"acTL appears in arbitrary pixel payload",
            &mut png,
        );
        assert_eq!(image_dimensions(&png), Some((ImageFormat::Png, 10, 20)));
        assert!(image_dimensions(&animated).is_none());
    }
    #[gpui::test]
    fn restored_file_preview_reopens_only_saved_page_and_preserves_scroll(
        cx: &mut gpui::TestAppContext,
    ) {
        let root = fixture();
        let path = root.join("large.rs");
        let line = "fn value() { let value = 42; }\n";
        fs::write(&path, line.repeat(700_000)).unwrap();
        let offset = (line.len() * 600_000) as u64;
        let row = gpui::ListOffset {
            item_ix: 9,
            offset_in_item: gpui::px(-5.),
        };
        let (browser, cx) = cx.add_window_view({
            let root = root.clone();
            move |window, cx| super::FileBrowser::new(root, window, cx)
        });
        cx.update(|_, cx| {
            browser.update(cx, |this, cx| {
                this.restore_selection(path.clone(), offset, row, cx);
                let pending = this.workspace_selection().unwrap();
                assert_eq!(pending.1, offset);
                assert_eq!(pending.2.item_ix, row.item_ix);
                assert_eq!(pending.2.offset_in_item, row.offset_in_item);
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let this = browser.read(cx);
            assert!(!this.loading_preview);
            let preview = this.preview.as_ref().unwrap();
            assert_eq!(preview.offset, offset);
            assert!(preview.next.unwrap() - offset <= super::TEXT_PAGE as u64);
            assert_eq!(this.preview_list.logical_scroll_top().item_ix, row.item_ix);
            assert_eq!(
                this.preview_list.logical_scroll_top().offset_in_item,
                row.offset_in_item
            );
            assert!(this.restoring_preview.is_none());
        });
        fs::remove_dir_all(root).unwrap();
    }
    #[gpui::test]
    fn font_change_remeasures_markdown_preserving_page_and_logical_anchor(
        cx: &mut gpui::TestAppContext,
    ) {
        let root = fixture();
        let path = root.join("font-preview.md");
        fs::write(
            &path,
            (0..24)
                .map(|i| format!("Paragraph number {i}.\n\n"))
                .collect::<String>(),
        )
        .unwrap();
        crate::theme::install_resolved(
            crate::theme::Theme::default().resolve(true, Default::default()),
        );
        let (browser, cx) = cx.add_window_view({
            let root = root.clone();
            move |window, cx| super::FileBrowser::new(root, window, cx)
        });
        let anchor = gpui::ListOffset {
            item_ix: 3,
            offset_in_item: gpui::px(0.),
        };
        cx.update(|_, cx| {
            browser.update(cx, |browser, cx| {
                browser.restore_selection(path, 0, anchor, cx)
            })
        });
        cx.run_until_parked();
        let draw = |cx: &mut gpui::VisualTestContext| {
            cx.draw(
                gpui::point(gpui::px(0.), gpui::px(0.)),
                gpui::size(gpui::px(480.), gpui::px(680.)),
                |_, _| browser.clone().into_any_element(),
            );
        };
        draw(cx);
        let (height, generation) = cx.update(|_, cx| {
            let browser = browser.read(cx);
            (
                browser.preview_list.bounds_for_item(3).unwrap().size.height,
                browser.preview_generation,
            )
        });
        crate::theme::install_resolved(
            crate::theme::Theme {
                ui_font_size: 20.,
                ..Default::default()
            }
            .resolve(true, Default::default()),
        );
        cx.update(|_, cx| browser.update(cx, |browser, cx| browser.invalidate_theme_metrics(cx)));
        draw(cx);
        cx.update(|_, cx| {
            let browser = browser.read(cx);
            assert!(browser.preview_list.bounds_for_item(3).unwrap().size.height > height);
            assert_eq!(browser.preview_list.logical_scroll_top().item_ix, 3);
            assert_eq!(
                browser.preview_list.logical_scroll_top().offset_in_item,
                gpui::px(0.)
            );
            assert_eq!(browser.preview_generation, generation);
            assert_eq!(browser.preview.as_ref().unwrap().offset, 0);
            assert!(!browser.loading_preview);
        });
        fs::remove_dir_all(root).unwrap();
    }
    #[gpui::test]
    fn stale_directory_and_preview_results_cannot_replace_new_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        let first = fixture();
        let second = fixture();
        let first_file = first.join("first.txt");
        let second_file = second.join("second.md");
        fs::write(&first_file, "Old project content").unwrap();
        fs::write(&second_file, "# New project content").unwrap();
        let (browser, cx) = cx.add_window_view({
            let first = first.clone();
            move |window, cx| super::FileBrowser::new(first, window, cx)
        });
        cx.update(|window, cx| {
            browser.update(cx, |this, cx| {
                this.select(
                    super::Entry {
                        path: first_file,
                        name: "first.txt".into(),
                        directory: false,
                    },
                    cx,
                );
                this.set_root(second.clone(), window, cx);
                this.select(
                    super::Entry {
                        path: second_file.clone(),
                        name: "second.md".into(),
                        directory: false,
                    },
                    cx,
                );
            })
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            let this = browser.read(cx);
            assert_eq!(this.root, second);
            assert_eq!(
                this.preview.as_ref().unwrap().path,
                second_file.canonicalize().unwrap()
            );
            assert!(
                this.rows
                    .iter()
                    .all(|row| !matches!(row,super::TreeRow::Entry(e,_) if e.name == "first.txt"))
            );
            assert!(
                this.rows
                    .iter()
                    .any(|row| matches!(row,super::TreeRow::Entry(e,_) if e.name == "second.md"))
            );
        });
        fs::remove_dir_all(first).unwrap();
        fs::remove_dir_all(second).unwrap();
    }
    #[test]
    fn long_unbroken_text_uses_bounded_unicode_safe_rows() {
        let text = "😀".repeat(20000);
        let chunks = bounded_lines(&text);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 1500));
        assert_eq!(chunks.concat(), text);
    }
    #[test]
    fn formatted_documents_and_large_images_use_native_viewer_without_decoding() {
        let root = fixture();
        let rtf = root.join("formatted.rtf");
        fs::write(&rtf, "{\\rtf1\\ansi formatted text}").unwrap();
        let preview = read_preview(&root, &rtf, 0).unwrap();
        assert!(preview.blocks.is_empty());
        assert!(preview.note.as_deref().unwrap().contains("formatted pages"));
        let png = root.join("huge.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[0; 8]);
        bytes.extend_from_slice(&50000u32.to_be_bytes());
        bytes.extend_from_slice(&50000u32.to_be_bytes());
        fs::write(&png, bytes).unwrap();
        let preview = read_preview(&root, &png, 0).unwrap();
        assert!(preview.image.is_none());
        assert!(preview.note.is_some());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn media_kinds_and_file_urls_use_actual_extensions() {
        assert_eq!(Kind::for_path(Path::new("thing.PDF")), Kind::Pdf);
        assert_eq!(Kind::for_path(Path::new("sound.m4a")), Kind::Audio);
        assert_eq!(Kind::for_path(Path::new("bundle.tar.gz")), Kind::Archive);
        assert_eq!(Kind::Archive.icon(), "file-archive");
        assert_eq!(
            file_url(Path::new("/tmp/a #é.png")),
            "file:///tmp/a%20%23%C3%A9.png"
        );
    }
}

/// A dropped playback never leaves its owned process running. The worker owns,
/// cancels, and reaps only the player it started.
struct AudioPlayback {
    cancelled: Arc<AtomicBool>,
}
/// Cancellation follows preview ownership, including project switches and
/// browser release. A detached worker may finish, but cannot keep its helper alive.
struct PreviewJob(Arc<AtomicBool>);
impl Drop for PreviewJob {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
impl Drop for AudioPlayback {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
struct ReapedChild(Child);
impl Drop for ReapedChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
fn wait_owned_child(
    child: &mut Child,
    cancelled: &AtomicBool,
    timeout: Option<Duration>,
) -> std::io::Result<()> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(
                    "Native media helper could not read this format",
                ))
            };
        }
        if cancelled.load(Ordering::Acquire)
            || timeout.is_some_and(|timeout| started.elapsed() >= timeout)
        {
            let _ = child.kill();
            let _ = child.wait();
            return if cancelled.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Media preview timed out",
                ))
            };
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
fn play_audio(root: &Path, path: &Path, cancelled: &AtomicBool) -> std::io::Result<()> {
    let path = confined(root, path)?;
    if cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    let mut child = ReapedChild(
        Command::new("/usr/bin/afplay")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    wait_owned_child(&mut child.0, cancelled, None)
}
fn video_poster(path: &Path, cancelled: &AtomicBool) -> std::io::Result<Vec<u8>> {
    if cancelled.load(Ordering::Acquire) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "Preview cancelled",
        ));
    }
    struct Temporary(PathBuf);
    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let scratch =
        Temporary(std::env::temp_dir().join(format!("orbit-media-{}", uuid::Uuid::new_v4())));
    fs::create_dir(&scratch.0)?;
    let mut child = ReapedChild(
        Command::new("/usr/bin/qlmanage")
            .args(["-t", "-s", "1000", "-o"])
            .arg(&scratch.0)
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    wait_owned_child(&mut child.0, cancelled, Some(Duration::from_secs(4)))?;
    if cancelled.load(Ordering::Acquire) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "Preview cancelled",
        ));
    }
    let thumbnail = fs::read_dir(&scratch.0)?
        .filter_map(Result::ok)
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
        .ok_or_else(|| std::io::Error::other("No video poster available"))?;
    let mut bytes = Vec::new();
    File::open(thumbnail.path())?
        .take((IMAGE_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > IMAGE_LIMIT
        || !image_dimensions(&bytes)
            .is_some_and(|(_, w, h)| w > 0 && h > 0 && w <= 2048 && h <= 2048)
    {
        return Err(std::io::Error::other("Video poster exceeds preview limits"));
    }
    Ok(bytes)
}

#[cfg(target_os = "macos")]
mod pdf {
    use std::{ffi::c_void, os::unix::ffi::OsStrExt, path::Path, ptr};
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Size {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Rect {
        origin: Point,
        size: Size,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Transform {
        a: f64,
        b: f64,
        c: f64,
        d: f64,
        tx: f64,
        ty: f64,
    }
    type Ref = *mut c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: Ref,
            bytes: *const u8,
            len: isize,
            is_directory: u8,
        ) -> Ref;
        fn CFDataCreateMutable(allocator: Ref, capacity: isize) -> Ref;
        fn CFDataGetLength(data: Ref) -> isize;
        fn CFDataGetBytePtr(data: Ref) -> *const u8;
        fn CFStringCreateWithCString(
            allocator: Ref,
            string: *const std::ffi::c_char,
            encoding: u32,
        ) -> Ref;
        fn CFRelease(value: Ref);
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPDFDocumentCreateWithURL(url: Ref) -> Ref;
        fn CGPDFDocumentGetNumberOfPages(document: Ref) -> usize;
        fn CGPDFDocumentGetPage(document: Ref, page: usize) -> Ref;
        fn CGPDFDocumentRelease(document: Ref);
        fn CGPDFPageGetBoxRect(page: Ref, box_type: i32) -> Rect;
        fn CGPDFPageGetDrawingTransform(
            page: Ref,
            box_type: i32,
            rect: Rect,
            rotate: i32,
            preserve_aspect: u8,
        ) -> Transform;
        fn CGColorSpaceCreateDeviceRGB() -> Ref;
        fn CGColorSpaceRelease(space: Ref);
        fn CGBitmapContextCreate(
            data: Ref,
            width: usize,
            height: usize,
            bits: usize,
            row_bytes: usize,
            color_space: Ref,
            bitmap_info: u32,
        ) -> Ref;
        fn CGContextRelease(context: Ref);
        fn CGContextSetRGBFillColor(context: Ref, r: f64, g: f64, b: f64, a: f64);
        fn CGContextFillRect(context: Ref, rect: Rect);
        fn CGContextConcatCTM(context: Ref, transform: Transform);
        fn CGContextDrawPDFPage(context: Ref, page: Ref);
        fn CGBitmapContextCreateImage(context: Ref) -> Ref;
        fn CGImageRelease(image: Ref);
    }
    #[link(name = "ImageIO", kind = "framework")]
    unsafe extern "C" {
        fn CGImageDestinationCreateWithData(
            data: Ref,
            type_id: Ref,
            count: usize,
            options: Ref,
        ) -> Ref;
        fn CGImageDestinationAddImage(destination: Ref, image: Ref, properties: Ref);
        fn CGImageDestinationFinalize(destination: Ref) -> u8;
    }
    struct Owned {
        pointer: Ref,
        release: unsafe extern "C" fn(Ref),
    }
    impl Owned {
        fn new(pointer: Ref, release: unsafe extern "C" fn(Ref)) -> std::io::Result<Self> {
            if pointer.is_null() {
                Err(std::io::Error::other(
                    "Native PDF renderer could not open this document",
                ))
            } else {
                Ok(Self { pointer, release })
            }
        }
    }
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe { (self.release)(self.pointer) }
        }
    }
    pub fn render_page(path: &Path, index: usize) -> std::io::Result<(Vec<u8>, usize)> {
        // Native PDF parsing remains page-lazy; only one <=1000px raster and its
        // PNG are retained. Very large documents stay available in Preview.
        if std::fs::metadata(path)?.len() > 64 * 1024 * 1024 {
            return Err(std::io::Error::other(
                "Use Preview for PDFs larger than 64 MB",
            ));
        }
        let bytes = path.as_os_str().as_bytes();
        // SAFETY: every pointer is checked, borrowed pages live under document,
        // all owned handles release after encoding, and dimensions are capped.
        unsafe {
            let url = Owned::new(
                CFURLCreateFromFileSystemRepresentation(
                    ptr::null_mut(),
                    bytes.as_ptr(),
                    bytes.len() as isize,
                    0,
                ),
                CFRelease,
            )?;
            let document = Owned::new(
                CGPDFDocumentCreateWithURL(url.pointer),
                CGPDFDocumentRelease,
            )?;
            let pages = CGPDFDocumentGetNumberOfPages(document.pointer);
            if pages == 0 || index >= pages {
                return Err(std::io::Error::other("PDF page is unavailable"));
            }
            let page = CGPDFDocumentGetPage(document.pointer, index + 1);
            if page.is_null() {
                return Err(std::io::Error::other("PDF page is unavailable"));
            }
            let bounds = CGPDFPageGetBoxRect(page, 1);
            if !bounds.size.width.is_finite()
                || !bounds.size.height.is_finite()
                || bounds.size.width <= 0.
                || bounds.size.height <= 0.
            {
                return Err(std::io::Error::other("Invalid PDF page dimensions"));
            }
            let scale = (1000. / bounds.size.width).min(1000. / bounds.size.height);
            let width = (bounds.size.width * scale).ceil().clamp(1., 1000.) as usize;
            let height = (bounds.size.height * scale).ceil().clamp(1., 1000.) as usize;
            let color_space = Owned::new(CGColorSpaceCreateDeviceRGB(), CGColorSpaceRelease)?;
            let context = Owned::new(
                CGBitmapContextCreate(
                    ptr::null_mut(),
                    width,
                    height,
                    8,
                    width * 4,
                    color_space.pointer,
                    1 | 0x4000,
                ),
                CGContextRelease,
            )?;
            let rect = Rect {
                origin: Point { x: 0., y: 0. },
                size: Size {
                    width: width as f64,
                    height: height as f64,
                },
            };
            CGContextSetRGBFillColor(context.pointer, 1., 1., 1., 1.);
            CGContextFillRect(context.pointer, rect);
            CGContextConcatCTM(
                context.pointer,
                CGPDFPageGetDrawingTransform(page, 1, rect, 0, 1),
            );
            CGContextDrawPDFPage(context.pointer, page);
            let image = Owned::new(CGBitmapContextCreateImage(context.pointer), CGImageRelease)?;
            let data = Owned::new(CFDataCreateMutable(ptr::null_mut(), 0), CFRelease)?;
            let type_id = Owned::new(
                CFStringCreateWithCString(ptr::null_mut(), c"public.png".as_ptr(), 0x08000100),
                CFRelease,
            )?;
            let destination = Owned::new(
                CGImageDestinationCreateWithData(data.pointer, type_id.pointer, 1, ptr::null_mut()),
                CFRelease,
            )?;
            CGImageDestinationAddImage(destination.pointer, image.pointer, ptr::null_mut());
            if CGImageDestinationFinalize(destination.pointer) == 0 {
                return Err(std::io::Error::other("PDF image encoding failed"));
            }
            let len = CFDataGetLength(data.pointer);
            let pointer = CFDataGetBytePtr(data.pointer);
            if len <= 0 || len > super::IMAGE_LIMIT as isize || pointer.is_null() {
                return Err(std::io::Error::other(
                    "PDF page image exceeds preview limits",
                ));
            }
            Ok((
                std::slice::from_raw_parts(pointer, len as usize).to_vec(),
                pages,
            ))
        }
    }
}
#[cfg(not(target_os = "macos"))]
mod pdf {
    pub fn render_page(_: &std::path::Path, _: usize) -> std::io::Result<(Vec<u8>, usize)> {
        Err(std::io::Error::other("PDF page preview requires macOS"))
    }
}

#[cfg(test)]
mod media_tests {
    use super::{ReapedChild, confined, image_dimensions, pdf, read_preview, wait_owned_child};
    use std::{
        fs,
        path::PathBuf,
        process::{Command, Stdio},
        sync::atomic::AtomicBool,
        time::Duration,
    };
    fn fixture() -> PathBuf {
        let path = std::env::temp_dir().join(format!("orbit-media-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn dropped_preview_ownership_cancels_work_before_any_filesystem_access() {
        let cancelled = std::sync::Arc::new(AtomicBool::new(false));
        let job = super::PreviewJob(cancelled.clone());
        drop(job);
        assert!(cancelled.load(std::sync::atomic::Ordering::Acquire));
        let error = match super::read_preview_cancellable(
            std::path::Path::new("/nonexistent"),
            std::path::Path::new("/nonexistent/video.mov"),
            0,
            &cancelled,
        ) {
            Err(error) => error,
            Ok(_) => panic!("cancelled preview ran"),
        };
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_pdf_renders_only_requested_page_and_navigates_real_document() {
        let root = fixture();
        let path = root.join("two-pages.pdf");
        let blue = "0 0.4 1 rg 20 20 260 360 re f\n";
        let orange = "1 0.4 0 rg 20 20 260 360 re f\n";
        let objects=[
            "<< /Type /Catalog /Pages 2 0 R >>".into(),
            "<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".into(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Contents 4 0 R >>".into(),
            format!("<< /Length {} >>\nstream\n{blue}endstream",blue.len()),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Resources << >> /Contents 6 0 R >>".into(),
            format!("<< /Length {} >>\nstream\n{orange}endstream",orange.len()),
        ];
        let mut document = String::from("%PDF-1.4\n");
        let mut offsets = vec![];
        for (ix, object) in objects.iter().enumerate() {
            offsets.push(document.len());
            document.push_str(&format!("{} 0 obj\n{object}\nendobj\n", ix + 1));
        }
        let xref = document.len();
        document.push_str("xref\n0 7\n0000000000 65535 f \n");
        for offset in offsets {
            document.push_str(&format!("{offset:010} 00000 n \n"));
        }
        document.push_str(&format!(
            "trailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        ));
        fs::write(&path, document).unwrap();
        let (first, pages) = pdf::render_page(&confined(&root, &path).unwrap(), 0).unwrap();
        let (second, _) = pdf::render_page(&path, 1).unwrap();
        assert_eq!(pages, 2);
        assert_ne!(first, second);
        let (_, width, height) = image_dimensions(&first).unwrap();
        assert!(width <= 1000 && height <= 1000);
        assert!(first.len() <= super::IMAGE_LIMIT);
        let preview = read_preview(&root, &path, 1).unwrap();
        assert_eq!(preview.pdf_pages, 2);
        assert_eq!(preview.offset, 1);
        assert!(preview.image.is_some());
        assert!(preview.blocks.is_empty());
        assert!(pdf::render_page(&path, 2).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn cancelled_and_timed_out_media_helpers_are_killed_and_reaped() {
        let mut child = ReapedChild(
            Command::new("/bin/sleep")
                .arg("10")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        wait_owned_child(&mut child.0, &AtomicBool::new(true), None).unwrap();
        assert!(child.0.try_wait().unwrap().is_some());
        let mut child = ReapedChild(
            Command::new("/bin/sleep")
                .arg("10")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let error = wait_owned_child(
            &mut child.0,
            &AtomicBool::new(false),
            Some(Duration::from_millis(20)),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(child.0.try_wait().unwrap().is_some());
    }
}

impl FileBrowser {
    pub(super) fn unified_rows(&self) -> Vec<serde_json::Value> {
        self.rows.iter().filter_map(|row|match row{
            TreeRow::Entry(entry,depth)=>Some(serde_json::json!({"path":entry.path,"name":entry.name,"depth":depth,"directory":entry.directory,"collapsed":!self.expanded.contains(&entry.path)})),
            _=>None,
        }).take(300).collect()
    }
    pub(super) fn unified_folder(
        &mut self,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if !self
            .rows
            .iter()
            .any(|r| matches!(r,TreeRow::Entry(e,_)if e.path==path&&e.directory))
        {
            return Err("This folder is no longer visible".into());
        }
        if !self.expanded.remove(&path) {
            self.expanded.insert(path.clone());
            self.load_directory(path, cx);
        }
        self.rebuild_rows();
        cx.notify();
        Ok(())
    }
}

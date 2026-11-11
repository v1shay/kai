//! Read-only native working-tree review. Git and syntax work run only on demand.
use crate::syntax::{Language, Lexer};
use crate::theme::{rgb, rgba};
use gpui::{prelude::*, *};
use std::{
    ffi::OsString,
    io::Read,
    os::unix::{ffi::OsStringExt, io::AsRawFd, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const FILE_LIMIT: usize = 256;
const LIST_BYTES: usize = 64 * 1024;
const PATCH_BYTES: usize = 256 * 1024;
const LINE_BYTES: usize = 2048;

#[derive(Clone, Debug)]
struct ChangedFile {
    path: PathBuf,
    old_path: Option<PathBuf>,
    label: String,
    status: &'static str,
    untracked: bool,
    counts: Option<crate::github::Counts>,
}
struct Changes {
    repo: PathBuf,
    branch:Option<String>,
    head: bool,
    filters: Vec<String>,
    files: Vec<ChangedFile>,
    limited: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineKind {
    Context,
    Added,
    Removed,
    Hunk,
    Note,
}
struct DiffLine {
    kind: LineKind,
    old: Option<usize>,
    new: Option<usize>,
    text: String,
    highlights: Vec<(std::ops::Range<usize>, u32)>,
}
struct Patch {
    lines: Vec<DiffLine>,
    limited: bool,
    width: f32,
    added: usize,
    removed: usize,
}
impl Patch {
    fn new(lines: Vec<DiffLine>, limited: bool) -> Self {
        // Cache once on the worker; no full-patch scan on UI interactions.
        let width = (lines.iter().map(|line| line.text.len()).max().unwrap_or(0) as f32 * 14.
            + 110.)
            .max(440.);
        let added = lines
            .iter()
            .filter(|line| line.kind == LineKind::Added && line.new.is_some())
            .count();
        let removed = lines
            .iter()
            .filter(|line| line.kind == LineKind::Removed && line.old.is_some())
            .count();
        Self {
            lines,
            limited,
            width,
            added,
            removed,
        }
    }
}
struct Job(Arc<AtomicBool>);
impl Drop for Job {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
enum ReviewSource {
    Workspace(Option<String>),
    Checkpoint {
        repository: crate::checkpoints::Repository,
        id: String,
        session: String,
        reported: Option<Arc<str>>,
    },
    Provider(Arc<str>),
}
#[derive(Clone)]
pub enum ReviewEvent {
    Changed,
    Restore {
        id: String,
        session: String,
        root: PathBuf,
        direction: crate::checkpoints::Direction,
    },
}
#[derive(Clone)]enum TreeRow{Directory{path:PathBuf,depth:usize},File{index:usize,depth:usize}}
pub struct DiffView {
    source_label:Option<String>,
    tree:Vec<TreeRow>,collapsed:std::collections::HashSet<PathBuf>,tree_at:Option<Instant>,motion:bool,
    appearance: crate::appearance::Appearance,
    source: ReviewSource,
    preview: Option<crate::checkpoints::Preview>,
    file_offset: usize,
    diff_position: crate::checkpoint_diff::Position,
    diff_previous: Vec<crate::checkpoint_diff::Position>,
    diff_next: Option<crate::checkpoint_diff::Position>,
    confirm_restore: bool,
    restoring: bool,
    restore_path: Option<PathBuf>,
    root: PathBuf,
    changes: Option<Changes>,
    selected: Option<usize>,
    patch: Option<Patch>,
    error: Option<String>,
    loading: bool,
    generation: u64,
    job: Option<Job>,
    file_scroll: UniformListScrollHandle,
    line_scroll: UniformListScrollHandle,
}
impl EventEmitter<ReviewEvent> for DiffView {}
impl DiffView {
    pub fn new(root: PathBuf, cx: &mut Context<Self>) -> Self {
        Self::with_source(root, ReviewSource::Workspace(None), cx)
    }
    pub fn set_workspace_root(&mut self,root:PathBuf,cx:&mut Context<Self>){
        if self.root==root||!matches!(self.source,ReviewSource::Workspace(_)){return}
        self.root=root;self.source=ReviewSource::Workspace(None);self.source_label=None;self.restore_path=None;
        self.file_offset=0;self.diff_position=Default::default();self.diff_previous.clear();self.collapsed.clear();self.refresh(cx);
    }
    #[cfg(test)]
    pub(super) fn test_root(&self)->&Path{&self.root}
    pub fn workspace_fallback(root: PathBuf, reason: String, cx: &mut Context<Self>) -> Self {
        Self::with_source(root, ReviewSource::Workspace(Some(reason)), cx)
    }
    pub fn checkpoint(
        root: PathBuf,
        repository: crate::checkpoints::Repository,
        id: String,
        session: String,
        reported: Option<Arc<str>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_source(
            root,
            ReviewSource::Checkpoint {
                repository,
                id,
                session,
                reported,
            },
            cx,
        )
    }
    pub fn provider(root: PathBuf, diff: Arc<str>, cx: &mut Context<Self>) -> Self {
        Self::with_source(root, ReviewSource::Provider(diff), cx)
    }
    fn with_source(root: PathBuf, source: ReviewSource, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            source_label:None,tree:vec![],collapsed:Default::default(),tree_at:None,motion:true,
            appearance: Default::default(),
            source,
            preview: None,
            file_offset: 0,
            diff_position: Default::default(),
            diff_previous: vec![],
            diff_next: None,
            confirm_restore: false,
            restoring: false,
            restore_path: None,
            root,
            changes: None,
            selected: None,
            patch: None,
            error: None,
            loading: false,
            generation: 0,
            job: None,
            file_scroll: UniformListScrollHandle::new(),
            line_scroll: UniformListScrollHandle::new(),
        };
        this.refresh(cx);
        this
    }
    pub fn set_motion(&mut self,motion:bool){self.motion=motion;}
    pub fn set_source_label(&mut self,label:String){self.source_label=Some(label);}
    fn rebuild_tree(&mut self){
        self.tree.clear();let Some(changes)=self.changes.as_ref()else{return};
        let mut indices:Vec<_>=(0..changes.files.len()).collect();indices.sort_by_key(|ix|&changes.files[*ix].path);
        let mut emitted=std::collections::HashSet::new();
        for index in indices{let path=&changes.files[index].path;let parents:Vec<_>=path.ancestors().skip(1).filter(|p|!p.as_os_str().is_empty()).collect::<Vec<_>>().into_iter().rev().collect();let mut hidden=false;
            for(depth,parent)in parents.iter().enumerate(){if emitted.insert(parent.to_path_buf()){self.tree.push(TreeRow::Directory{path:parent.to_path_buf(),depth});}if self.collapsed.contains(*parent){hidden=true;break}}
            if !hidden{self.tree.push(TreeRow::File{index,depth:parents.len()});}
        }
    }
    fn tree_row(&self,ix:usize,cx:&Context<Self>)->AnyElement{match &self.tree[ix]{
        TreeRow::File{index,depth}=>div().pl(px(*depth as f32*12.)).child(self.file_row(*index,cx)).into_any_element(),
        TreeRow::Directory{path,depth}=>{let path=path.clone();let closed=self.collapsed.contains(&path);let name=path.file_name().unwrap_or_default().to_string_lossy().into_owned();div().id(SharedString::from(format!("diff-directory-{}",path.to_string_lossy()))).debug_selector({let path=path.clone();move||format!("diff-directory-{}",path.to_string_lossy())}).h(px(30.)).pl(px(10.+*depth as f32*12.)).flex().items_center().gap_2().text_size(crate::theme::ui_px(10.)).text_color(rgb(0xa7adb9)).cursor_pointer().hover(|s|s.bg(rgba(0xffffff09))).child(crate::icon(if closed{"chevron-right"}else{"chevron-down"},10.,0xa7adb9)).child(crate::icon(if closed{"workspace-folder"}else{"workspace-folder-open"},13.,0xa7adb9)).child(name).on_click(cx.listener(move|view,_,_,cx|{if !view.collapsed.remove(&path){view.collapsed.insert(path.clone());}view.rebuild_tree();view.tree_at=Some(Instant::now());cx.notify();})).into_any_element()}
    }}
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
    #[cfg(test)]
    pub fn is_workspace_fallback(&self) -> bool {
        matches!(self.source, ReviewSource::Workspace(Some(_)))
    }
    #[cfg(test)]
    pub fn review_error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    #[cfg(test)]
    pub fn is_provider_review(&self) -> bool {
        matches!(self.source, ReviewSource::Provider(_))
    }
    pub fn checkpoint_action(&self) -> Option<(String, Option<crate::checkpoints::Direction>)> {
        let preview = self.preview.as_ref()?;
        Some((
            preview.checkpoint.id.clone(),
            (preview.total > 0
                && (preview.checkpoint.phase == "ready"
                    || preview.checkpoint.phase.starts_with("applying-")))
            .then_some(if preview.checkpoint.undone {
                crate::checkpoints::Direction::Redo
            } else {
                crate::checkpoints::Direction::Undo
            }),
        ))
    }
    pub fn checkpoint_id(&self) -> Option<&str> {
        match &self.source {
            ReviewSource::Checkpoint { id, .. } => Some(id),
            _ => None,
        }
    }
    pub fn request_restore_confirmation(&mut self, cx: &mut Context<Self>) {
        self.confirm_restore = true;
        cx.notify();
    }
    pub fn restore_finished(&mut self, result: Result<(), String>, cx: &mut Context<Self>) {
        self.restoring = false;
        match result {
            Ok(()) => {
                self.confirm_restore = false;
                self.refresh(cx);
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
    pub fn refresh_checkpoint(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.checkpoint_id() == Some(id) {
            self.refresh(cx);
        }
    }
    pub fn selected_key(&self) -> Option<&Path> {
        self.selected
            .and_then(|ix| self.changes.as_ref()?.files.get(ix))
            .map(|file| file.path.as_path())
    }
    pub fn selected_path(&self) -> Option<PathBuf> {
        if matches!(self.source, ReviewSource::Provider(_)) {
            return None;
        }
        if let Some(path) = &self.restore_path {
            return Some(path.clone());
        }
        Some(self.changes.as_ref()?.repo.join(self.selected_key()?))
    }
    pub fn restore_selection(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(changes) = &self.changes {
            if let Some(ix) = changes
                .files
                .iter()
                .position(|file| changes.repo.join(&file.path) == path)
            {
                self.select(ix, cx);
            }
        } else {
            self.restore_path = Some(path);
        }
    }
    fn begin(&mut self) -> (u64, Arc<AtomicBool>) {
        self.job = None;
        self.generation = self.generation.wrapping_add(1);
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(Job(cancel.clone()));
        self.loading = true;
        self.error = None;
        (self.generation, cancel)
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (generation, cancel) = self.begin();
        self.patch = None;
        self.changes = None;
        self.preview = None;
        self.selected = None;
        self.diff_next = None;
        let root = self.root.clone();
        let source = self.source.clone();
        let offset = self.file_offset;
        let job = cx
            .background_executor()
            .spawn(async move { review_changes(&source, &root, offset, &cancel) });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.job = None;
                this.loading = false;
                match result {
                    Ok((changes, preview)) => {
                        let first = !changes.files.is_empty();
                        let selected = this
                            .restore_path
                            .take()
                            .and_then(|path| {
                                changes
                                    .files
                                    .iter()
                                    .position(|file| changes.repo.join(&file.path) == path)
                            })
                            .unwrap_or(0);
                        if matches!(this.source,ReviewSource::Workspace(None)){this.source_label=Some(format!("{} · {} changed files",changes.branch.as_deref().unwrap_or("Detached HEAD"),changes.files.len()));}
                        this.changes = Some(changes);
                        this.rebuild_tree();
                        this.preview = preview;
                        if first {
                            this.select(selected, cx);
                        }
                    }
                    Err(error) => {
                        // A recording snapshot is refreshed by the seal callback. Missing or
                        // legacy snapshots still open an honest, usable workspace review.
                        if matches!(this.source, ReviewSource::Checkpoint { .. })
                            && !error.starts_with("Finalizing")
                        {
                            this.source = match &this.source {
                                ReviewSource::Checkpoint {
                                    reported: Some(diff),
                                    ..
                                } => ReviewSource::Provider(diff.clone()),
                                _ => ReviewSource::Workspace(Some(format!(
                                    "Exact prompt snapshot unavailable: {error}"
                                ))),
                            };
                            this.confirm_restore = false;
                            this.file_offset = 0;
                            this.refresh(cx);
                            return;
                        }
                        this.error = Some(error);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.diff_position = Default::default();
        self.diff_previous.clear();
        self.load_patch(ix, cx);
    }
    fn load_patch(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(changes) = &self.changes else {
            return;
        };
        let Some(file) = changes.files.get(ix).cloned() else {
            return;
        };
        let repo = changes.repo.clone();
        let head = changes.head;
        let filters = changes.filters.clone();
        let root = self.root.clone();
        let source = self.source.clone();
        let position = self.diff_position;
        let (generation, cancel) = self.begin();
        self.selected = Some(ix);
        self.patch = None;
        self.diff_next = None;
        self.line_scroll = UniformListScrollHandle::new();
        let job = cx.background_executor().spawn(async move {
            match source {
                ReviewSource::Checkpoint { repository, id, .. } => {
                    let page = repository
                        .diff_page(&id, &file.path, position, &cancel)
                        .map_err(|e| e.to_string())?;
                    let next = page.next;
                    Ok((checkpoint_patch(page, &file.path), next))
                }
                ReviewSource::Provider(diff) => {
                    let section = provider_sections(&diff)
                        .get(ix)
                        .copied()
                        .ok_or("This recorded diff file is unavailable.")?;
                    Ok((Patch::new(parse_patch(section, &file.path), false), None))
                }
                ReviewSource::Workspace(_) => {
                    read_patch(&root, &repo, head, &file, &filters, &cancel)
                        .map(|patch| (patch, None))
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.job = None;
                this.loading = false;
                match result {
                    Ok((patch, next)) => {
                        this.patch = Some(patch);
                        this.diff_next = next;
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.emit(ReviewEvent::Changed);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn restore_button(&self, cx: &Context<Self>) -> AnyElement {
        let Some(preview) = self.preview.as_ref().filter(|p| {
            p.total > 0
                && (p.checkpoint.phase == "ready" || p.checkpoint.phase.starts_with("applying-"))
        }) else {
            return div().into_any_element();
        };
        let undone = preview.checkpoint.undone;
        let action = if undone { "Redo" } else { "Undo" };
        div().flex_none().px_3().py_2().border_b_1().border_color(rgba(0xffffff12)).text_size(crate::theme::ui_px(11.))
            .child(div().flex().items_center().justify_between()
                .child(format!("{} file{} · this prompt",preview.total,if preview.total==1 {""} else {"s"}))
                .child(div().id("prompt-review-restore").debug_selector(||"prompt-review-restore".into()).px_3().py_1().rounded_md().bg(rgba(0xffffff10)).cursor_pointer().hover(|s|s.bg(rgba(0xffffff1a)))
                    .child(if self.restoring { "Restoring…" } else { action })
                    .on_click(cx.listener(|this,_,_,cx| { if !this.restoring { this.confirm_restore = !this.confirm_restore; cx.notify(); } }))))
            .when(self.confirm_restore && !self.restoring, |s|s.child(div().id("prompt-review-confirmation").debug_selector(||"prompt-review-confirmation".into()).mt_2().p_2().rounded_md().bg(rgba(0xffffff08))
                .child(format!("{action} this prompt’s {} file changes? Newer edits are checked before restoring; conflicts are kept safe.",preview.total))
                .child(div().mt_2().flex().justify_end().gap_2()
                    .child(div().id("cancel-prompt-restore").px_2().py_1().cursor_pointer().child("Cancel").on_click(cx.listener(|this,_,_,cx| {this.confirm_restore=false;cx.notify();})))
                    .child(div().id("confirm-prompt-restore").debug_selector(||"confirm-prompt-restore".into()).px_3().py_1().rounded_md().bg(rgba(0x8fc89a25)).cursor_pointer().child(format!("{action} changes"))
                        .on_click(cx.listener(|this,_,_,cx| {
                            if this.restoring { return; }
                            if let Some(preview) = &this.preview {
                                this.restoring=true;
                                cx.emit(ReviewEvent::Restore {id:preview.checkpoint.id.clone(),session:preview.checkpoint.session.clone(),root:this.root.clone(),direction:if preview.checkpoint.undone {crate::checkpoints::Direction::Redo} else {crate::checkpoints::Direction::Undo}});
                                cx.notify();
                            }
                        }))))))
            .into_any_element()
    }
    fn file_row(&self, ix: usize, cx: &Context<Self>) -> AnyElement {
        let file = &self.changes.as_ref().unwrap().files[ix];
        div()
            .id(("changed-file", ix))
            .debug_selector(move || format!("changed-file-{ix}"))
            .h(px(30.))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .min_w(px(0.))
            .cursor_pointer()
            .rounded_md()
            .when(self.selected == Some(ix), |s| s.bg(rgba(0xffffff12)))
            .hover(|s| s.bg(rgba(0xffffff0c)))
            .child(
                svg()
                    .path(crate::files::icon_for_path(&file.path))
                    .size(px(13.))
                    .text_color(rgb(0xa7b8de)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .text_ellipsis()
                    .text_size(crate::theme::ui_px(11.))
                    .child(file.path.file_name().unwrap_or_default().to_string_lossy().into_owned()),
            )
            .when_some(file.counts.as_ref(),|s,counts|s.child(crate::github_ui::counts(counts)))
            .when(file.counts.is_none(),|s|s.child(
                div()
                    .text_size(crate::theme::ui_px(10.))
                    .text_color(rgb(if file.status == "Deleted" {
                        0xe58c8c
                    } else {
                        0x8fc89a
                    }))
                    .child(file.status),
            ))
            .on_click(cx.listener(move |this, _, _, cx| this.select(ix, cx)))
            .into_any_element()
    }
    fn line(&self, ix: usize) -> AnyElement {
        let line = &self.patch.as_ref().unwrap().lines[ix];
        let (background, color, marker) = match line.kind {
            LineKind::Added => (0x8fc89a16, 0xaddbb6, "+"),
            LineKind::Removed => (0xe58c8c16, 0xf0acac, "−"),
            LineKind::Hunk => (0x459bf710, 0x88bdf5, ""),
            LineKind::Note => (0xffffff05, 0x929292, ""),
            LineKind::Context => (0x00000000, 0xebebeb, ""),
        };
        div()
            .h(crate::theme::mono_px(22.))
            .flex()
            .items_center()
            .bg(rgba(background))
            .whitespace_nowrap()
            .font_family(crate::theme::mono_font())
            .text_size(crate::theme::mono_px(11.))
            .line_height(crate::theme::mono_line_height(22.))
            .child(
                div()
                    .w(crate::theme::mono_px(42.))
                    .flex_none()
                    .text_right()
                    .pr_2()
                    .text_color(rgb(0x737b82))
                    .child(line.old.map_or(String::new(), |n| n.to_string())),
            )
            .child(
                div()
                    .w(crate::theme::mono_px(42.))
                    .flex_none()
                    .text_right()
                    .pr_2()
                    .text_color(rgb(0x737b82))
                    .child(line.new.map_or(String::new(), |n| n.to_string())),
            )
            .child(
                div()
                    .w(crate::theme::mono_px(18.))
                    .flex_none()
                    .text_color(rgb(color))
                    .child(marker),
            )
            .child(
                StyledText::new(line.text.clone()).with_highlights(line.highlights.iter().map(
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
            .text_color(rgb(color))
            .into_any_element()
    }
}
impl Render for DiffView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.changes.as_ref().map_or(0, |c| c.files.len());
        let tree_count=self.tree.len();let tree_reveal=if self.motion&&!crate::native::reduced_motion(){self.tree_at.map_or(1.,|at|crate::presentation::ease_out((at.elapsed().as_secs_f32()/0.18).min(1.)).clamp(0.,1.))}else{1.};
        if self.tree_at.is_some_and(|at|at.elapsed()<Duration::from_millis(180))&&tree_reveal<1.{let owner=cx.entity().downgrade();window.on_next_frame(move|_,cx|{if let Some(owner)=owner.upgrade(){cx.notify(owner.entity_id());}});}
        let lines = self.patch.as_ref().map_or(0, |p| p.lines.len());
        let width = self.patch.as_ref().map_or(440., |p| p.width);
        div().debug_selector(|| "native-diff-view".into()).size_full().min_h(px(0.))
            .flex().flex_col().text_color(rgb(0xebebeb)).bg(rgba(self.appearance.tint(0xffffff04)))
            .child(div().h(px(32.)).flex_none().px_3().flex().items_center().justify_between()
                .debug_selector(||"diff-source-label".into()).text_size(crate::theme::ui_px(10.)).text_color(rgb(0x929292)).child(self.source_label.clone().unwrap_or_else(||match &self.source {
                    ReviewSource::Checkpoint { .. } => "This prompt · saved before / after",
                    ReviewSource::Provider(_) => "Agent-reported diff · recorded response",
                    ReviewSource::Workspace(Some(_)) => "Current workspace · not an exact prompt diff",
                    ReviewSource::Workspace(None) => "Working tree · current changes",
                }.to_owned()))
                .child(div().id("refresh-changes").cursor_pointer().rounded_md().px_2().py_1()
                    .hover(|s| s.bg(rgba(0xffffff12))).child("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))))
            .when_some(match &self.source { ReviewSource::Workspace(reason) => reason.as_ref(), _ => None }, |s,reason|s.child(div().px_3().py_2().text_size(crate::theme::ui_px(10.)).text_color(rgb(0x929292)).child(reason.clone())))
            .when_some(self.preview.as_ref(), |s,preview|s.child(div().flex_none().px_3().py_1().text_size(crate::theme::ui_px(11.)).text_color(rgb(0xa7adb4)).child(preview.checkpoint.title.clone())))
            .child(self.restore_button(cx))
            .when(files > 0, |s| s.child(uniform_list("changed-files", tree_count, cx.processor(|this, range: std::ops::Range<usize>, _, cx| range.map(|ix| this.tree_row(ix, cx)).collect()))
                .track_scroll(self.file_scroll.clone()).h(px((tree_count.min(8) * 30) as f32)).opacity(tree_reveal).flex_none().w_full()))
            .when(self.preview.as_ref().is_some_and(|p|self.file_offset>0 || self.file_offset+p.changes.len()<p.total), |s|s.child(div().flex_none().flex().items_center().justify_between().px_3().py_1().text_size(crate::theme::ui_px(10.))
                .child(div().id("previous-prompt-files").cursor_pointer().child("‹ Previous files").on_click(cx.listener(|this,_,_,cx| {this.file_offset=this.file_offset.saturating_sub(64);this.refresh(cx);})))
                .child(format!("{}–{} / {}",self.file_offset+1,self.file_offset+files,self.preview.as_ref().map_or(0,|p|p.total)))
                .child(div().id("next-prompt-files").cursor_pointer().child("Next files ›").on_click(cx.listener(|this,_,_,cx| {if this.preview.as_ref().is_some_and(|p|this.file_offset+p.changes.len()<p.total) {this.file_offset+=64;this.refresh(cx);}})))))
            .when(self.changes.as_ref().is_some_and(|c| c.limited), |s| s.child(div().px_3().py_1().text_size(crate::theme::ui_px(10.)).text_color(rgb(0x929292)).child("Showing the first 256 files or 64 KB of paths.")))
            .when(self.loading, |s| s.child(div().p_3().text_size(crate::theme::ui_px(12.)).text_color(rgb(0x929292)).child("Reading changes…")))
            .when_some(self.error.as_ref(), |s, error| s.child(div().p_3().text_size(crate::theme::ui_px(12.)).text_color(rgb(0xe5ac8c)).child(error.clone())))
            .when(!self.loading && self.error.is_none() && files == 0, |s| s.child(div().p_4().text_size(crate::theme::ui_px(12.)).text_color(rgb(0x929292)).child(if matches!(self.source,ReviewSource::Checkpoint { .. }) { "This prompt did not change any workspace files." } else if matches!(self.source,ReviewSource::Provider(_)) { "No file changes in this recorded diff." } else { "No uncommitted changes in this workspace." })))
            .when_some(self.patch.as_ref(), |s, patch| s.child(
                div().h(px(28.)).flex_none().flex().items_center().gap_2().px_3()
                    .border_b_1().border_color(rgba(0xffffff12)).text_size(crate::theme::ui_px(10.))
                    .child(div().flex_1().min_w(px(0.)).text_ellipsis().child(self.selected.and_then(|ix| self.changes.as_ref()?.files.get(ix)).map_or("Changes".to_owned(), |file| file.label.clone())))
                    .when_some(self.selected_path(), |s, path| s.child(div().id("diff-quick-look").px_2()
                        .cursor_pointer().child("Quick Look")
                        .on_click(move |_, _, cx| {
                            if crate::unified::active() { crate::unified::quick_look(path.clone()); }
                            else { cx.open_url(&crate::files::file_url(&path)); }
                        })))
                    .when_some(self.selected_path().filter(|_| crate::unified::active()), |s, path| s.child(
                        div().id("diff-copy-file").px_2().cursor_pointer().child("Copy file")
                            .on_click(move |_, _, _| crate::unified::copy_file(path.clone()))))
                    .child(crate::github_ui::counts(&self.selected.and_then(|ix|self.changes.as_ref()?.files.get(ix)?.counts.clone()).unwrap_or(crate::github::Counts{added:patch.added as u64,removed:patch.removed as u64,binary:false})))
            ))
            .when(lines > 0, |s| s.child(div().id("diff-scroll-x").flex_1().min_h(px(0.)).overflow_x_scroll()
                .child(uniform_list("diff-lines", lines, cx.processor(|this, range: std::ops::Range<usize>, _, _| range.map(|ix| this.line(ix)).collect()))
                    .debug_selector(|| "diff-lines".into()).track_scroll(self.line_scroll.clone()).w(px(width)).h_full())))
            .when(self.diff_next.is_some() || !self.diff_previous.is_empty(), |s|s.child(div().flex_none().px_3().py_2().flex().justify_between().text_size(crate::theme::ui_px(11.))
                .child(div().id("previous-prompt-diff").cursor_pointer().child("‹ Previous section").on_click(cx.listener(|this,_,_,cx| {if let Some(position)=this.diff_previous.pop() {this.diff_position=position;if let Some(ix)=this.selected {this.load_patch(ix,cx);}}})))
                .child(div().id("next-prompt-diff").cursor_pointer().child("Next section ›").on_click(cx.listener(|this,_,_,cx| {if let Some(position)=this.diff_next {if this.diff_previous.len()==64 {this.diff_previous.remove(0);}this.diff_previous.push(this.diff_position);this.diff_position=position;if let Some(ix)=this.selected {this.load_patch(ix,cx);}}})))))
            .when(self.patch.as_ref().is_some_and(|p| p.limited), |s| s.child(div().p_3().text_size(crate::theme::ui_px(10.)).text_color(rgb(0x929292)).child("Large patch · preview limited to 256 KB. Use Files to inspect the full source.")))
    }
}

fn review_changes(
    source: &ReviewSource,
    root: &Path,
    offset: usize,
    cancel: &AtomicBool,
) -> Result<(Changes, Option<crate::checkpoints::Preview>), String> {
    match source {
        ReviewSource::Workspace(_) => read_changes(root, cancel).map(|changes| (changes, None)),
        ReviewSource::Checkpoint {
            repository,
            id,
            session,
            ..
        } => {
            let checkpoint = repository.get(id).map_err(|e| e.to_string())?;
            if checkpoint.session != *session
                || std::fs::canonicalize(root).ok().as_ref() != Some(&checkpoint.root)
            {
                return Err("This snapshot belongs to another chat or workspace.".into());
            }
            if matches!(checkpoint.phase.as_str(), "recording" | "sealing") {
                return Err("Finalizing this prompt’s saved files…".into());
            }
            let preview = if checkpoint.phase == "read-only" {
                crate::checkpoints::Preview {
                    checkpoint,
                    changes: vec![],
                    total: 0,
                }
            } else {
                repository
                    .preview(
                        id,
                        if checkpoint.undone {
                            crate::checkpoints::Direction::Redo
                        } else {
                            crate::checkpoints::Direction::Undo
                        },
                        offset,
                    )
                    .map_err(|e| e.to_string())?
            };
            let totals=if preview.total>0{Some(repository.change_summary(id,cancel).map_err(|e|e.to_string())?)}else{None};
            let changes = Changes {
                repo: root.to_owned(),
                branch:None,
                head: false,
                filters: vec![],
                files: preview
                    .changes
                    .iter().filter(|change|!change.directory)
                    .map(|change| ChangedFile {
                        path: change.path.clone(),
                        old_path: None,
                        label: change.path.to_string_lossy().into_owned(),
                        status: change.kind,
                        untracked: false,
                        counts: totals.as_ref().and_then(|s|s.files.iter().find(|f|f.path==change.path)).map(|f|f.counts.clone()),
                    })
                    .collect(),
                limited: false,
            };
            Ok((changes, Some(preview)))
        }
        ReviewSource::Provider(diff) => {
            let files = provider_sections(diff)
                .iter()
                .enumerate()
                .map(|(ix, section)| {
                    let path = section
                        .lines()
                        .find_map(|line| line.strip_prefix("+++ b/"))
                        .or_else(|| section.lines().find_map(|line| line.strip_prefix("--- a/")))
                        .map(PathBuf::from)
                        .unwrap_or_else(|| PathBuf::from(format!("Recorded file {}", ix + 1)));
                    ChangedFile {
                        label: path.to_string_lossy().into_owned(),
                        path,
                        old_path: None,
                        status: "Reported",
                        untracked: false,
                        counts:Some(crate::github::patch_counts(section)),
                    }
                })
                .collect();
            Ok((
                Changes {
                    repo: root.to_owned(),
                    branch:None,
                    head: false,
                    filters: vec![],
                    files,
                    limited: diff
                        .lines()
                        .filter(|line| line.starts_with("diff --git "))
                        .take(FILE_LIMIT + 1)
                        .count()
                        > FILE_LIMIT,
                },
                None,
            ))
        }
    }
}
fn provider_sections(diff: &str) -> Vec<&str> {
    let mut starts = diff
        .match_indices("diff --git ")
        .filter(|(ix, _)| *ix == 0 || diff.as_bytes()[ix - 1] == b'\n')
        .map(|(ix, _)| ix)
        .take(FILE_LIMIT + 1)
        .collect::<Vec<_>>();
    if starts.is_empty() {
        return vec![diff];
    }
    let end = if starts.len() > FILE_LIMIT {
        starts.pop().unwrap()
    } else {
        diff.len()
    };
    starts.push(end);
    starts
        .windows(2)
        .map(|pair| &diff[pair[0]..pair[1]])
        .collect()
}
fn checkpoint_patch(page: crate::checkpoint_diff::Page, path: &Path) -> Patch {
    use crate::checkpoint_diff::Kind;
    let mut text = String::new();
    let mut notes = Vec::new();
    for line in page.lines {
        match line.kind {
            Kind::Note => notes.push(DiffLine {
                kind: LineKind::Note,
                old: None,
                new: None,
                text: line.text,
                highlights: vec![],
            }),
            kind => {
                text.push_str(match kind {
                    Kind::Added => "+",
                    Kind::Removed => "-",
                    Kind::Context => " ",
                    _ => "",
                });
                text.push_str(&line.text);
                text.push('\n');
            }
        }
    }
    let mut lines = parse_patch(&text, path);
    lines.extend(notes);
    Patch::new(lines, false)
}

fn git(root: &Path) -> Command {
    let mut cmd = Command::new("/usr/bin/git");
    cmd.arg("--literal-pathspecs")
        .arg("--no-pager")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "color.ui=false",
            "-c",
            "core.quotePath=false",
        ])
        .arg("-C")
        .arg(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    cmd
}
fn output(
    cmd: &mut Command,
    limit: usize,
    cancel: &AtomicBool,
    allow_difference: bool,
) -> Result<(Vec<u8>, bool), String> {
    if cancel.load(Ordering::Acquire) {
        return Err("Change preview cancelled.".into());
    }
    cmd.process_group(0);
    let mut child = cmd
        .spawn()
        .map_err(|_| "Git could not read this workspace.".to_owned())?;
    let pid = child.id() as i32;
    let mut stdout = child.stdout.take().unwrap();
    let fd = stdout.as_raw_fd();
    // Nonblocking reads let cancellation and the deadline terminate a quiet Git
    // process. There is no polling outside an explicit background read.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        let _ = child.wait();
        return Err("Could not read the change preview.".into());
    }
    let mut bytes = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0u8; 8192];
    let deadline = Instant::now() + Duration::from_secs(8);
    let result = loop {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            break Err(if cancel.load(Ordering::Acquire) {
                "Change preview cancelled."
            } else {
                "Git took too long. Narrow the workspace and try again."
            }
            .into());
        }
        match stdout.read(&mut buffer) {
            Ok(0) => match crate::runtime::child_exit_pending(pid) {
                Ok(true) => break Ok((bytes, false)),
                Ok(false) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => break Err("Git exited unexpectedly.".into()),
            },
            Ok(n) => {
                let remaining = limit.saturating_sub(bytes.len());
                bytes.extend_from_slice(&buffer[..n.min(remaining)]);
                if n > remaining {
                    break Ok((bytes, true));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break Err("Could not read the change preview.".into()),
        }
    };
    // One owned, read-only Git process; kill/reap before returning. No shell,
    // textconv, external diff, watch task, or recurring refresh is launched.
    // Keep the direct PID waitable until its owned group is cleaned up, even
    // when Git has exited. This avoids PID reuse before descendant cleanup.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let status = child
        .wait()
        .map_err(|_| "Git exited unexpectedly.".to_owned())?;
    if matches!(&result, Ok((_, false)))
        && !(status.success() || allow_difference && status.code() == Some(1))
    {
        return Err(
            "This workspace has no readable Git history or the file is no longer available.".into(),
        );
    }
    result
}
fn read_changes(root: &Path, cancel: &AtomicBool) -> Result<Changes, String> {
    let root = root
        .canonicalize()
        .map_err(|_| "The workspace is no longer available.".to_owned())?;
    let (repo, _) = output(
        git(&root).args(["rev-parse", "--show-toplevel"]),
        4096,
        cancel,
        false,
    )?;
    let repo = PathBuf::from(OsString::from_vec(
        repo.strip_suffix(b"\n").unwrap_or(&repo).to_vec(),
    ));
    let head = output(
        git(&repo).args(["rev-parse", "--verify", "HEAD"]),
        128,
        cancel,
        false,
    )
    .is_ok();
    if cancel.load(Ordering::Acquire) {
        return Err("Change preview cancelled.".into());
    }
    let scope = root
        .strip_prefix(&repo)
        .map_err(|_| "Workspace is outside the repository.".to_owned())?;
    let filters = filter_overrides(&repo, cancel)?;
    let mut status = git(&repo);
    disable_filters(&mut status, &filters);
    let (bytes, truncated) = output(
        status
            .args([
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--",
            ])
            .arg(if scope.as_os_str().is_empty() {
                Path::new(".")
            } else {
                scope
            }),
        LIST_BYTES,
        cancel,
        false,
    )?;
    let (mut files, limited) = parse_files(&bytes, scope, truncated);
    let totals=crate::github::workspace_counts(&repo,&filters,head,cancel).map_err(|e|e.to_string())?;
    for file in &mut files{file.counts=if file.untracked{crate::github::untracked_counts(&repo.join(&file.path),cancel).ok()}else{totals.get(&file.path).cloned()};}
    let branch=output(git(&repo).args(["symbolic-ref","--quiet","--short","HEAD"]),4096,cancel,true).ok().map(|(data,_)|String::from_utf8_lossy(&data).trim().to_owned()).filter(|s|!s.is_empty());
    Ok(Changes {
        repo,
        branch,
        head,
        filters,
        files,
        limited,
    })
}
pub(crate) fn filter_overrides(repo: &Path, cancel: &AtomicBool) -> Result<Vec<String>, String> {
    let (keys, limited) = output(
        git(repo).args([
            "config",
            "-z",
            "--name-only",
            "--get-regexp",
            "^filter\\..*\\.(clean|process)$",
        ]),
        LIST_BYTES,
        cancel,
        true,
    )?;
    if limited {
        return Err("Too many Git filter rules to preview this workspace.".into());
    }
    let mut drivers = std::collections::BTreeSet::new();
    for key in keys.split(|b| *b == 0).filter(|key| !key.is_empty()) {
        let key = std::str::from_utf8(key)
            .map_err(|_| "A Git filter rule uses an unsupported encoding.".to_owned())?;
        let driver = key
            .rsplit_once('.')
            .map(|(driver, _)| driver)
            .unwrap_or(key);
        if driver.len() > 512 || drivers.len() > 128 {
            return Err("Too many Git filter rules to preview this workspace.".into());
        }
        drivers.insert(driver);
    }
    Ok(drivers
        .into_iter()
        .flat_map(|driver| {
            [
                format!("{driver}.clean="),
                format!("{driver}.process="),
                format!("{driver}.required=false"),
            ]
        })
        .collect())
}
fn disable_filters(command: &mut Command, filters: &[String]) {
    for filter in filters {
        command.arg("-c").arg(filter);
    }
}
fn parse_files(bytes: &[u8], scope: &Path, truncated: bool) -> (Vec<ChangedFile>, bool) {
    let mut fields = bytes.split(|b| *b == 0);
    let mut files = Vec::new();
    let mut limited = truncated;
    while let Some(field) = fields.next() {
        if field.len() < 4 || field[2] != b' ' {
            continue;
        }
        // A truncated final pathname must never become a clickable path.
        if truncated && !bytes.ends_with(&[0]) && fields.clone().next().is_none() {
            break;
        }
        let xy = &field[..2];
        let path = PathBuf::from(OsString::from_vec(field[3..].to_vec()));
        let old_path = if xy.contains(&b'R') || xy.contains(&b'C') {
            fields
                .next()
                .map(|old| PathBuf::from(OsString::from_vec(old.to_vec())))
                .filter(|old| {
                    !old.is_absolute()
                        && !old
                            .components()
                            .any(|c| matches!(c, std::path::Component::ParentDir))
                })
        } else {
            None
        };
        if path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue;
        }
        let Ok(relative) = path.strip_prefix(scope) else {
            continue;
        };
        if files.len() == FILE_LIMIT {
            limited = true;
            break;
        }
        let untracked = xy == b"??";
        let status = if untracked || xy.contains(&b'A') {
            "Added"
        } else if xy.contains(&b'D') {
            "Deleted"
        } else if xy.contains(&b'R') {
            "Renamed"
        } else if xy.contains(&b'U') {
            "Conflict"
        } else {
            "Modified"
        };
        files.push(ChangedFile {
            label: relative.to_string_lossy().into_owned(),
            path,
            old_path,
            status,
            untracked,
            counts:None,
        });
    }
    (files, limited)
}
fn read_patch(
    root: &Path,
    repo: &Path,
    head: bool,
    file: &ChangedFile,
    filters: &[String],
    cancel: &AtomicBool,
) -> Result<Patch, String> {
    let mut command = git(repo);
    disable_filters(&mut command, filters);
    command.args([
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--unified=3",
    ]);
    let no_index = file.untracked || !head;
    if no_index {
        let original = repo.join(&file.path);
        if std::fs::symlink_metadata(&original)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Ok(Patch::new(
                vec![DiffLine {
                    kind: LineKind::Note,
                    old: None,
                    new: None,
                    text:
                        "Symbolic link added · its destination is not read by the change preview."
                            .into(),
                    highlights: vec![],
                }],
                false,
            ));
        }
        let path = original
            .canonicalize()
            .map_err(|_| "The file is no longer available.".to_owned())?;
        let root = root
            .canonicalize()
            .map_err(|_| "The workspace is no longer available.".to_owned())?;
        if !path.starts_with(&root) || !path.is_file() {
            return Err(
                "Use Files to inspect this path; change previews stay inside the workspace.".into(),
            );
        }
        command
            .args(["--no-index", "--"])
            .arg("/dev/null")
            .arg(path);
    } else {
        let root = root
            .canonicalize()
            .map_err(|_| "The workspace is no longer available.".to_owned())?;
        let scope = root
            .strip_prefix(repo)
            .map_err(|_| "Workspace is outside the repository.".to_owned())?;
        let old = file
            .old_path
            .as_ref()
            .filter(|path| path.starts_with(scope));
        // Rename matching is limited to these two explicit paths, never every
        // changed file in the repository.
        if old.is_some() {
            command.arg("--find-renames");
        }
        command.arg("HEAD").arg("--").arg(&file.path);
        if let Some(old) = old {
            command.arg(old);
        }
    }
    let (bytes, limited) = output(&mut command, PATCH_BYTES, cancel, no_index)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = if limited {
        text.rsplit_once('\n').map_or("", |(complete, _)| complete)
    } else {
        &text
    };
    Ok(Patch::new(parse_patch(text, &file.path), limited))
}
fn hunk_numbers(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "@@" {
        return None;
    }
    let old = parts
        .next()?
        .strip_prefix('-')?
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new = parts
        .next()?
        .strip_prefix('+')?
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old, new))
}
fn parse_patch(text: &str, path: &Path) -> Vec<DiffLine> {
    let language = Language::for_path(path);
    let mut old_lexer = language.map(Lexer::new);
    let mut new_lexer = language.map(Lexer::new);
    let (mut old, mut new) = (0, 0);
    let mut in_hunk = false;
    let mut lines = Vec::new();
    for raw in text.lines() {
        if raw.starts_with("diff --git ") {
            in_hunk = false;
            continue;
        }
        if let Some((a, b)) = hunk_numbers(raw) {
            old = a;
            new = b;
            in_hunk = true;
            old_lexer = language.map(Lexer::new);
            new_lexer = language.map(Lexer::new);
            lines.push(DiffLine {
                kind: LineKind::Hunk,
                old: None,
                new: None,
                text: raw.chars().take(256).collect(),
                highlights: vec![],
            });
            continue;
        }
        if !in_hunk {
            if [
                "old mode ",
                "new mode ",
                "new file mode ",
                "deleted file mode ",
                "rename from ",
                "rename to ",
                "Submodule ",
            ]
            .iter()
            .any(|prefix| raw.starts_with(prefix))
            {
                lines.push(DiffLine {
                    kind: LineKind::Note,
                    old: None,
                    new: None,
                    text: raw.chars().take(256).collect(),
                    highlights: vec![],
                });
            }
            if raw.starts_with("Binary files ") || raw == "GIT binary patch" {
                lines.push(DiffLine {
                    kind: LineKind::Note,
                    old: None,
                    new: None,
                    text: "Binary file changed · open it in Files to preview.".into(),
                    highlights: vec![],
                });
            }
            continue;
        }
        let (kind, before, after) = match raw.as_bytes().first() {
            Some(b'+') => {
                let n = new;
                new += 1;
                (LineKind::Added, None, Some(n))
            }
            Some(b'-') => {
                let n = old;
                old += 1;
                (LineKind::Removed, Some(n), None)
            }
            Some(b' ') => {
                let a = old;
                let b = new;
                old += 1;
                new += 1;
                (LineKind::Context, Some(a), Some(b))
            }
            Some(b'\\') => {
                lines.push(DiffLine {
                    kind: LineKind::Note,
                    old: None,
                    new: None,
                    text: "No newline at end of file".into(),
                    highlights: vec![],
                });
                continue;
            }
            _ => continue,
        };
        let source = &raw[1..];
        let mut start = 0;
        loop {
            let mut end = (start + LINE_BYTES).min(source.len());
            while !source.is_char_boundary(end) {
                end -= 1;
            }
            let text = &source[start..end];
            let highlights = match kind {
                LineKind::Added => color_line(&mut new_lexer, text),
                LineKind::Removed => color_line(&mut old_lexer, text),
                _ => {
                    color_line(&mut old_lexer, text);
                    color_line(&mut new_lexer, text)
                }
            };
            lines.push(DiffLine {
                kind,
                old: if start == 0 { before } else { None },
                new: if start == 0 { after } else { None },
                text: text.to_owned(),
                highlights,
            });
            start = end;
            if start == source.len() {
                break;
            }
        }
        if kind != LineKind::Added
            && let Some(lexer) = &mut old_lexer
        {
            lexer.end_line();
        }
        if kind != LineKind::Removed
            && let Some(lexer) = &mut new_lexer
        {
            lexer.end_line();
        }
    }
    if lines.is_empty() {
        lines.push(DiffLine {
            kind: LineKind::Note,
            old: None,
            new: None,
            text: "No textual line changes in this file.".into(),
            highlights: vec![],
        });
    }
    lines
}
fn color_line(lexer: &mut Option<Lexer>, text: &str) -> Vec<(std::ops::Range<usize>, u32)> {
    lexer
        .as_mut()
        .map_or_else(Vec::new, |lexer| lexer.highlight(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    struct Repo(PathBuf);
    impl Repo {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("code-diff-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let repo = Self(root);
            repo.run(&["init", "-q"]);
            repo.run(&["config", "user.email", "fixture@example.invalid"]);
            repo.run(&["config", "user.name", "Fixture"]);
            repo.run(&["config", "core.hooksPath", "/dev/null"]);
            repo.run(&["config", "commit.gpgsign", "false"]);
            repo
        }
        fn run(&self, args: &[&str]) {
            let status = git(&self.0)
                .args(args)
                .stdout(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "{args:?}");
        }
        fn write(&self, name: &str, data: &str) {
            let path = self.0.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, data).unwrap();
        }
        fn commit(&self) {
            self.run(&["add", "."]);
            self.run(&["commit", "-qm", "Fixture"]);
        }
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn prompt_review_uses_each_pass_snapshot_and_preserves_prior_pass_on_undo() {
        let repo = Repo::new();
        repo.write("first.rs", "fn first() { old(); }\n");
        repo.write("second.rs", "fn second() { old(); }\n");
        repo.commit();
        let storage = repo
            .0
            .parent()
            .unwrap()
            .join(format!("code-review-snapshots-{}", uuid::Uuid::new_v4()));
        let snapshots = crate::checkpoints::Repository::new(storage.clone()).unwrap();
        let cancel = AtomicBool::new(false);
        let first = snapshots
            .capture_before(&repo.0, "chat", "first", "First prompt", &cancel)
            .unwrap();
        repo.write("first.rs", "fn first() { first_pass(); }\n");
        snapshots.seal(&first, &cancel).unwrap();
        let second = snapshots
            .capture_before(&repo.0, "chat", "second", "Second prompt", &cancel)
            .unwrap();
        repo.write("second.rs", "fn second() { second_pass(); }\n");
        snapshots.seal(&second, &cancel).unwrap();
        // Newer live edits cannot contaminate the immutable recorded preview.
        repo.write("unrelated.rs", "newer editor text\n");
        let source = ReviewSource::Checkpoint {
            repository: snapshots.clone(),
            id: second.clone(),
            session: "chat".into(),
            reported: None,
        };
        let (changes, preview) = review_changes(&source, &repo.0, 0, &cancel).unwrap();
        assert_eq!(preview.unwrap().total, 1);
        assert_eq!(changes.files[0].path, Path::new("second.rs"));
        let patch = checkpoint_patch(
            snapshots
                .diff_page(&second, Path::new("second.rs"), Default::default(), &cancel)
                .unwrap(),
            Path::new("second.rs"),
        );
        assert!(patch.lines.iter().any(|line| line.kind == LineKind::Removed
            && line.text.contains("old()")
            && line.old == Some(1)));
        assert!(patch.lines.iter().any(|line| line.kind == LineKind::Added
            && line.text.contains("second_pass()")
            && line.new == Some(1)
            && !line.highlights.is_empty()));
        assert!(
            !patch
                .lines
                .iter()
                .any(|line| line.text.contains("first_pass") || line.text.contains("newer editor"))
        );
        snapshots
            .apply(&second, crate::checkpoints::Direction::Undo, &cancel)
            .unwrap();
        assert!(
            std::fs::read_to_string(repo.0.join("first.rs"))
                .unwrap()
                .contains("first_pass")
        );
        assert!(
            std::fs::read_to_string(repo.0.join("second.rs"))
                .unwrap()
                .contains("old()")
        );
        assert!(
            std::fs::read_to_string(repo.0.join("unrelated.rs"))
                .unwrap()
                .contains("newer editor")
        );
        snapshots
            .apply(&second, crate::checkpoints::Direction::Redo, &cancel)
            .unwrap();
        assert!(
            std::fs::read_to_string(repo.0.join("second.rs"))
                .unwrap()
                .contains("second_pass")
        );
        let forged = ReviewSource::Checkpoint {
            repository: snapshots,
            id: second,
            session: "other-chat".into(),
            reported: None,
        };
        assert!(review_changes(&forged, &repo.0, 0, &cancel).is_err());
        std::fs::remove_dir_all(storage).unwrap();
    }
    #[test]
    fn prompt_native_adapter_keeps_binary_notes_and_pages_large_file_lists() {
        let repo = Repo::new();
        let storage = repo
            .0
            .parent()
            .unwrap()
            .join(format!("code-review-pages-{}", uuid::Uuid::new_v4()));
        let snapshots = crate::checkpoints::Repository::new(storage.clone()).unwrap();
        let cancel = AtomicBool::new(false);
        let id = snapshots
            .capture_before(&repo.0, "chat", "many", "Many files", &cancel)
            .unwrap();
        for ix in 0..70 {
            repo.write(&format!("file-{ix:02}.rs"), "pub fn added() {}\n");
        }
        std::fs::write(repo.0.join("image.png"), [0, 255, 1, 2]).unwrap();
        snapshots.seal(&id, &cancel).unwrap();
        let source = ReviewSource::Checkpoint {
            repository: snapshots.clone(),
            id: id.clone(),
            session: "chat".into(),
            reported: None,
        };
        let (first, preview) = review_changes(&source, &repo.0, 0, &cancel).unwrap();
        assert_eq!(first.files.len(), 64);
        assert_eq!(preview.unwrap().total, 71);
        let (last, _) = review_changes(&source, &repo.0, 64, &cancel).unwrap();
        assert_eq!(last.files.len(), 7);
        assert!(
            !first
                .files
                .iter()
                .any(|f| last.files.iter().any(|other| other.path == f.path))
        );
        let patch = checkpoint_patch(
            snapshots
                .diff_page(&id, Path::new("image.png"), Default::default(), &cancel)
                .unwrap(),
            Path::new("image.png"),
        );
        assert!(
            patch
                .lines
                .iter()
                .any(|line| line.kind == LineKind::Note
                    && line.text.to_lowercase().contains("binary"))
        );
        assert!(!patch.lines.iter().any(|line| line.text.contains("HEAD")));
        std::fs::remove_dir_all(storage).unwrap();
    }
    #[test]
    fn reported_native_diff_keeps_file_sections_and_renames_without_workspace_reads() {
        let text = "diff --git a/old.rs b/new.rs\nrename from old.rs\nrename to new.rs\n--- a/old.rs\n+++ b/new.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/deleted.rs b/deleted.rs\n--- a/deleted.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n";
        let source = ReviewSource::Provider(Arc::from(text));
        let (changes, preview) = review_changes(
            &source,
            Path::new("/does-not-exist"),
            0,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            preview.is_none(),
            "Provider text never grants Undo authority"
        );
        assert_eq!(changes.files.len(), 2);
        assert_eq!(changes.files[0].path, Path::new("new.rs"));
        assert_eq!(changes.files[1].path, Path::new("deleted.rs"));
        let sections = provider_sections(text);
        let first = parse_patch(sections[0], &changes.files[0].path);
        assert!(
            first
                .iter()
                .any(|line| line.kind == LineKind::Added && line.text == "new")
        );
        assert!(!first.iter().any(|line| line.text == "gone"));
    }

    #[test]
    fn lazy_git_review_includes_staged_unstaged_deleted_and_untracked_without_mutation() {
        let repo = Repo::new();
        repo.write("code/🦀 main.rs", "fn main() { println!(\"old\"); }\n");
        repo.write("code/delete.rs", "fn gone() {}\n");
        repo.write("other.rs", "fn untouched() {}\n");
        repo.commit();
        repo.write("code/🦀 main.rs", "fn main() { println!(\"staged\"); }\n");
        repo.run(&["add", "code/🦀 main.rs"]);
        repo.write("code/🦀 main.rs", "fn main() { println!(\"working\"); }\n");
        repo.write("code/added.rs", "pub fn added() {}\n");
        repo.write("other.rs", "fn changed_elsewhere() {}\n");
        std::fs::remove_file(repo.0.join("code/delete.rs")).unwrap();
        let cancel = AtomicBool::new(false);
        let root = repo.0.join("code");
        let changes = read_changes(&root, &cancel).unwrap();
        assert!(changes.head);
        assert_eq!(
            changes.files.len(),
            3,
            "A subproject must exclude sibling changes"
        );
        let file = changes
            .files
            .iter()
            .find(|f| f.label == "🦀 main.rs")
            .unwrap();
        let patch =
            read_patch(&root, &changes.repo, true, file, &changes.filters, &cancel).unwrap();
        assert!(
            patch
                .lines
                .iter()
                .any(|l| l.kind == LineKind::Removed && l.text.contains("old"))
        );
        assert!(patch.lines.iter().any(|l| l.kind == LineKind::Added
            && l.text.contains("working")
            && !l.highlights.is_empty()));
        assert!(!patch.lines.iter().any(|l| l.text.contains("staged")));
        let added = changes.files.iter().find(|f| f.untracked).unwrap();
        assert!(
            read_patch(&root, &changes.repo, true, added, &changes.filters, &cancel)
                .unwrap()
                .lines
                .iter()
                .any(|l| l.kind == LineKind::Added)
        );
        let deleted = changes
            .files
            .iter()
            .find(|f| f.status == "Deleted")
            .unwrap();
        assert!(
            read_patch(
                &root,
                &changes.repo,
                true,
                deleted,
                &changes.filters,
                &cancel
            )
            .unwrap()
            .lines
            .iter()
            .any(|l| l.kind == LineKind::Removed)
        );
        let (staged, _) = output(
            git(&repo.0).args(["show", ":code/🦀 main.rs"]),
            4096,
            &cancel,
            false,
        )
        .unwrap();
        assert!(
            String::from_utf8(staged).unwrap().contains("staged"),
            "Review must not alter the index"
        );
        assert!(
            std::fs::read_to_string(repo.0.join("code/🦀 main.rs"))
                .unwrap()
                .contains("working")
        );
    }
    #[test]
    fn unborn_repository_and_binary_files_have_honest_previews() {
        let repo = Repo::new();
        repo.write("new.rs", "let x = 3;\n");
        repo.run(&["add", "new.rs"]);
        let cancel = AtomicBool::new(false);
        let changes = read_changes(&repo.0, &cancel).unwrap();
        assert!(!changes.head);
        let patch = read_patch(
            &repo.0,
            &changes.repo,
            false,
            &changes.files[0],
            &changes.filters,
            &cancel,
        )
        .unwrap();
        assert!(
            patch
                .lines
                .iter()
                .any(|l| l.kind == LineKind::Added && l.new == Some(1))
        );
        std::fs::write(repo.0.join("media.bin"), [0, 1, 0, 2]).unwrap();
        let changes = read_changes(&repo.0, &cancel).unwrap();
        let file = changes
            .files
            .iter()
            .find(|f| f.label == "media.bin")
            .unwrap();
        let patch = read_patch(
            &repo.0,
            &changes.repo,
            false,
            file,
            &changes.filters,
            &cancel,
        )
        .unwrap();
        assert!(
            patch
                .lines
                .iter()
                .any(|l| l.kind == LineKind::Note && l.text.contains("Binary"))
        );
    }
    #[test]
    fn diff_gutters_utf8_and_independent_lexers_survive_metadata_like_source() {
        let text = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -7,2 +9,2 @@\n-// removed comment\n+pub fn 🦀() {}\n---source\n+let count = 42;\n\\ No newline at end of file\n";
        let rows = parse_patch(text, Path::new("a.rs"));
        let added = rows.iter().find(|r| r.text.contains("pub fn")).unwrap();
        assert_eq!((added.old, added.new), (None, Some(9)));
        assert!(
            added
                .highlights
                .iter()
                .any(|(_, c)| *c == crate::syntax::KEYWORD)
        );
        let metadata_like = rows.iter().find(|r| r.text == "--source").unwrap();
        assert_eq!(metadata_like.kind, LineKind::Removed);
        assert_eq!(metadata_like.old, Some(8));
        assert_eq!(
            rows.iter().find(|r| r.text.contains("count")).unwrap().new,
            Some(10)
        );
        for row in rows {
            for (range, _) in row.highlights {
                assert!(row.text.get(range).is_some());
            }
        }
        let long = format!("@@ -0,0 +1 @@\n+{}\n", "🦀".repeat(3000));
        let rows = parse_patch(&long, Path::new("a.rs"));
        assert!(rows.iter().all(|r| r.text.len() <= LINE_BYTES));
        assert_eq!(rows.iter().filter(|r| r.new == Some(1)).count(), 1);
    }
    #[test]
    fn path_lists_preserve_unicode_rename_and_reject_incomplete_or_escaping_paths() {
        let bytes =
            b"R  src/new.rs\0src/old.rs\0?? src/with space.rs\0 M ../outside.rs\0?? src/incomplete";
        let (files, limited) = parse_files(bytes, Path::new("src"), true);
        assert!(limited);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].status, "Renamed");
        assert_eq!(files[1].label, "with space.rs");
    }
    #[test]
    fn large_patch_is_bounded_and_untracked_symlinks_cannot_read_outside_workspace() {
        let repo = Repo::new();
        repo.write("large.rs", &"let x = 42;\n".repeat(40_000));
        let cancel = AtomicBool::new(false);
        let changes = read_changes(&repo.0, &cancel).unwrap();
        let patch = read_patch(
            &repo.0,
            &changes.repo,
            false,
            &changes.files[0],
            &changes.filters,
            &cancel,
        )
        .unwrap();
        assert!(patch.limited);
        assert!(patch.lines.iter().map(|l| l.text.len()).sum::<usize>() <= PATCH_BYTES);
        std::os::unix::fs::symlink("/etc/hosts", repo.0.join("outside.txt")).unwrap();
        let changes = read_changes(&repo.0, &cancel).unwrap();
        let file = changes
            .files
            .iter()
            .find(|f| f.label == "outside.txt")
            .unwrap();
        let patch = read_patch(
            &repo.0,
            &changes.repo,
            false,
            file,
            &changes.filters,
            &cancel,
        )
        .unwrap();
        assert_eq!(patch.lines.len(), 1);
        assert!(patch.lines[0].text.contains("destination is not read"));
    }
    #[test]
    fn cancellation_reaps_a_quiet_owned_process_without_waiting_for_stdout() {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let notifier = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            worker_cancel.store(true, Ordering::Release);
        });
        let started = Instant::now();
        let result = output(
            Command::new("/bin/sleep").arg("10").stdout(Stdio::piped()),
            4096,
            &cancel,
            false,
        );
        notifier.join().unwrap();
        assert!(result.unwrap_err().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn real_rename_preview_preserves_original_context_and_shows_only_changed_lines() {
        let repo = Repo::new();
        repo.write(
            "before.rs",
            "fn first() {}\nfn second() {}\nfn third() {}\nfn fourth() {}\n",
        );
        repo.commit();
        repo.run(&["mv", "before.rs", "after.rs"]);
        repo.write(
            "after.rs",
            "fn first() {}\nfn second() { let x = 42; }\nfn third() {}\nfn fourth() {}\n",
        );
        let cancel = AtomicBool::new(false);
        let changes = read_changes(&repo.0, &cancel).unwrap();
        assert_eq!(changes.files.len(), 1);
        let file = &changes.files[0];
        assert_eq!(file.old_path.as_deref(), Some(Path::new("before.rs")));
        let patch = read_patch(
            &repo.0,
            &changes.repo,
            true,
            file,
            &changes.filters,
            &cancel,
        )
        .unwrap();
        assert!(
            patch
                .lines
                .iter()
                .any(|l| l.kind == LineKind::Context && l.text.contains("first"))
        );
        assert!(
            patch
                .lines
                .iter()
                .any(|l| l.kind == LineKind::Removed && l.text.contains("second"))
        );
        assert_eq!(
            patch
                .lines
                .iter()
                .filter(|l| l.kind == LineKind::Added)
                .count(),
            1
        );
    }
    #[test]
    fn git_filters_cannot_start_configured_helpers_during_review() {
        let repo = Repo::new();
        repo.write(".gitattributes", "*.rs filter=fixture\n");
        repo.write("main.rs", "fn before() {}\n");
        repo.commit();
        repo.run(&["config", "filter.fixture.clean", "/usr/bin/false"]);
        repo.run(&["config", "filter.fixture.required", "true"]);
        repo.write("main.rs", "fn after() {}\n");
        let cancel = AtomicBool::new(false);
        let changes = read_changes(&repo.0, &cancel).unwrap();
        assert!(
            changes
                .filters
                .iter()
                .any(|key| key == "filter.fixture.clean=")
        );
        let patch = read_patch(
            &repo.0,
            &changes.repo,
            true,
            &changes.files[0],
            &changes.filters,
            &cancel,
        )
        .unwrap();
        assert!(
            patch
                .lines
                .iter()
                .any(|line| line.kind == LineKind::Added && line.text.contains("after"))
        );
    }
    #[test]
    fn mode_changes_are_visible_and_multiple_patch_headers_are_never_source_lines() {
        let text = "diff --git a/a.rs b/a.rs\nold mode 100644\nnew mode 100755\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/c.rs b/c.rs\n--- a/c.rs\n+++ b/c.rs\n@@ -1 +1 @@\n-second\n+third\n";
        let patch = Patch::new(parse_patch(text, Path::new("a.rs")), false);
        assert!(
            patch
                .lines
                .iter()
                .any(|line| line.kind == LineKind::Note && line.text == "new mode 100755")
        );
        assert!(
            !patch
                .lines
                .iter()
                .any(|line| line.text == "-- a/c.rs" || line.text == "++ b/c.rs")
        );
        assert_eq!(
            patch
                .lines
                .iter()
                .filter(|line| line.kind == LineKind::Added)
                .count(),
            2
        );
        let long = Patch::new(
            parse_patch(
                &format!("@@ -0,0 +1 @@\n+{}\n", "x".repeat(2000)),
                Path::new("a.rs"),
            ),
            false,
        );
        assert!(long.width >= 2000. * 14. + 110.);
    }
}

#[cfg(test)]mod native_tree_tests{
 use super::*;use crate::{ui_tests,MessageBlock,Row};use std::prelude::v1::test;
 #[gpui::test]fn native_change_tree_has_directory_controls_and_per_file_counts(cx:&mut gpui::TestAppContext){
  let(view,cx)=ui_tests::fixture(cx);let patch="diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1,2 @@\n-old\n+new\n+extra\ndiff --git a/src/nested/b.rs b/src/nested/b.rs\n--- a/src/nested/b.rs\n+++ b/src/nested/b.rs\n@@ -1 +1 @@\n-before\n+after\n";
  view.update(cx,|app,cx|{let review=cx.new(|cx|DiffView::provider(app.project.clone(),Arc::from(patch),cx));review.update(cx,|r,_|r.set_motion(false));app.diff_view=Some(review);app.diff_started=Some(Instant::now()-Duration::from_secs(1));app.rows=vec![Row{role:Some("assistant:codex".into()),block:MessageBlock::Paragraph("Reviewed changes".into())}];app.list.reset(1);cx.notify();});
  cx.run_until_parked();ui_tests::draw_frame(&view,cx);
  assert!(cx.debug_bounds("diff-directory-src").is_some());assert!(cx.debug_bounds("diff-directory-src/nested").is_some());assert!(cx.debug_bounds("changed-file-0").is_some());
  view.update(cx,|app,cx|{let review=app.diff_view.as_ref().unwrap().read(cx);assert_eq!(review.changes.as_ref().unwrap().files[0].counts.as_ref().unwrap().added,2);assert_eq!(review.tree.len(),4);});
  ui_tests::click_selector(cx,"diff-directory-src");ui_tests::draw_frame(&view,cx);view.update(cx,|app,cx|assert_eq!(app.diff_view.as_ref().unwrap().read(cx).tree.len(),1));
  ui_tests::click_selector(cx,"diff-directory-src");ui_tests::draw_frame(&view,cx);view.update(cx,|app,cx|assert_eq!(app.diff_view.as_ref().unwrap().read(cx).tree.len(),4));
 }
}

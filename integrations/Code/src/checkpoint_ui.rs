//! Native prompt timeline and explicit file restore previews. Filesystem work is
//! performed by workers; provider text can name a checkpoint but cannot run it.
use super::*;
use checkpoints::{Checkpoint, Direction, Preview, Repository};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct Timeline {
    session: String,
    anchor: Point<Pixels>,
    started: Instant,
    entries: Vec<Checkpoint>,
    offset: usize,
    loading: bool,
    selected: Option<Preview>,
    preview_offset: usize,
    error: Option<String>,
    forget: Option<Checkpoint>,
    request: uuid::Uuid,
    diff: Option<(PathBuf, crate::checkpoint_diff::Page)>,
    diff_scroll: UniformListScrollHandle,
    diff_cancel: Option<DiffCancel>,
    pending_preview: Option<(String, usize)>,
}
struct DiffCancel(Arc<AtomicBool>);
impl Drop for DiffCancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl Timeline {
    pub(super) fn animating(&self) -> bool {
        self.started.elapsed() < Duration::from_millis(320)
    }
}
/// Called only from a worker, after the display prompt has been separated from
/// provider context. An incomplete checkpoint stops launch rather than lying
/// about Undo being available.
pub(super) fn launch_checkpointed(
    repository: &Repository,
    request: RunRequest,
    session: &str,
    prompt_key: &str,
    title: &str,
    native_command: bool,
    cancel: &AtomicBool,
) -> anyhow::Result<(RunHandle, String)> {
    let checkpoint = if request.allow_edits {
        repository.capture_before(&request.project, session, prompt_key, title, cancel)?
    } else {
        repository.record_read_only(&request.project, session, prompt_key, title)?
    };
    if cancel.load(Ordering::Acquire) {
        repository.abandon(&checkpoint)?;
        anyhow::bail!("Launch canceled before the agent started");
    }
    let result = if native_command {
        runtime::launch_cli_command(request)
    } else {
        runtime::launch(request)
    };
    match result {
        Ok(handle) => Ok((handle, checkpoint)),
        Err(error) => {
            let _ = repository.abandon(&checkpoint);
            Err(error)
        }
    }
}
pub(super) fn row_checkpoint(row: &Row) -> Option<&str> {
    match &row.block {
        MessageBlock::Activity { label, details }
            if label == "Prompt changes" && uuid::Uuid::parse_str(details).is_ok() =>
        {
            Some(details)
        }
        _ => None,
    }
}
/// A painted completion retains its identity even when older history is
/// prepended or decoded pages are evicted before its click is dispatched.
#[derive(Clone)]
pub(super) struct CompletionTarget {
    session: String,
    pub(super) root: PathBuf,
    checkpoint: Option<String>,
    reported: Option<Arc<str>>,
}
impl Orbit {
    pub(super) fn checkpoint_for_completion(&self, ix: usize) -> Option<String> {
        let mut checkpoint = None;
        for number in (0..ix.min(self.rows.len())).rev() {
            if let Some(deferred) = self.deferred_rows.get(&number) {
                if let Some(id) = &deferred.checkpoint {
                    checkpoint = Some(id.clone());
                    break;
                }
                if deferred.user_boundary {
                    break;
                }
            } else {
                if let Some(id) = row_checkpoint(&self.rows[number]) {
                    checkpoint = Some(id.to_owned());
                    break;
                }
                if self.rows[number].role.as_deref() == Some("user") {
                    break;
                }
            }
        }
        checkpoint
    }
    pub(super) fn completion_target(&self, ix: usize) -> Option<CompletionTarget> {
        let session = self.active.clone()?;
        let scope = self.checkpoint_scope(&session);
        let checkpoint = scope
            .as_ref()
            .and_then(|_| self.checkpoint_for_completion(ix));
        let reported = if checkpoint
            .as_ref()
            .is_some_and(|id| self.checkpoint_actions.get(id).is_some_and(Option::is_some))
        {
            None
        } else {
            self.provider_diff_for_completion(ix)
        };
        Some(CompletionTarget {
            session,
            root: scope.unwrap_or_else(|| {
                self.execution_root()
                    .unwrap_or_else(|| self.project.clone())
            }),
            checkpoint,
            reported,
        })
    }
    pub(super) fn open_completion_target(
        &mut self,
        target: CompletionTarget,
        confirm: bool,
        cx: &mut Context<Self>,
    ) {
        let current_root = self.checkpoint_scope(&target.session).unwrap_or_else(|| {
            self.execution_root()
                .unwrap_or_else(|| self.project.clone())
        });
        if self.active.as_ref() != Some(&target.session) || current_root != target.root {
            self.notice = Some(
                "The chat or workspace changed. Select its completion again to review its files."
                    .into(),
            );
            cx.notify();
            return;
        }
        let CompletionTarget {
            session,
            root,
            checkpoint,
            reported: provider,
        } = target;
        self.notice = None;
        let repository = self.checkpoint_repository.clone();
        self.broker_panel = None;
        self.files_visible = false;
        self.checkpoint_timeline = None;
        if let Some(browser) = &self.file_browser {
            browser.update(cx, |browser, cx| browser.set_visible(false, cx));
        }
        let review=cx.new(|cx| match (checkpoint,repository,provider) {
            (Some(id),Ok(repository),provider) => diff_view::DiffView::checkpoint(root,repository,id,session,provider,cx),
            (_,_,Some(diff)) => diff_view::DiffView::provider(root,diff,cx),
            _ => diff_view::DiffView::workspace_fallback(root,"This older response has no saved file snapshot or recorded agent diff. Reviewing the current working tree instead; historical Undo is unavailable.".into(),cx),
        });
        review.update(cx, |review, cx| {
            review.set_appearance(self.appearance, cx);
            review.set_motion(self.motion);
            if confirm {
                review.request_restore_confirmation(cx);
            }
        });
        self.diff_observer = Some(cx.subscribe(
            &review,
            |this, review, event: &diff_view::ReviewEvent, cx| {
                match event {
                    diff_view::ReviewEvent::Changed => {
                        if let Some((id, direction)) = review.read(cx).checkpoint_action() {
                            if this.checkpoint_actions.len() >= 256 {
                                this.checkpoint_actions.clear();
                            }
                            this.checkpoint_actions.insert(id, direction);
                        }
                        this.queue_workspace_save(cx);
                    }
                    diff_view::ReviewEvent::Restore {
                        id,
                        session,
                        root,
                        direction,
                    } => this.apply_review_checkpoint(
                        review.clone(),
                        id.clone(),
                        session.clone(),
                        root.clone(),
                        *direction,
                        cx,
                    ),
                }
                cx.notify();
            },
        ));
        self.diff_view = Some(review);
        self.diff_started = Some(Instant::now());
        cx.notify();
    }
    fn provider_diff_for_completion(&self, ix: usize) -> Option<Arc<str>> {
        for number in (0..ix.min(self.rows.len())).rev() {
            if let Some(deferred) = self.deferred_rows.get(&number) {
                if deferred.user_boundary {
                    break;
                }
                continue;
            }
            let row = &self.rows[number];
            if row.role.as_deref() == Some("user") {
                break;
            }
            if let MessageBlock::Interaction(chat_interactions::ChatInteraction::Status {
                title,
                details,
                ..
            }) = &row.block
                && title == "Changes"
                && details.len() <= 256 * 1024
                && details.starts_with("diff --git ")
            {
                return Some(Arc::from(details.as_str()));
            }
        }
        None
    }
    /// Warm only visible completion controls, at most four metadata jobs at a
    /// time. The render path never reads manifests or workspace files.
    pub(super) fn prefetch_completion_action(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(id) = self.checkpoint_for_completion(ix) else {
            return;
        };
        if self.checkpoint_actions.contains_key(&id) || self.checkpoint_action_loading >= 4 {
            return;
        }
        let Some(session) = self.active.clone() else {
            return;
        };
        let Some(root) = self.checkpoint_scope(&session) else {
            return;
        };
        if self.checkpoint_actions.len() >= 256 {
            self.checkpoint_actions.clear();
        }
        self.checkpoint_actions.insert(id.clone(), None);
        self.checkpoint_action_loading += 1;
        let repository = self.checkpoint_repository.clone();
        cx.spawn(async move |this, cx| {
            let expected = session.clone();
            let lookup = id.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository.and_then(|repository| {
                        let checkpoint = repository.get(&lookup).map_err(|e| e.to_string())?;
                        if checkpoint.session != expected
                            || std::fs::canonicalize(&root).ok().as_ref() != Some(&checkpoint.root)
                        {
                            return Err("Checkpoint ownership changed.".into());
                        }
                        if checkpoint.phase != "ready" && !checkpoint.phase.starts_with("applying-")
                        {
                            return Ok(None);
                        }
                        let direction = if checkpoint.undone {
                            Direction::Redo
                        } else {
                            Direction::Undo
                        };
                        repository
                            .preview(&lookup, direction, 0)
                            .map(|p| (p.total > 0).then_some(direction))
                            .map_err(|e| e.to_string())
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.checkpoint_action_loading = this.checkpoint_action_loading.saturating_sub(1);
                if this.active.as_deref() == Some(&session) {
                    this.checkpoint_actions.insert(id, result.ok().flatten());
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn completion_restore_action(&self, target: &CompletionTarget) -> Option<Direction> {
        self.checkpoint_actions
            .get(target.checkpoint.as_ref()?)
            .copied()
            .flatten()
    }
    fn restore_blocker(&self, cx: &Context<Self>) -> Option<String> {
        let root = self
            .execution_root()
            .unwrap_or_else(|| self.project.clone());
        if runtime::broker::has_active_children(&root) {
            return Some(
                "Stop subagents and wait for their changes before restoring a prompt.".into(),
            );
        }
        if self.quit_pending
            || self.starting
            || !self.team_starting.is_empty()
            || !self.runs.is_empty()
            || !self.settling.is_empty()
            || !self.finalizing.is_empty()
            || self.checkpoint_busy > 0
            || self.saved_terminal_decks.iter().flat_map(|deck| &deck.sessions)
                .any(|terminal| terminal.project == root)
            || self.all_terminals().any(|session| {
                let panel = session.panel.read(cx);
                !panel.is_settled() || panel.resume().is_some_and(|terminal| terminal.project == root)
            })
        {
            return Some("Stop agents and terminals and wait for their files to finish before restoring a prompt.".into());
        }
        None
    }
    fn apply_review_checkpoint(
        &mut self,
        review: Entity<diff_view::DiffView>,
        id: String,
        session: String,
        root: PathBuf,
        direction: Direction,
        cx: &mut Context<Self>,
    ) {
        if let Some(error) = self.restore_blocker(cx) {
            review.update(cx, |review, cx| review.restore_finished(Err(error), cx));
            return;
        }
        let scope = self.checkpoint_scope(&session);
        if self.active.as_deref() != Some(&session)
            || scope.as_ref() != Some(&root)
            || self.diff_view.as_ref() != Some(&review)
        {
            review.update(cx, |review, cx| {
                review.restore_finished(
                    Err("The active chat or workspace changed; files were not restored.".into()),
                    cx,
                )
            });
            return;
        }
        let repository = self.checkpoint_repository.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.checkpoint_cancel = cancel.clone();
        self.checkpoint_busy += 1;
        cx.spawn(async move |this, cx| {
            let verify_session = session.clone();
            let verify_id = id.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository.and_then(|repository| {
                        let checkpoint = repository.get(&verify_id).map_err(|e| e.to_string())?;
                        if checkpoint.session != verify_session
                            || std::fs::canonicalize(&root).ok().as_ref() != Some(&checkpoint.root)
                        {
                            return Err(
                                "Checkpoint ownership changed; files were not restored.".into()
                            );
                        }
                        repository
                            .apply(&verify_id, direction, &cancel)
                            .map_err(|e| e.to_string())
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.checkpoint_busy = this.checkpoint_busy.saturating_sub(1);
                if result.is_ok() {
                    this.checkpoint_actions.insert(
                        id,
                        Some(if direction == Direction::Undo {
                            Direction::Redo
                        } else {
                            Direction::Undo
                        }),
                    );
                }
                review.update(cx, |review, cx| review.restore_finished(result, cx));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn preview_latest_checkpoint(
        &mut self,
        direction: Direction,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.active.clone() else {
            return;
        };
        let Some(root) = self.checkpoint_scope(&session) else {
            return;
        };
        let repository = self.checkpoint_repository.clone();
        cx.spawn(async move |this, cx| {
            let owned_session = session.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository.and_then(|r| {
                        r.latest_completed(&owned_session, &root, direction)
                            .map_err(|e| e.to_string())
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.active.as_ref() != Some(&session) {
                    return;
                }
                match result {
                    Ok(Some(checkpoint)) => this.preview_checkpoint(checkpoint.id, anchor, 0, cx),
                    Ok(None) => {
                        this.notice = Some(
                            "No completed prompt with eligible file changes in this chat.".into(),
                        );
                        cx.notify();
                    }
                    Err(error) => {
                        this.notice = Some(error);
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
    fn preview_checkpoint_diff(
        &mut self,
        path: PathBuf,
        position: crate::checkpoint_diff::Position,
        cx: &mut Context<Self>,
    ) {
        let Some(panel) = self.checkpoint_timeline.as_ref() else {
            return;
        };
        let Some(preview) = &panel.selected else {
            return;
        };
        let session = panel.session.clone();
        let Some(root) = self.checkpoint_scope(&session) else {
            return;
        };
        let id = preview.checkpoint.id.clone();
        let request = uuid::Uuid::new_v4();
        if let Some(panel) = self.checkpoint_timeline.as_mut() {
            panel.request = request;
            panel.loading = true;
            panel.error = None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(panel) = self.checkpoint_timeline.as_mut() {
            panel.diff_cancel = Some(DiffCancel(cancel.clone()));
        }
        let repository = self.checkpoint_repository.clone();
        cx.spawn(async move |this, cx| {
            let owned_session = session.clone();
            let owned_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository.and_then(|r| {
                        let checkpoint = r.get(&id).map_err(|e| e.to_string())?;
                        if checkpoint.session != owned_session
                            || std::fs::canonicalize(root).ok().as_ref() != Some(&checkpoint.root)
                        {
                            return Err("Checkpoint belongs to another chat or workspace.".into());
                        }
                        r.diff_page(&id, &owned_path, position, &cancel)
                            .map_err(|e| e.to_string())
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(panel) = this
                    .checkpoint_timeline
                    .as_mut()
                    .filter(|p| p.session == session && p.request == request)
                {
                    panel.loading = false;
                    match result {
                        Ok(page) => {
                            panel.diff = Some((path, page));
                            panel.diff_scroll = UniformListScrollHandle::new();
                        }
                        Err(error) => panel.error = Some(error),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn seal_checkpoint_run(
        &mut self,
        session: &str,
        settled: async_channel::Receiver<()>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.checkpoint_runs.remove(session) else {
            return;
        };
        let Ok(repository) = self.checkpoint_repository.clone() else {
            return;
        };
        self.checkpoint_busy += 1;
        cx.spawn(async move |this, cx| {
            let sealed_id = id.clone();
            // Finished output does not mean sandbox descendants have exited.
            let _ = settled.recv().await;
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository
                        .seal(&id, &AtomicBool::new(false))
                        .map_err(|e| e.to_string())?;
                    let checkpoint = repository.get(&id).map_err(|e| e.to_string())?;
                    if checkpoint.phase == "ready" {
                        let direction = if checkpoint.undone {
                            Direction::Redo
                        } else {
                            Direction::Undo
                        };
                        repository
                            .preview(&id, direction, 0)
                            .map(|preview| (preview.total > 0).then_some(direction))
                            .map_err(|e| e.to_string())
                    } else {
                        Ok(None)
                    }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.checkpoint_busy = this.checkpoint_busy.saturating_sub(1);
                this.change_summaries.invalidate(&sealed_id);
                this.github_after_agent_turn(cx);
                let succeeded = result.is_ok();
                match result {
                    Ok(direction) => {
                        if this.checkpoint_actions.len() >= 256 {
                            this.checkpoint_actions.clear();
                        }
                        this.checkpoint_actions.insert(sealed_id.clone(), direction);
                    }
                    Err(error) => {
                        this.notice = Some(format!("Could not finish file checkpoint: {error}"))
                    }
                }
                if let Some(review) = &this.diff_view {
                    review.update(cx, |review, cx| review.refresh_checkpoint(&sealed_id, cx));
                }
                let pending = this.checkpoint_timeline.as_ref().and_then(|panel| {
                    panel
                        .pending_preview
                        .as_ref()
                        .filter(|(id, _)| id == &sealed_id)
                        .map(|(id, offset)| (id.clone(), *offset, panel.anchor))
                });
                if let Some((id, offset, anchor)) = pending.filter(|_| succeeded) {
                    this.preview_checkpoint(id, anchor, offset, cx);
                } else if this.checkpoint_timeline.is_some() {
                    this.reload_checkpoint_timeline(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn toggle_checkpoint_timeline(
        &mut self,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.checkpoint_timeline.is_some() {
            self.checkpoint_timeline = None;
            cx.notify();
            return;
        }
        let Some(session) = self.active.clone() else {
            self.notice = Some("Open a conversation to see its prompt history.".into());
            cx.notify();
            return;
        };
        self.checkpoint_timeline = Some(Timeline {
            session,
            anchor,
            started: Instant::now(),
            entries: vec![],
            offset: 0,
            loading: false,
            selected: None,
            preview_offset: 0,
            error: None,
            forget: None,
            request: uuid::Uuid::new_v4(),
            diff: None,
            diff_scroll: UniformListScrollHandle::new(),
            diff_cancel: None,
            pending_preview: None,
        });
        self.reload_checkpoint_timeline(cx);
    }
    fn reload_checkpoint_timeline(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.checkpoint_timeline.as_mut() else {
            return;
        };
        panel.loading = true;
        let request = uuid::Uuid::new_v4();
        panel.request = request;
        let session = panel.session.clone();
        let offset = panel.offset;
        let repository = self.checkpoint_repository.clone();
        cx.spawn(async move |this, cx| {
            let read_session = session.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository
                        .and_then(|r| r.list(&read_session, offset).map_err(|e| e.to_string()))
                })
                .await;
            let _ =
                this.update(cx, |this, cx| {
                    if let Some(panel) = this.checkpoint_timeline.as_mut().filter(|p| {
                        p.session == session && p.offset == offset && p.request == request
                    }) {
                        panel.loading = false;
                        match result {
                            Ok(entries) => {
                                panel.entries = entries;
                            }
                            Err(error) => panel.error = Some(error),
                        }
                    }
                    cx.notify();
                });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn checkpoint_scope(&self, session: &str) -> Option<PathBuf> {
        self.sessions.iter().find(|s| s.id == session).map(|s| {
            s.execution_path
                .clone()
                .unwrap_or_else(|| s.project_path.clone())
        })
    }
    pub(super) fn preview_checkpoint(
        &mut self,
        id: String,
        anchor: Point<Pixels>,
        offset: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.active.clone() else {
            return;
        };
        let Some(root) = self.checkpoint_scope(&session) else {
            return;
        };
        if self
            .checkpoint_timeline
            .as_ref()
            .is_none_or(|p| p.session != session)
        {
            self.checkpoint_timeline = Some(Timeline {
                session: session.clone(),
                anchor,
                started: Instant::now(),
                entries: vec![],
                offset: 0,
                loading: true,
                selected: None,
                preview_offset: offset,
                error: None,
                forget: None,
                request: uuid::Uuid::new_v4(),
                diff: None,
                diff_scroll: UniformListScrollHandle::new(),
                diff_cancel: None,
                pending_preview: None,
            });
        }
        let request = uuid::Uuid::new_v4();
        if let Some(panel) = self.checkpoint_timeline.as_mut() {
            panel.request = request;
            panel.pending_preview = Some((id.clone(), offset));
            panel.diff_cancel = None;
            panel.loading = true;
            panel.error = None;
        }
        let repository = self.checkpoint_repository.clone();
        cx.spawn(async move |this, cx| {
            let read_session = session.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository.and_then(|r| {
                        let checkpoint = r.get(&id).map_err(|e| e.to_string())?;
                        if checkpoint.session != read_session
                            || std::fs::canonicalize(&root).ok().as_ref() != Some(&checkpoint.root)
                        {
                            return Err(
                                "That checkpoint belongs to another conversation or workspace."
                                    .into(),
                            );
                        }
                        if matches!(checkpoint.phase.as_str(),"recording"|"sealing") {return Err("Finalizing this prompt’s files… the preview will open when cleanup finishes.".into());}
                        let direction = if checkpoint.undone {
                            Direction::Redo
                        } else {
                            Direction::Undo
                        };
                        if checkpoint.phase == "read-only" {
                            Ok(Preview {
                                checkpoint,
                                changes: vec![],
                                total: 0,
                            })
                        } else {
                            r.preview(&id, direction, offset).map_err(|e| e.to_string())
                        }
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(panel) = this
                    .checkpoint_timeline
                    .as_mut()
                    .filter(|p| p.session == session && p.request == request)
                {
                    panel.loading = false;
                    panel.preview_offset = offset;
                    match result {
                        Ok(preview) => {
                            panel.selected = Some(preview);
                            panel.pending_preview = None;
                            panel.diff = None;
                            panel.error = None
                        }
                        Err(error) => panel.error = Some(error),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn apply_checkpoint(&mut self, cx: &mut Context<Self>) {
        if let Some(error) = self.restore_blocker(cx) {
            self.notice = Some(error);
            cx.notify();
            return;
        }
        let Some(panel) = self.checkpoint_timeline.as_mut() else {
            return;
        };
        let Some(preview) = panel.selected.as_ref() else {
            return;
        };
        if self.active.as_deref() != Some(&panel.session) {
            return;
        }
        let id = preview.checkpoint.id.clone();
        let direction = if preview.checkpoint.undone {
            Direction::Redo
        } else {
            Direction::Undo
        };
        let session = panel.session.clone();
        let Some(root) = self.checkpoint_scope(&session) else {
            return;
        };
        let repository = self.checkpoint_repository.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        self.checkpoint_cancel = cancel.clone();
        self.checkpoint_busy += 1;
        if let Some(panel) = self.checkpoint_timeline.as_mut() {
            panel.loading = true;
        }
        cx.spawn(async move |this, cx| {
            let verify_session = session.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository.and_then(|r| {
                        let checkpoint = r.get(&id).map_err(|e| e.to_string())?;
                        if checkpoint.session != verify_session
                            || std::fs::canonicalize(&root).ok().as_ref() != Some(&checkpoint.root)
                        {
                            return Err(
                                "Checkpoint ownership changed; files were not restored.".into()
                            );
                        }
                        r.apply(&id, direction, &cancel).map_err(|e| e.to_string())
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.checkpoint_busy = this.checkpoint_busy.saturating_sub(1);
                if let Some(panel) = this
                    .checkpoint_timeline
                    .as_mut()
                    .filter(|p| p.session == session)
                {
                    panel.loading = false;
                    panel.selected = None;
                    panel.error = result.err();
                }
                this.reload_checkpoint_timeline(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn request_forget_checkpoint(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(session) = self.active.clone() else {
            return;
        };
        let repository = self.checkpoint_repository.clone();
        cx.spawn(async move |this, cx| {
            let expected = session.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository.and_then(|r| {
                        let checkpoint = r.get(&id).map_err(|e| e.to_string())?;
                        if checkpoint.session != expected {
                            return Err("This checkpoint belongs to another conversation.".into());
                        }
                        Ok(checkpoint)
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if let Some(panel) = this
                    .checkpoint_timeline
                    .as_mut()
                    .filter(|p| p.session == session)
                {
                    match result {
                        Ok(checkpoint) => panel.forget = Some(checkpoint),
                        Err(error) => panel.error = Some(error),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn forget_checkpoint(&mut self, cx: &mut Context<Self>) {
        if self.quit_pending || self.checkpoint_busy > 0 {
            return;
        }
        let Some(panel) = self.checkpoint_timeline.as_mut() else {
            return;
        };
        let Some(checkpoint) = panel.forget.take() else {
            return;
        };
        let session = panel.session.clone();
        if self.active.as_deref() != Some(&session) {
            return;
        }
        panel.loading = true;
        let repository = self.checkpoint_repository.clone();
        self.checkpoint_busy += 1;
        cx.spawn(async move |this, cx| {
            let expected = session.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    repository.and_then(|r| {
                        let current = r.get(&checkpoint.id).map_err(|e| e.to_string())?;
                        if current.session != expected {
                            return Err("Checkpoint ownership changed; backups were kept.".into());
                        }
                        r.forget(&current.id, &AtomicBool::new(false))
                            .map_err(|e| e.to_string())
                    })
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.checkpoint_busy = this.checkpoint_busy.saturating_sub(1);
                if result.is_ok() {
                    this.checkpoint_actions.clear();
                }
                if let Some(panel) = this
                    .checkpoint_timeline
                    .as_mut()
                    .filter(|p| p.session == session)
                {
                    panel.loading = false;
                    panel.selected = None;
                    panel.error = result.err();
                }
                this.reload_checkpoint_timeline(cx);
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn checkpoint_timeline_view(&self, cx: &Context<Self>) -> AnyElement {
        let Some(panel) = self.checkpoint_timeline.as_ref() else {
            return div().into_any_element();
        };
        let reveal = if self.motion {
            presentation::ease_out((panel.started.elapsed().as_secs_f32() / 0.32).min(1.))
                .clamp(0., 1.)
        } else {
            1.
        };
        let mut body = div().flex().flex_col().gap_2().min_h(px(0.));
        if let Some(preview) = &panel.selected {
            let redo = preview.checkpoint.undone;
            body=body.child(div().text_size(crate::theme::ui_px(12.)).font_weight(FontWeight::SEMIBOLD).child(preview.checkpoint.title.clone()))
                .child(div().text_size(crate::theme::ui_px(11.)).text_color(rgb(MUTED)).child(format!("{} changed files · {}",preview.total,if redo{"Restore the prompt's resulting files"}else{"Restore files from before this prompt"})))
                .child(div().id("checkpoint-files").max_h(px(170.)).overflow_y_scroll().children(preview.changes.iter().enumerate().map(|(ix,change)| { let path=change.path.clone(); div().id(("checkpoint-diff-file",ix)).debug_selector(move||format!("checkpoint-diff-file-{ix}")).flex().gap_2().px_2().py_1().rounded_md().cursor_pointer().hover(|s|s.bg(rgba(GLASS_HOVER))).text_size(crate::theme::ui_px(11.)).child(icon(files::icon_for_path(&change.path),12.,MUTED)).child(div().flex_1().min_w(px(0.)).text_ellipsis().child(change.path.to_string_lossy().into_owned())).child(div().text_color(rgb(MUTED)).child(change.kind)).on_click(cx.listener(move|this,_,_,cx|this.preview_checkpoint_diff(path.clone(),Default::default(),cx))) })))
                .child(div().text_size(crate::theme::ui_px(10.)).text_color(rgb(MUTED)).child("Workspace files only. Current edits are checked again before applying. External services and ignored caches are excluded."));
            if panel.preview_offset + preview.changes.len() < preview.total {
                let id = preview.checkpoint.id.clone();
                let offset = panel.preview_offset + checkpoints::PAGE;
                let anchor = panel.anchor;
                body = body.child(
                    div()
                        .id("checkpoint-next-files")
                        .p_2()
                        .cursor_pointer()
                        .text_size(crate::theme::ui_px(11.))
                        .child("Next changed files")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.preview_checkpoint(id.clone(), anchor, offset, cx)
                        })),
                );
            }
            if let Some((path, patch)) = &panel.diff {
                let patch_width = patch
                    .lines
                    .iter()
                    .map(|line| line.text.len())
                    .max()
                    .unwrap_or(0)
                    .saturating_add(2) as f32
                    * 6.;
                body = body
                    .child(
                        div()
                            .text_size(crate::theme::ui_px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(path.to_string_lossy().into_owned()),
                    )
                    .child(
                        div()
                            .id("checkpoint-diff-scroll-x")
                            .h(px(220.))
                            .overflow_x_scroll()
                            .child(
                                uniform_list(
                                    "checkpoint-diff-lines",
                                    patch.lines.len(),
                                    cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                                        let Some((_, page)) = this
                                            .checkpoint_timeline
                                            .as_ref()
                                            .and_then(|p| p.diff.as_ref())
                                        else {
                                            return vec![];
                                        };
                                        range
                                            .map(|ix| {
                                                let line = &page.lines[ix];
                                                let (prefix, color) = match line.kind {
                                                    crate::checkpoint_diff::Kind::Added => {
                                                        ("+", 0xaddbb6)
                                                    }
                                                    crate::checkpoint_diff::Kind::Removed => {
                                                        ("−", 0xf0acac)
                                                    }
                                                    crate::checkpoint_diff::Kind::Header => {
                                                        ("", 0x88bdf5)
                                                    }
                                                    crate::checkpoint_diff::Kind::Note => {
                                                        ("", MUTED)
                                                    }
                                                    _ => (" ", TEXT),
                                                };
                                                div()
                                                    .h(px(20.))
                                                    .font_family(crate::theme::mono_font())
                                                    .text_size(crate::theme::ui_px(10.))
                                                    .line_height(crate::theme::ui_line_height(20.))
                                                    .whitespace_nowrap()
                                                    .overflow_hidden()
                                                    .text_color(rgb(color))
                                                    .child(format!("{prefix}{}", line.text))
                                                    .into_any_element()
                                            })
                                            .collect()
                                    }),
                                )
                                .debug_selector(|| "checkpoint-diff-lines".into())
                                .track_scroll(panel.diff_scroll.clone())
                                .h(px(220.))
                                .w(px(patch_width.max(400.))),
                            ),
                    );
                if let Some(next) = patch.next {
                    let path = path.clone();
                    body = body.child(
                        div()
                            .id("checkpoint-next-diff-page")
                            .p_2()
                            .cursor_pointer()
                            .text_size(crate::theme::ui_px(11.))
                            .child("Next patch page")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preview_checkpoint_diff(path.clone(), next, cx)
                            })),
                    );
                }
            }
            body = body.child(
                div()
                    .id("apply-checkpoint")
                    .debug_selector(|| "apply-checkpoint".into())
                    .h(px(32.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .bg(rgba(GLASS_HOVER))
                    .text_size(crate::theme::ui_px(12.))
                    .child(if preview.total == 0 {
                        if preview.checkpoint.phase == "read-only" {
                            "Read-only prompt · no file snapshot"
                        } else {
                            "No file changes"
                        }
                    } else if panel.loading {
                        "Restoring…"
                    } else if redo {
                        "Redo these file changes"
                    } else {
                        "Undo these file changes"
                    })
                    .when(!panel.loading && preview.total > 0, |s| {
                        s.cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| this.apply_checkpoint(cx)))
                    }),
            );
        } else {
            body = body.child(
                div()
                    .id("checkpoint-entries")
                    .max_h(px(370.))
                    .overflow_y_scroll()
                    .children(panel.entries.iter().enumerate().map(|(ix, entry)| {
                        let id = entry.id.clone();
                        let prompt_key = entry.prompt_key.clone();
                        let anchor = panel.anchor;
                        let ready = entry.phase == "ready" || entry.phase.starts_with("applying-");
                        div()
                            .id(("checkpoint-entry", ix))
                            .debug_selector(move || format!("checkpoint-prompt-{prompt_key}"))
                            .p_2()
                            .rounded_md()
                            .text_size(crate::theme::ui_px(11.))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(entry.title.clone()),
                            )
                            .child(div().text_color(rgb(MUTED)).child(format!(
                                    "{} · {}",
                                    chrono::DateTime::from_timestamp_millis(entry.created)
                                        .map(|d| d.format("%b %d · %H:%M").to_string())
                                        .unwrap_or_default(),
                                    if ready {
                                        if entry.undone {
                                            "Redo available"
                                        } else {
                                            "Undo available"
                                        }
                                    } else {
                                        entry.phase.as_str()
                                    }
                                )))
                            .when(
                                matches!(
                                    entry.phase.as_str(),
                                    "ready"
                                        | "read-only"
                                        | "not-started"
                                        | "capture-failed"
                                        | "seal-failed"
                                        | "interrupted"
                                        | "forgetting"
                                ),
                                |s| {
                                    let forget_id = entry.id.clone();
                                    s.child(
                                        div()
                                            .id(("forget-checkpoint", ix))
                                            .debug_selector(move || {
                                                format!("forget-checkpoint-{ix}")
                                            })
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .text_size(crate::theme::ui_px(10.))
                                            .text_color(rgb(MUTED))
                                            .cursor_pointer()
                                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                            .child("Forget checkpoint…")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                this.request_forget_checkpoint(
                                                    forget_id.clone(),
                                                    cx,
                                                );
                                            })),
                                    )
                                },
                            )
                            .when(ready, |s| {
                                s.cursor_pointer()
                                    .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.preview_checkpoint(id.clone(), anchor, 0, cx)
                                    }))
                            })
                    })),
            );
            if panel.entries.len() == checkpoints::PAGE {
                body = body.child(
                    div()
                        .id("checkpoint-more")
                        .p_2()
                        .text_size(crate::theme::ui_px(11.))
                        .cursor_pointer()
                        .child("Older prompts")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(panel) = this.checkpoint_timeline.as_mut() {
                                panel.offset += checkpoints::PAGE;
                            }
                            this.reload_checkpoint_timeline(cx);
                        })),
                );
            }
            if panel.entries.is_empty() && !panel.loading {
                body=body.child(div().p_2().text_size(crate::theme::ui_px(11.)).text_color(rgb(MUTED)).child("Earlier prompts have no Code file checkpoint. New prompts are captured before launch."));
            }
        }
        if panel.loading && panel.selected.is_none() {
            body = body.child(
                div()
                    .p_2()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(MUTED))
                    .child("Loading prompt history…"),
            );
        }
        if let Some(error) = &panel.error {
            body = body.child(
                div()
                    .p_2()
                    .text_size(crate::theme::ui_px(11.))
                    .text_color(rgb(0xf2a6a6))
                    .child(error.clone()),
            );
        }
        if let Some(checkpoint) = &panel.forget {
            body=body.child(div().p_3().rounded_md().bg(rgba(GLASS_SURFACE)).flex().flex_col().gap_2()
                .child(div().text_size(crate::theme::ui_px(12.)).child(format!("Forget “{}”?",checkpoint.title)))
                .child(div().text_size(crate::theme::ui_px(11.)).text_color(rgb(MUTED)).child("Removes this prompt's private backups and Undo/Redo. Your workspace files and chat stay as they are."))
                .child(div().flex().gap_2().justify_end()
                    .child(div().id("cancel-forget-checkpoint").p_2().cursor_pointer().text_size(crate::theme::ui_px(11.)).child("Keep backups").on_click(cx.listener(|this,_,_,cx|{if let Some(panel)=this.checkpoint_timeline.as_mut(){panel.forget=None;}cx.notify();})))
                    .child(div().id("confirm-forget-checkpoint").debug_selector(||"confirm-forget-checkpoint".into()).p_2().rounded_md().bg(rgba(GLASS_HOVER)).cursor_pointer().text_size(crate::theme::ui_px(11.)).child("Forget checkpoint").on_click(cx.listener(|this,_,_,cx|this.forget_checkpoint(cx))))));
        }
        anchored()
            .position(panel.anchor + point(px(0.), px(6. * (1. - reveal))))
            .snap_to_window_with_margin(px(12.))
            .child(
                div()
                    .id("checkpoint-timeline")
                    .debug_selector(|| "checkpoint-timeline".into())
                    .on_mouse_down_out(cx.listener(|this,event:&MouseDownEvent,_,cx|{this.dismissed_on_press(control_clicks::Control::Timeline,event.position);this.checkpoint_timeline=None;cx.notify();}))
                    .w(px(430.))
                    .p_3()
                    .rounded_xl()
                    .bg(rgba(self.appearance.tint(0x242424dd)))
                    .border_1()
                    .border_color(rgba(0xffffff18))
                    .opacity(reveal)
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(crate::theme::ui_px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Prompt history"),
                            )
                            .child(
                                div()
                                    .id("close-checkpoint-timeline")
                                    .debug_selector(|| "close-checkpoint-timeline".into())
                                    .p_1()
                                    .cursor_pointer()
                                    .child(icon("close", 12., MUTED))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.checkpoint_timeline = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(body),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn duplicate_prompts_keep_distinct_durable_user_checkpoint_markers() {
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        for id in [&first, &second] {
            let text = format!("Same prompt\n\n<!--code-checkpoint:{id}-->");
            let blocks = presentation::message_blocks("user", &text);
            assert!(blocks.iter().any(|b|matches!(b,MessageBlock::Activity{label,details} if label=="Prompt changes"&&details==id)));
            assert!(
                !presentation::message_blocks("assistant", &text).iter().any(
                    |b| matches!(b,MessageBlock::Activity{label,..}if label=="Prompt changes")
                )
            );
        }
        assert_ne!(first, second);
    }
    #[gpui::test]
    fn completion_associations_survive_page_eviction_without_crossing_user_boundaries(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, _| {
            let ids = (0..9)
                .map(|_| uuid::Uuid::new_v4().to_string())
                .collect::<Vec<_>>();
            app.rows.clear();
            app.history_pages.clear();
            for (ix, id) in ids.iter().enumerate() {
                app.rows.extend([
                    Row {
                        role: Some("user".into()),
                        block: MessageBlock::Paragraph("Same prompt".into()),
                    },
                    Row {
                        role: None,
                        block: MessageBlock::Activity {
                            label: "Prompt changes".into(),
                            details: id.clone(),
                        },
                    },
                    Row {
                        role: Some("assistant".into()),
                        block: MessageBlock::Paragraph("Completed".into()),
                    },
                ]);
                app.history_pages.push(HistoryPage {
                    number: ix,
                    start: ix * 3,
                    len: 3,
                    resident: true,
                    used: ix as u64,
                });
            }
            app.list = ListState::new(app.rows.len(), ListAlignment::Top, px(200.));
            app.history_visible = 100..110;
            assert_eq!(app.checkpoint_for_completion(2), Some(ids[0].clone()));
            app.evict_history_pages();
            assert!(app.deferred_rows.contains_key(&1));
            assert_eq!(app.checkpoint_for_completion(2), Some(ids[0].clone()));
            assert_eq!(app.checkpoint_for_completion(26), Some(ids[8].clone()));
            app.rows[25] = Row {
                role: None,
                block: MessageBlock::Paragraph("No checkpoint".into()),
            };
            assert_eq!(
                app.checkpoint_for_completion(26),
                None,
                "Never borrow a previous prompt's checkpoint"
            );
        });
    }
    #[gpui::test]
    fn native_preview_requires_explicit_undo_and_redo_and_preserves_the_chat(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let (root, id) = cx.update(|_, cx| {
            view.update(cx, |app, _| {
                let root = app.project.clone();
                std::fs::write(root.join("file.rs"), "before").unwrap();
                let repository = app.checkpoint_repository.as_ref().unwrap();
                let id = repository
                    .capture_before(
                        &root,
                        "fixture-session",
                        "exact-prompt",
                        "Change one file",
                        &AtomicBool::new(false),
                    )
                    .unwrap();
                std::fs::write(root.join("file.rs"), "after").unwrap();
                repository.seal(&id, &AtomicBool::new(false)).unwrap();
                app.active = Some("fixture-session".into());
                (root, id)
            })
        });
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.preview_checkpoint(id.clone(), point(px(240.), px(100.)), 0, cx)
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if cx.update(|_, cx| {
                view.read(cx)
                    .checkpoint_timeline
                    .as_ref()
                    .is_some_and(|p| p.selected.is_some() && !p.loading)
            }) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            std::fs::read_to_string(root.join("file.rs")).unwrap(),
            "after",
            "Preview must never restore automatically"
        );
        crate::ui_tests::draw_frame(&view, cx);
        let button = cx.debug_bounds("apply-checkpoint").unwrap().center();
        cx.simulate_click(button, Modifiers::none());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if std::fs::read_to_string(root.join("file.rs")).unwrap() == "before"
                && cx.update(|_, cx| view.read(cx).checkpoint_busy == 0)
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                assert_eq!(app.active.as_deref(), Some("fixture-session"));
                app.preview_checkpoint(id.clone(), point(px(240.), px(100.)), 0, cx);
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if cx.update(|_, cx| {
                view.read(cx).checkpoint_timeline.as_ref().is_some_and(|p| {
                    p.selected.as_ref().is_some_and(|v| v.checkpoint.undone) && !p.loading
                })
            }) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        crate::ui_tests::draw_frame(&view, cx);
        let button = cx.debug_bounds("apply-checkpoint").unwrap().center();
        cx.simulate_click(button, Modifiers::none());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if std::fs::read_to_string(root.join("file.rs")).unwrap() == "after"
                && cx.update(|_, cx| view.read(cx).checkpoint_busy == 0)
            {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[gpui::test]
    fn foreign_checkpoint_ids_never_expose_a_restore_action(cx: &mut gpui::TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let (root, id) = cx.update(|_, cx| {
            view.update(cx, |app, _| {
                let root = app.project.clone();
                std::fs::write(root.join("file.rs"), "before").unwrap();
                let repository = app.checkpoint_repository.as_ref().unwrap();
                let id = repository
                    .capture_before(
                        &root,
                        "another-session",
                        "foreign",
                        "Foreign prompt",
                        &AtomicBool::new(false),
                    )
                    .unwrap();
                std::fs::write(root.join("file.rs"), "after").unwrap();
                repository.seal(&id, &AtomicBool::new(false)).unwrap();
                app.active = Some("fixture-session".into());
                (root, id)
            })
        });
        cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                app.preview_checkpoint(id.clone(), point(px(240.), px(100.)), 0, cx)
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if cx.update(|_, cx| {
                view.read(cx)
                    .checkpoint_timeline
                    .as_ref()
                    .is_some_and(|p| p.error.is_some() && !p.loading)
            }) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("apply-checkpoint").is_none());
        assert_eq!(
            std::fs::read_to_string(root.join("file.rs")).unwrap(),
            "after"
        );
    }
    #[gpui::test]
    fn forgetting_a_checkpoint_requires_its_own_explicit_confirmation(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let (root, id) = cx.update(|_, cx| {
            view.update(cx, |app, cx| {
                let root = app.project.clone();
                std::fs::write(root.join("file.rs"), "kept").unwrap();
                let repository = app.checkpoint_repository.as_ref().unwrap();
                let id = repository
                    .capture_before(
                        &root,
                        "fixture-session",
                        "exact",
                        "Test prompt",
                        &AtomicBool::new(false),
                    )
                    .unwrap();
                repository.seal(&id, &AtomicBool::new(false)).unwrap();
                app.active = Some("fixture-session".into());
                app.toggle_checkpoint_timeline(point(px(240.), px(100.)), cx);
                (root, id)
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if cx.update(|_, cx| {
                view.read(cx)
                    .checkpoint_timeline
                    .as_ref()
                    .is_some_and(|p| !p.entries.is_empty() && !p.loading)
            }) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        crate::ui_tests::draw_frame(&view, cx);
        let button = cx.debug_bounds("forget-checkpoint-0").unwrap().center();
        cx.simulate_click(button, Modifiers::none());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if cx.update(|_, cx| {
                view.read(cx)
                    .checkpoint_timeline
                    .as_ref()
                    .is_some_and(|p| p.forget.is_some())
            }) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        cx.update(|_, cx| {
            assert!(
                view.read(cx)
                    .checkpoint_repository
                    .as_ref()
                    .unwrap()
                    .get(&id)
                    .is_ok()
            )
        });
        crate::ui_tests::draw_frame(&view, cx);
        let confirm = cx
            .debug_bounds("confirm-forget-checkpoint")
            .unwrap()
            .center();
        cx.simulate_click(confirm, Modifiers::none());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            if cx.update(|_, cx| {
                view.read(cx)
                    .checkpoint_repository
                    .as_ref()
                    .unwrap()
                    .get(&id)
                    .is_err()
            }) {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            std::fs::read_to_string(root.join("file.rs")).unwrap(),
            "kept"
        );
    }
}

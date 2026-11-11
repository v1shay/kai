//! Persistent Git checkouts for chats whose original checkout has an active writer.
use anyhow::{Context as _, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    os::unix::{ffi::OsStringExt, fs::DirBuilderExt, io::AsRawFd},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Serialize, Deserialize)]
struct Workspace {
    source: PathBuf,
    relative: PathBuf,
    base: String,
    imported: Option<String>,
}

fn git(root: &Path) -> Command {
    let mut command = Command::new("git");
    crate::process_env::apply(&mut command);
    command
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "submodule.recurse=false",
        ]);
    command
}

fn checked(command: &mut Command) -> Result<Vec<u8>> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .context("Could not run Git")?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

fn path_output(mut value: Vec<u8>) -> PathBuf {
    if value.last() == Some(&b'\n') {
        value.pop();
    }
    PathBuf::from(OsString::from_vec(value))
}

fn oid(output: Vec<u8>) -> Result<String> {
    let output = String::from_utf8(output)?;
    let id = output.trim();
    ensure!(
        (id.len() == 40 || id.len() == 64) && id.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid Git object ID"
    );
    Ok(id.to_owned())
}

pub(super) fn directory(storage: &Path, session: &str) -> PathBuf {
    storage.join(format!("{:x}", Sha256::digest(session.as_bytes())))
}

pub(super) fn owns(storage: &Path, session: &str, execution: &Path) -> bool {
    execution.starts_with(directory(storage, session).join("tree"))
}

fn write_state(directory: &Path, state: &Workspace) -> Result<()> {
    let temp = directory.join("workspace.pending");
    fs::write(&temp, serde_json::to_vec(state)?)?;
    fs::rename(temp, directory.join("workspace.json"))?;
    Ok(())
}

fn lock(directory: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("lock"))?;
    // The lock is released on close, including process exit.
    ensure!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "This chat's worktree has an operation in progress"
    );
    Ok(file)
}

fn tree_at(directory: &Path, state: &Workspace) -> Result<PathBuf> {
    let tree = directory
        .join("tree")
        .canonicalize()
        .context("The chat's worktree is unavailable")?;
    let project = tree
        .join(&state.relative)
        .canonicalize()
        .context("The project's directory is missing from the worktree")?;
    ensure!(
        project.starts_with(&tree),
        "The worktree project resolves outside its checkout"
    );
    Ok(project)
}

pub(super) fn prepare(storage: &Path, session: &str, execution: &Path) -> Result<PathBuf> {
    let execution = execution.canonicalize()?;
    let source = path_output(
        checked(git(&execution).args(["rev-parse", "--show-toplevel"])).context(
            "Parallel editing requires a Git project with a commit. The draft is preserved.",
        )?,
    )
    .canonicalize()?;
    ensure!(
        !storage.starts_with(&source),
        "Worktree storage must be outside the project"
    );
    let relative = execution.strip_prefix(&source)?.to_owned();
    let directory = directory(storage, session);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)?;
    let _lock = lock(&directory)?;
    let state_path = directory.join("workspace.json");
    let state = if state_path.is_file() {
        let state: Workspace = serde_json::from_slice(&fs::read(state_path)?)?;
        ensure!(
            state.source == source && state.relative == relative,
            "The saved worktree belongs to a different project"
        );
        state
    } else {
        let base = oid(
            checked(git(&source).args(["rev-parse", "--verify", "HEAD^{commit}"]))
                .context("Parallel editing requires an initial Git commit")?,
        )?;
        let state = Workspace {
            source,
            relative,
            base,
            imported: None,
        };
        write_state(&directory, &state)?;
        state
    };
    let tree = directory.join("tree");
    if !tree.exists() {
        checked(
            git(&state.source)
                .args(["worktree", "add", "--detach", "--"])
                .arg(&tree)
                .arg(&state.base),
        )
        .context("Could not create the chat's worktree")?;
    }
    tree_at(&directory, &state)
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        for name in ["index", "index.lock", "changes.patch"] {
            let _ = fs::remove_file(self.0.join(name));
        }
        let _ = fs::remove_dir(&self.0);
    }
}

struct Import {
    directory: PathBuf,
    worktree: PathBuf,
    state: Workspace,
    current: String,
    patch: PathBuf,
    destination: (String, String),
    summary: String,
    _scratch: Scratch,
    _lock: File,
}

fn destination(root: &Path) -> Result<(String, String)> {
    let branch = checked(git(root).args(["symbolic-ref", "--quiet", "--short", "HEAD"]))
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_else(|_| "Detached HEAD".into());
    let head = oid(checked(git(root).args(["rev-parse", "--verify", "HEAD"]))?)?;
    Ok((branch, head))
}

/// Prepare one patch for review. The exact bytes shown in the summary are used
/// for apply, even if a later turn changes the source worktree.
fn prepare_import(
    storage: &Path,
    session: &str,
    repository: &crate::checkpoints::Repository,
) -> Result<Option<Import>> {
    let directory = directory(storage, session);
    let _lock = lock(&directory)?;
    let state: Workspace = serde_json::from_slice(&fs::read(directory.join("workspace.json"))?)?;
    tree_at(&directory, &state)?;
    let worktree = directory.join("tree");
    ensure!(
        !crate::runtime::broker::has_active_children(&state.source)
            && !crate::runtime::broker::has_active_children(&worktree),
        "Wait for subagents to finish before applying worktree changes"
    );
    let (scratch, current, patch) = repository.with_workspace_write(&worktree, || {
        let scratch = Scratch(directory.join(uuid::Uuid::new_v4().to_string()));
        fs::DirBuilder::new().mode(0o700).create(&scratch.0)?;
        let index = || {
            let mut command = git(&worktree);
            command.env("GIT_INDEX_FILE", scratch.0.join("index"));
            command
        };
        checked(index().args(["read-tree", "HEAD"]))?;
        checked(index().args(["add", "--all", "--", "."]))?;
        let current = oid(checked(index().arg("write-tree"))?)?;
        let from = state.imported.as_deref().unwrap_or(&state.base);
        let patch = scratch.0.join("changes.patch");
        let status = git(&worktree)
            .args([
                "diff",
                "--binary",
                "--full-index",
                "--no-renames",
                "--no-ext-diff",
                "--no-textconv",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ])
            .args([from, &current, "--"])
            .stdout(File::create(&patch)?)
            .status()?;
        ensure!(status.success(), "Could not prepare the worktree changes");
        Ok((scratch, current, patch))
    })?;
    if fs::metadata(&patch)?.len() == 0 {
        return Ok(None);
    }
    let summary = checked(
        git(&worktree)
            .args(["apply", "--stat", "--summary"])
            .arg(&patch),
    )?;
    let summary = String::from_utf8_lossy(&summary)
        .lines()
        .take(20)
        .collect::<Vec<_>>()
        .join("\n");
    let destination = destination(&state.source)?;
    // Check destination ownership before asking for confirmation and again
    // before writing, so busy checkouts never receive an import.
    repository.with_workspace_write(&state.source, || Ok(()))?;
    Ok(Some(Import {
        directory,
        worktree,
        state,
        current,
        patch,
        destination,
        summary,
        _scratch: scratch,
        _lock,
    }))
}

fn apply_import(
    import: Import,
    session: &str,
    repository: &crate::checkpoints::Repository,
) -> Result<()> {
    let Import {
        directory,
        worktree,
        mut state,
        current,
        patch,
        destination: expected,
        _scratch,
        _lock,
        ..
    } = import;
    repository.with_workspace_write(&state.source, || {
        ensure!(destination(&state.source)? == expected,
            "The destination branch or commit changed while you reviewed it. Review the worktree changes again.");
        ensure!(!crate::runtime::broker::has_active_children(&state.source),
            "Wait for subagents to finish before applying worktree changes");
        let check = |reverse: bool| {
            let mut command = git(&state.source);
            command.args(["apply", "--check", "--binary", "--whitespace=nowarn"]);
            if reverse {
                command.arg("--reverse");
            }
            checked(command.arg(&patch))
        };
        match check(false) {
            Ok(_) => {
                checked(
                    git(&state.source)
                        .args(["apply", "--binary", "--whitespace=nowarn"])
                        .arg(&patch),
                )?;
            }
            Err(error) if check(true).is_err() => {
                bail!("Could not apply the worktree changes. Both copies were preserved. {error}");
            }
            Err(_) => {} // The complete patch is already present.
        }
        Ok(())
    })?;
    // Keep the snapshot reachable across Git GC for subsequent imports.
    checked(
        git(&worktree)
            .arg("update-ref")
            .arg(format!(
                "refs/kai/worktrees/{:x}/imported",
                Sha256::digest(session.as_bytes())
            ))
            .arg(&current),
    )
    .context("Changes reached the project, but the worktree sync marker could not be saved")?;
    state.imported = Some(current);
    write_state(&directory, &state)
        .context("Changes reached the project, but the worktree sync marker could not be saved")?;
    Ok(())
}

#[cfg(test)]
fn apply(
    storage: &Path,
    session: &str,
    repository: &crate::checkpoints::Repository,
) -> Result<bool> {
    let Some(import) = prepare_import(storage, session, repository)? else {
        return Ok(false);
    };
    apply_import(import, session, repository)?;
    Ok(true)
}

use super::{Orbit, SessionSummary};
use gpui::prelude::*;
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    Window, div, px, rgb, rgba,
};

impl Orbit {
    pub(super) fn parallel_workspace_storage(&self) -> PathBuf {
        self.background_data.join("parallel-worktrees")
    }

    pub(super) fn has_parallel_workspace(&self, session: &SessionSummary) -> bool {
        session
            .execution_path
            .as_ref()
            .is_some_and(|path| owns(&self.parallel_workspace_storage(), &session.id, path))
    }

    pub(super) fn parallel_workspace_controls(&self, cx: &Context<Self>) -> impl IntoElement {
        let session = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|session| &session.id == id));
        let draft = self.active.is_none() && self.project != self.unprojected_root();
        let managed = session.is_some_and(|session| self.has_parallel_workspace(session));
        let busy = self.checkpoint_busy > 0
            || self.starting_current()
            || session.is_some_and(|session| {
                self.runs.contains_key(&session.id)
                    || self.settling.contains_key(&session.id)
                    || self.finalizing.contains(&session.id)
            });
        div().when(draft || managed, |s| {
            s.mx_3()
                .my_2()
                .pt_2()
                .border_t_1()
                .border_color(rgba(0xffffff18))
                .text_size(crate::theme::ui_px(11.))
                .text_color(rgb(super::MUTED))
                .child(if managed {
                    "This chat uses its own Git worktree."
                } else if self.parallel_draft {
                    "New chat will start from the current Git commit."
                } else {
                    "New chat uses the current checkout."
                })
                .child(
                    div()
                        .id("parallel-workspace-action")
                        .mt_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .text_color(rgb(super::TEXT))
                        .child(if managed {
                            "Apply worktree changes to project"
                        } else if self.parallel_draft {
                            "Use current checkout"
                        } else {
                            "Use a separate worktree"
                        })
                        .when(!busy, |button| {
                            button
                                .cursor_pointer()
                                .hover(|s| s.bg(rgba(super::GLASS_HOVER)))
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    if managed {
                                        app.apply_parallel_workspace(window, cx);
                                    } else if app.active.is_none() {
                                        app.parallel_draft = !app.parallel_draft;
                                        cx.notify();
                                    }
                                }))
                        }),
                )
        })
    }

    fn apply_parallel_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self
            .active
            .as_ref()
            .and_then(|id| self.sessions.iter().find(|session| &session.id == id))
            .cloned()
        else {
            return;
        };
        if self.quit_pending
            || self.checkpoint_busy > 0
            || self.starting_current()
            || self.runs.contains_key(&session.id)
            || self.settling.contains_key(&session.id)
            || self.finalizing.contains(&session.id)
            || !self.has_parallel_workspace(&session)
        {
            return;
        }
        let Ok(repository) = self.checkpoint_repository.clone() else {
            self.notice = Some("Workspace protection is unavailable. No files changed.".into());
            cx.notify();
            return;
        };
        let storage = self.parallel_workspace_storage();
        self.dismiss_panel(window, cx);
        self.checkpoint_busy += 1;
        self.notice = Some("Preparing worktree review…".into());
        let id = session.id;
        let repo = repository.clone();
        let owner = id.clone();
        let preparation = cx
            .background_executor()
            .spawn(async move { prepare_import(&storage, &owner, &repo) });
        cx.spawn_in(window, async move |this, cx| {
            let result = match preparation.await {
                Ok(Some(import)) => {
                    let detail = format!(
                        "{}\n{} · {}\n\n{}",
                        import.state.source.display(),
                        import.destination.0,
                        &import.destination.1[..12],
                        import.summary
                    );
                    let decision = this
                        .update_in(cx, |app, window, cx| {
                            (!app.quit_pending && app.active.as_deref() == Some(&id)).then(|| {
                                window.prompt(
                                    gpui::PromptLevel::Warning,
                                    "Apply worktree changes?",
                                    Some(&detail),
                                    &["Apply changes", "Cancel"],
                                    cx,
                                )
                            })
                        })
                        .ok()
                        .flatten();
                    if let Some(decision) = decision {
                        if decision.await.ok() == Some(0) {
                            let owner = id.clone();
                            cx.background_executor()
                                .spawn(async move { apply_import(import, &owner, &repository) })
                                .await
                                .map(|_| "Worktree changes applied to the project.")
                        } else {
                            Ok("Worktree changes kept in their checkout.")
                        }
                    } else {
                        Ok("Worktree changes kept in their checkout.")
                    }
                }
                Ok(None) => Ok("The worktree has no new changes to apply."),
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |app, cx| {
                app.checkpoint_busy = app.checkpoint_busy.saturating_sub(1);
                if app.active.as_deref() == Some(&id) {
                    app.notice = Some(
                        result
                            .map(str::to_owned)
                            .unwrap_or_else(|error| format!("{error:#}")),
                    );
                }
                app.github_after_agent_turn(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("kai-parallel-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parallel_checkouts_keep_both_edits_and_import_without_overwriting_conflicts() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source");
        let storage = fixture.0.join("worktrees");
        fs::create_dir_all(source.join("package")).unwrap();
        checked(git(&source).arg("init")).unwrap();
        fs::write(source.join("package/shared.txt"), "baseline\n").unwrap();
        fs::write(source.join(".gitignore"), "ignored\n").unwrap();
        checked(git(&source).args(["add", "."])).unwrap();
        checked(git(&source).args([
            "-c",
            "user.name=Kai Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "Initial snapshot",
        ]))
        .unwrap();
        let repository =
            crate::checkpoints::Repository::new(fixture.0.join("checkpoints")).unwrap();
        let cancel = AtomicBool::new(false);
        let local = repository
            .capture_before(&source, "local", "one", "Local edit", &cancel)
            .unwrap();
        assert!(
            repository
                .capture_before(
                    &source.join("package"),
                    "parallel",
                    "two",
                    "Parallel edit",
                    &cancel
                )
                .unwrap_err()
                .is::<crate::checkpoints::WorkspaceOccupied>()
        );

        let execution = prepare(&storage, "parallel", &source.join("package")).unwrap();
        assert!(owns(
            &storage.canonicalize().unwrap(),
            "parallel",
            &execution
        ));
        assert_eq!(
            prepare(&storage, "parallel", &source.join("package")).unwrap(),
            execution
        );
        let isolated = repository
            .capture_before(&execution, "parallel", "two", "Parallel edit", &cancel)
            .unwrap();
        fs::write(source.join("local.txt"), "local only\n").unwrap();
        checked(git(&source).args(["add", "local.txt"])).unwrap();
        fs::write(execution.join("parallel.bin"), [0, 1, 2, 0, 4]).unwrap();
        fs::write(execution.join("shared.txt"), "from worktree\n").unwrap();
        fs::write(
            execution.parent().unwrap().join("ignored"),
            "do not import\n",
        )
        .unwrap();
        assert!(
            apply(&storage, "parallel", &repository)
                .unwrap_err()
                .is::<crate::checkpoints::WorkspaceOccupied>()
        );
        repository.seal(&isolated, &cancel).unwrap();
        assert!(
            apply(&storage, "parallel", &repository)
                .unwrap_err()
                .is::<crate::checkpoints::WorkspaceOccupied>()
        );
        repository.seal(&local, &cancel).unwrap();

        fs::write(source.join("package/shared.txt"), "local conflict\n").unwrap();
        assert!(
            apply(&storage, "parallel", &repository)
                .unwrap_err()
                .to_string()
                .contains("Both copies were preserved")
        );
        assert!(!source.join("package/parallel.bin").exists());
        assert_eq!(
            fs::read_to_string(source.join("package/shared.txt")).unwrap(),
            "local conflict\n"
        );
        assert_eq!(
            fs::read_to_string(execution.join("shared.txt")).unwrap(),
            "from worktree\n"
        );
        fs::write(source.join("package/shared.txt"), "baseline\n").unwrap();
        let import = prepare_import(&storage, "parallel", &repository)
            .unwrap()
            .unwrap();
        let branch = import.destination.0.clone();
        checked(git(&source).args(["switch", "-c", "changed-during-review"])).unwrap();
        assert!(
            apply_import(import, "parallel", &repository)
                .unwrap_err()
                .to_string()
                .contains("destination branch")
        );
        assert!(!source.join("package/parallel.bin").exists());
        checked(git(&source).args(["switch", &branch])).unwrap();
        assert!(apply(&storage, "parallel", &repository).unwrap());
        assert_eq!(
            fs::read(source.join("package/parallel.bin")).unwrap(),
            [0, 1, 2, 0, 4]
        );
        assert_eq!(
            fs::read_to_string(source.join("local.txt")).unwrap(),
            "local only\n"
        );
        assert!(!source.join("ignored").exists());
        assert_eq!(
            checked(git(&source).args(["diff", "--cached", "--name-only"])).unwrap(),
            b"local.txt\n"
        );
        assert!(!apply(&storage, "parallel", &repository).unwrap());
        fs::write(execution.join("parallel.bin"), [0, 3, 0]).unwrap();
        assert!(apply(&storage, "parallel", &repository).unwrap());
        assert_eq!(
            fs::read(source.join("package/parallel.bin")).unwrap(),
            [0, 3, 0]
        );
    }
}

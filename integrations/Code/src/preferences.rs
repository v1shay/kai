//! Orbit-only project/chat visibility. Agent settings and source sessions are untouched.
use crate::discovery::Project;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::mpsc,
};

/// Code-owned presentation metadata: never rename directories or native sessions.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Lifecycle {
    pub project_names: HashMap<PathBuf, String>,
    pub chat_names: HashMap<String, String>,
    pub pinned_projects: HashSet<PathBuf>,
    pub pinned_chats: HashSet<String>,
    pub archived_chats: HashSet<String>,
}
impl Lifecycle {
    pub fn normalized(mut self) -> Self {
        self.project_names
            .retain(|path, name| path.is_absolute() && valid_name(name));
        self.chat_names
            .retain(|id, name| valid_id(id) && valid_name(name));
        self.pinned_projects.retain(|path| path.is_absolute());
        self.pinned_chats.retain(|id| valid_id(id));
        self.archived_chats.retain(|id| valid_id(id));
        self
    }
}
pub fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
}
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)
}
#[derive(Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    hidden: HashSet<PathBuf>,
    #[serde(default)]
    custom: Vec<SavedProject>,
    #[serde(default)]
    hidden_sessions: HashSet<String>,
    #[serde(default)]
    lifecycle: Lifecycle,
}
#[derive(Serialize, Deserialize)]
struct SavedProject {
    name: String,
    path: PathBuf,
}
pub enum Event {
    Loaded(
        HashSet<PathBuf>,
        Vec<Project>,
        HashSet<String>,
        Box<Lifecycle>,
    ),
    Error(String),
}
enum Command {
    Remove(PathBuf),
    Add(Project),
    HideSession(String),
    RestoreSession(String),
    Lifecycle(Lifecycle),
    Flush(mpsc::Sender<Result<(), String>>),
}
pub struct Preferences {
    tx: mpsc::Sender<Command>,
    pub events: async_channel::Receiver<Event>,
}
impl Preferences {
    pub fn new(path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let (events_tx, events) = async_channel::unbounded();
        std::thread::Builder::new()
            .name("orbit-preferences".into())
            .spawn(move || {
                let (mut settings, mut unsaved_error) = match read(&path) {
                    Ok(settings) => (settings, None),
                    Err(error) => {
                        let error = format!("Could not load project and chat settings: {error}");
                        let _ = events_tx.send_blocking(Event::Error(error.clone()));
                        (Settings::default(), Some(error))
                    }
                };
                let _ = events_tx.send_blocking(Event::Loaded(
                    settings.hidden.clone(),
                    settings
                        .custom
                        .iter()
                        .map(|p| Project {
                            name: p.name.clone(),
                            path: p.path.clone(),
                        })
                        .collect(),
                    settings.hidden_sessions.clone(),
                    Box::new(settings.lifecycle.clone().normalized()),
                ));
                let mut dirty = false;
                while let Ok(command) = rx.recv() {
                    match command {
                        Command::Remove(path) => {
                            settings.custom.retain(|p| p.path != path);
                            settings.hidden.insert(path);
                        }
                        Command::Add(project) => {
                            settings.hidden.remove(&project.path);
                            settings.custom.retain(|p| p.path != project.path);
                            settings.custom.push(SavedProject {
                                name: project.name,
                                path: project.path,
                            });
                        }
                        Command::HideSession(id) => {
                            settings.hidden_sessions.insert(id);
                        }
                        Command::RestoreSession(id) => {
                            settings.hidden_sessions.remove(&id);
                        }
                        Command::Lifecycle(metadata) => settings.lifecycle = metadata.normalized(),
                        Command::Flush(done) => {
                            // A second Quit after disk recovery retries the full
                            // dirty snapshot; read failures alone never overwrite it.
                            if dirty {
                                match write(&path, &settings) {
                                    Ok(()) => {
                                        dirty = false;
                                        unsaved_error = None;
                                    }
                                    Err(error) => {
                                        unsaved_error = Some(format!(
                                            "Could not save project and chat settings: {error}"
                                        ))
                                    }
                                }
                            }
                            let _ = done.send(match &unsaved_error {
                                Some(error) => Err(error.clone()),
                                None => Ok(()),
                            });
                            continue;
                        }
                    }
                    dirty = true;
                    match write(&path, &settings) {
                        Ok(()) => {
                            dirty = false;
                            unsaved_error = None;
                        }
                        Err(error) => {
                            let error =
                                format!("Could not save project and chat settings: {error}");
                            unsaved_error = Some(error.clone());
                            let _ = events_tx.send_blocking(Event::Error(error));
                        }
                    }
                }
            })
            .expect("start project preferences worker");
        Self { tx, events }
    }
    /// FIFO acknowledgement after earlier writes, including any unsaved error.
    pub fn flush(&self) -> mpsc::Receiver<Result<(), String>> {
        let (tx, rx) = mpsc::channel();
        let _ = self.tx.send(Command::Flush(tx));
        rx
    }
    pub fn save_lifecycle(&self, lifecycle: Lifecycle) {
        let _ = self.tx.send(Command::Lifecycle(lifecycle));
    }
    pub fn remove(&self, path: PathBuf) {
        let _ = self.tx.send(Command::Remove(path));
    }
    pub fn add(&self, project: Project) {
        let _ = self.tx.send(Command::Add(project));
    }
    pub fn hide_session(&self, id: String) {
        let _ = self.tx.send(Command::HideSession(id));
    }
    pub fn restore_session(&self, id: String) {
        let _ = self.tx.send(Command::RestoreSession(id));
    }
}
fn read(path: &std::path::Path) -> anyhow::Result<Settings> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(error.into()),
    }
}
fn write(path: &std::path::Path, settings: &Settings) -> anyhow::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.next");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(&serde_json::to_vec(settings)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn flush(preferences: &Preferences) {
        preferences
            .flush()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
    }
    #[test]
    fn chat_removal_and_undo_survive_worker_restarts_without_touching_sources() {
        let root =
            std::env::temp_dir().join(format!("orbit-chat-visibility-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("preferences.json");
        let source = root.join("native-session.jsonl");
        std::fs::write(&source, "original history").unwrap();
        // Existing preference files migrate without discarding project visibility.
        std::fs::write(&path, r#"{"hidden":["/keep-hidden"],"custom":[]}"#).unwrap();
        let preferences = Preferences::new(path.clone());
        preferences.hide_session("native-fixture-id".into());
        flush(&preferences);
        drop(preferences);
        let preferences = Preferences::new(path.clone());
        let Event::Loaded(projects, _, chats, _) = preferences.events.recv_blocking().unwrap()
        else {
            panic!("load preferences")
        };
        assert!(projects.contains(&PathBuf::from("/keep-hidden")));
        assert!(chats.contains("native-fixture-id"));
        preferences.restore_session("native-fixture-id".into());
        flush(&preferences);
        drop(preferences);
        let preferences = Preferences::new(path);
        let Event::Loaded(projects, _, chats, _) = preferences.events.recv_blocking().unwrap()
        else {
            panic!("reload preferences")
        };
        assert!(projects.contains(&PathBuf::from("/keep-hidden")));
        assert!(chats.is_empty());
        assert_eq!(std::fs::read_to_string(source).unwrap(), "original history");
        drop(preferences);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn lifecycle_metadata_survives_restart_without_renaming_agent_sources() {
        let root = std::env::temp_dir().join(format!("code-lifecycle-{}", uuid::Uuid::new_v4()));
        let project = root.join("original-folder");
        std::fs::create_dir_all(&project).unwrap();
        let source = project.join("session.jsonl");
        std::fs::write(&source, "native history").unwrap();
        let path = root.join("preferences.json");
        let preferences = Preferences::new(path.clone());
        let mut lifecycle = Lifecycle::default();
        lifecycle
            .project_names
            .insert(project.clone(), "Friendly name".into());
        lifecycle
            .chat_names
            .insert("native-chat".into(), "Reviewed result".into());
        lifecycle.pinned_projects.insert(project.clone());
        lifecycle.pinned_chats.insert("native-chat".into());
        lifecycle.archived_chats.insert("native-chat".into());
        preferences.save_lifecycle(lifecycle);
        flush(&preferences);
        drop(preferences);
        let preferences = Preferences::new(path);
        let Event::Loaded(_, _, _, lifecycle) = preferences.events.recv_blocking().unwrap() else {
            panic!("load metadata");
        };
        assert_eq!(lifecycle.project_names[&project], "Friendly name");
        assert_eq!(lifecycle.chat_names["native-chat"], "Reviewed result");
        assert!(lifecycle.pinned_chats.contains("native-chat"));
        assert!(lifecycle.pinned_projects.contains(&project));
        assert!(lifecycle.archived_chats.contains("native-chat"));
        assert_eq!(std::fs::read_to_string(source).unwrap(), "native history");
        assert!(!root.join("Friendly name").exists());
        drop(preferences);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn preference_flush_reports_unsaved_errors_until_a_successful_retry() {
        let root =
            std::env::temp_dir().join(format!("code-preference-failure-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("preferences.json");
        let preferences = Preferences::new(path.clone());
        assert!(matches!(
            preferences.events.recv_blocking().unwrap(),
            Event::Loaded(..)
        ));
        std::fs::create_dir(&path).unwrap();
        let mut lifecycle = Lifecycle::default();
        lifecycle
            .chat_names
            .insert("keep".into(), "Preserve this name".into());
        preferences.save_lifecycle(lifecycle.clone());
        assert!(
            preferences
                .flush()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .is_err()
        );
        assert!(
            preferences
                .flush()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .is_err()
        );
        std::fs::remove_dir(&path).unwrap();
        // No second edit is required: retrying Quit/flush saves the dirty data.
        flush(&preferences);
        assert_eq!(
            read(&path).unwrap().lifecycle.chat_names["keep"],
            "Preserve this name"
        );
        drop(preferences);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_lifecycle_names_cannot_break_sidebar_layout_or_roundtrip() {
        let mut lifecycle = Lifecycle::default();
        for (id, name) in [
            ("blank", " "),
            ("control", "line\nbreak"),
            ("valid", "Crabs 🦀"),
        ] {
            lifecycle.chat_names.insert(id.into(), name.into());
        }
        lifecycle
            .project_names
            .insert(PathBuf::from("relative"), "Invalid path".into());
        let lifecycle = lifecycle.normalized();
        assert_eq!(lifecycle.chat_names.len(), 1);
        assert_eq!(lifecycle.chat_names["valid"], "Crabs 🦀");
        assert!(lifecycle.project_names.is_empty());
    }
    #[test]
    fn project_removal_persists_and_readding_restores_without_deleting_files() {
        let root = std::env::temp_dir().join(format!("orbit-projects-{}", uuid::Uuid::new_v4()));
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("keep.txt"), "keep").unwrap();
        let path = root.join("preferences.json");
        let mut settings = Settings::default();
        settings.hidden.insert(project.clone());
        write(&path, &settings).unwrap();
        assert!(read(&path).unwrap().hidden.contains(&project));
        assert_eq!(
            std::fs::read_to_string(project.join("keep.txt")).unwrap(),
            "keep"
        );
        settings.hidden.remove(&project);
        settings.custom.push(SavedProject {
            name: "Restored".into(),
            path: project.clone(),
        });
        write(&path, &settings).unwrap();
        let loaded = read(&path).unwrap();
        assert!(loaded.hidden.is_empty());
        assert_eq!(loaded.custom[0].path, project);
        std::fs::remove_dir_all(root).unwrap();
    }
}

//! A single background actor owns SQLite, discovery, and Markdown work.
#[path = "runtime/opencode_history.rs"]
mod opencode_history;
#[cfg(test)]
use crate::store::StoredMessage;
use crate::{
    discovery::{self, Discovery, SessionSummary},
    presentation::{self, MessageBlock},
    runtime,
    store::Store,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
};

pub struct ParsedMessage {
    pub role: String,
    pub blocks: Vec<MessageBlock>,
}
/// Summary-only archive fetches are explicitly paged by the visible list.
pub const ARCHIVED_PAGE_SIZE: usize = crate::store::SESSION_ID_PAGE_SIZE;
pub enum Event {
    Model(
        String,
        u64,
        Option<String>,
        Option<Box<crate::usage::Usage>>,
        Option<crate::handoff::Packet>,
    ),
    Handoff(uuid::Uuid, Result<crate::handoff::Packet, String>),
    AgentSwitched(
        uuid::Uuid,
        Result<(SessionSummary, Arc<crate::handoff::Packet>), String>,
    ),
    Ready(Discovery, Vec<SessionSummary>, Option<PathBuf>),
    Loaded(String, u64, Vec<ParsedMessage>, bool, bool),
    /// Bounded chronological window at an approximate source-space position.
    HistorySeeked(String, u64, Vec<ParsedMessage>, bool, bool, f64, (f64, f64)),
    HistoryRange(String, u64, f64, f64),
    OlderLoaded(String, u64, Vec<ParsedMessage>, bool),
    HistoryPage(String, u64, usize, Vec<ParsedMessage>, bool),
    ViewportRestored(String, u64, usize, Vec<ParsedMessage>, bool, bool),
    Search(u64, Vec<SessionSummary>),
    ContextSearch(u64, Vec<SessionSummary>),
    Metadata(Vec<SessionSummary>),
    MetadataError(String),
    Parsed(String, Vec<MessageBlock>),
    PagedFinished(String),
    FinishError(String, String),
    ProjectSessions(PathBuf, u64, Vec<SessionSummary>),
    SidebarProject(PathBuf, u64, Vec<SessionSummary>),
    SidebarProjectError(PathBuf, u64, String),
    ArchivedLoaded(u64, Vec<SessionSummary>),
    ArchivedError(u64, String),
    LoadError(String, u64, String),
    ProjectError(PathBuf, u64, String),
    Error(String),
}
enum Command {
    PauseLiveHistory(String,u64,uuid::Uuid),
    ResumeLiveHistory(String,u64,u64,uuid::Uuid),
    LoadModel(String, u64),
    SetModel(SessionSummary, Option<String>),
    SaveUsage(String, crate::usage::Usage),
    PrepareHandoff(SessionSummary, uuid::Uuid, bool),
    #[cfg(test)]
    BindHandoff(SessionSummary, Arc<crate::handoff::Packet>),
    SwitchAgent(
        SessionSummary,
        Arc<crate::handoff::Packet>,
        Option<String>,
        uuid::Uuid,
    ),
    Load(SessionSummary, u64, usize, usize),
    SeekHistory(SessionSummary, u64, f64),
    Older(SessionSummary, u64, usize),
    HistoryPage(SessionSummary, u64, usize),
    SaveViewport(String, u64, usize, bool),
    RestoreViewport(SessionSummary, u64, usize),
    Flush(mpsc::Sender<()>),
    Search(String, u64),
    ContextSearch(String, u64),
    Metadata(Vec<String>),
    Save(SessionSummary, String, String),
    SavePrompt(SessionSummary, String, Option<crate::store::NativeInput>),
    Finish(SessionSummary, String),
    FinishSource(SessionSummary, crate::response_buffer::ResponseSource),
    UpdateSession(SessionSummary),
    Refresh,
    LoadProject(PathBuf, u64),
    LoadSidebarProject(PathBuf, u64),
    LoadArchived(Vec<String>, u64),
}
enum RequestScope {
    History(String, u64),
    Project(PathBuf, u64),
    SidebarProject(PathBuf, u64),
    Archived(u64),
    Finish(String),
    General,
}
#[derive(Default)]
struct LatestReads {
    history: AtomicU64,
    search: AtomicU64,
    context_search: AtomicU64,
    project: AtomicU64,
    archived: AtomicU64,
}
impl LatestReads {
    fn stale(&self, command: &Command) -> bool {
        match command {
            Command::LoadModel(_, generation)
            | Command::ResumeLiveHistory(_,_,generation,_)
            | Command::Load(_, generation, _, _)
            | Command::SeekHistory(_, generation, _)
            | Command::Older(_, generation, _)
            | Command::HistoryPage(_, generation, _)
            | Command::RestoreViewport(_, generation, _) => {
                *generation != self.history.load(Ordering::Acquire)
            }
            Command::Search(_, generation) => *generation != self.search.load(Ordering::Acquire),
            Command::ContextSearch(_, generation) => {
                *generation != self.context_search.load(Ordering::Acquire)
            }
            Command::LoadProject(_, generation) => {
                *generation != self.project.load(Ordering::Acquire)
            }
            Command::LoadArchived(_, generation) => {
                *generation != self.archived.load(Ordering::Acquire)
            }
            _ => false,
        }
    }
}
pub struct Engine {
    tx: mpsc::Sender<Command>,
    latest: Arc<LatestReads>,
    pub events: async_channel::Receiver<Event>,
}
impl Engine {
    pub fn new(home: PathBuf, root: PathBuf, database: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        let (events_tx, events) = async_channel::bounded(32);
        let latest = Arc::new(LatestReads::default());
        let worker_latest = latest.clone();
        std::thread::Builder::new()
            .name("orbit-state".into())
            .spawn(move || {
                let mut store = match Store::open(&database) {
                    Ok(store) => store,
                    Err(error) => {
                        let _ = events_tx
                            .send_blocking(Event::Error(format!("History unavailable: {error}")));
                        return;
                    }
                };
                let refresh = |store: &mut Store| -> anyhow::Result<Event> {
                    let found = discovery::discover(&home, &root);
                    store.upsert_discovered_sessions(&found.sessions)?;
                    let sessions = store.sessions()?;
                    let mut checked = std::collections::HashSet::new();
                    let preferred = sessions
                        .iter()
                        .map(|session| &session.project_path)
                        .find(|path| checked.insert((*path).clone()) && path.is_dir())
                        .cloned()
                        .or_else(|| {
                            found
                                .projects
                                .iter()
                                .find(|project| project.path != std::path::Path::new("/"))
                                .map(|project| project.path.clone())
                        });
                    Ok(Event::Ready(found, sessions, preferred))
                };
                match refresh(&mut store) {
                    Ok(event) => {
                        let _ = events_tx.send_blocking(event);
                    }
                    Err(error) => {
                        let _ = events_tx.send_blocking(Event::Error(error.to_string()));
                    }
                }
                let viewport_root = database.parent().unwrap_or(&root).join("viewport");
                let mut histories: VecDeque<(String, HistoryState)> = VecDeque::new();
                let mut paused_live_history:Option<(String,uuid::Uuid,HistoryState)>=None;
                while let Ok(command) = rx.recv() {
                    // A newer interaction supersedes queued reads before any
                    // filesystem scan, SQLite query, parsing, or cache work.
                    // Writes and cursor continuation reads preserve FIFO order.
                    if worker_latest.stale(&command) {
                        continue;
                    }
                    let read_generation = match &command {
                        Command::LoadModel(_, generation)
                        | Command::ResumeLiveHistory(_,_,generation,_)
                        | Command::Load(_, generation, _, _)
                        | Command::SeekHistory(_, generation, _)
                        | Command::Older(_, generation, _)
                        | Command::HistoryPage(_, generation, _)
                        | Command::RestoreViewport(_, generation, _) => {
                            Some((&worker_latest.history, *generation))
                        }
                        Command::Search(_, generation) => {
                            Some((&worker_latest.search, *generation))
                        }
                        Command::ContextSearch(_, generation) => {
                            Some((&worker_latest.context_search, *generation))
                        }
                        Command::LoadProject(_, generation) => {
                            Some((&worker_latest.project, *generation))
                        }
                        Command::LoadArchived(_, generation) => {
                            Some((&worker_latest.archived, *generation))
                        }
                        _ => None,
                    };
                    let scope = match &command {
                        Command::Load(session, generation, _, _)
                        | Command::SeekHistory(session, generation, _)
                        | Command::Older(session, generation, _)
                        | Command::HistoryPage(session, generation, _)
                        | Command::RestoreViewport(session, generation, _) => {
                            RequestScope::History(session.id.clone(), *generation)
                        }
                        Command::LoadProject(path, generation) => {
                            RequestScope::Project(path.clone(), *generation)
                        }
                        Command::LoadSidebarProject(path, generation) => {
                            RequestScope::SidebarProject(path.clone(), *generation)
                        }
                        Command::LoadArchived(_, generation) => RequestScope::Archived(*generation),
                        Command::Finish(session, _) | Command::FinishSource(session, _) => {
                            RequestScope::Finish(session.id.clone())
                        }
                        _ => RequestScope::General,
                    };
                    let result: anyhow::Result<Option<Event>> = (|| match command {
                        Command::PauseLiveHistory(id,generation,epoch)=>{
                            if let Some(position)=histories.iter().position(|(owner,state)|owner==&id&&state.generation==generation){if let Some((_,state))=histories.remove(position){paused_live_history=Some((id,epoch,state));}}Ok(None)
                        }
                        Command::ResumeLiveHistory(id,from,generation,epoch)=>{
                            let state=if paused_live_history.as_ref().is_some_and(|(owner,run,state)|owner==&id&&*run==epoch&&state.generation==from){paused_live_history.take().map(|(_,_,state)|state)}else{histories.iter().position(|(owner,state)|owner==&id&&state.generation==from).and_then(|position|histories.remove(position)).map(|(_,state)|state)};
                            if let Some(mut state)=state{state.generation=generation;histories.retain(|(owner,_)|owner!=&id);if histories.len()>=8{histories.pop_front();}histories.push_back((id,state));}Ok(None)
                        }
                        Command::Flush(done) => {
                            let _ = done.send(());
                            Ok(None)
                        }
                        Command::SaveViewport(id, generation, page, at_bottom) => {
                            let mut persisted = false;
                            if let Some((_, state)) =
                                histories.iter_mut().find(|(current, state)| {
                                    current == &id && state.generation == generation
                                })
                            {
                                persisted = state
                                    .persist_viewport(&viewport_root, &id, page, &store)
                                    .is_ok();
                            }
                            if !persisted
                                && at_bottom
                                && page == 0
                                && let Some(session) = store.session(&id)?
                            {
                                // A newly completed turn invalidates the old cursor.
                                // Persist a fresh bounded tail only for an explicitly
                                // bottom-anchored viewport, never substitute a deep
                                // manually scrolled page or replace live UI cursors.
                                let mut fresh = HistoryState::new(&session, generation, &store)?;
                                let messages = fresh.next(&store, &session, 200)?;
                                fresh.cache(&messages, fresh.has_older())?;
                                let _ = fresh.persist_viewport(&viewport_root, &id, 0, &store);
                            }
                            Ok(None)
                        }
                        Command::RestoreViewport(session, generation, page) => {
                            let session = resolve_history_session(&home, &store, session)?;
                            let (state, messages, has_older) = HistoryState::restore_viewport(
                                &viewport_root,
                                &session,
                                generation,
                                page,
                                &store,
                            )?;
                            histories.retain(|(id, _)| id != &session.id);
                            if histories.len() >= 8 {
                                histories.pop_front();
                            }
                            let has_newer = page > 0 || state.represented_range.1 < 1.;
                            histories.push_back((session.id.clone(), state));
                            Ok(Some(Event::ViewportRestored(
                                session.id,
                                generation,
                                page,
                                parse_messages(messages),
                                has_older,
                                has_newer,
                            )))
                        }
                        Command::LoadModel(id, generation) => {
                            let mut usage=store.session_usage(&id)?;
                            if let Some(session)=store.session(&id)?{
                                if let Some(native)=session.transcript_path.as_deref().and_then(|p|crate::usage::native_chat_settings(p,session.agent)){
                                    usage.get_or_insert_with(Default::default).merge(native);
                                }
                            }
                            Ok(Some(Event::Model(id.clone(),generation,store.session_model(&id)?,usage.map(Box::new),store.session_handoff(&id)?)))
                        },
                        Command::PrepareHandoff(session, token, interrupted) => {
                            let result = (|| {
                                let session = store.session(&session.id)?.unwrap_or(session);
                                let parent = store.session_handoff(&session.id)?;
                                let objective = store.first_saved_request(&session.id)?;
                                let cached = histories
                                    .iter_mut()
                                    .find(|(id, state)| {
                                        id == &session.id
                                            && state.represented_range.1 >= 1.
                                            && store
                                                .viewport_head(id)
                                                .is_ok_and(|head| head == state.local_head)
                                            && state
                                                .native
                                                .as_ref()
                                                .is_none_or(NativeHistory::source_matches)
                                    })
                                    .and_then(|(_, state)| state.cached(0, &store, &session).ok());
                                let (messages, has_older) = if let Some(cached) = cached {
                                    cached
                                } else {
                                    let mut state = HistoryState::new(&session, 0, &store)?;
                                    let messages = state.next(&store, &session, 200)?;
                                    (messages, state.has_older())
                                };
                                let packet = crate::handoff::Packet::build(
                                    &session,
                                    parent.as_ref(),
                                    objective.as_deref(),
                                    messages
                                        .into_iter()
                                        .map(|m| crate::handoff::ContextMessage {
                                            role: m.role,
                                            text: m.text,
                                        })
                                        .collect(),
                                    has_older,
                                    interrupted,
                                );
                                anyhow::ensure!(
                                    packet.valid()
                                        && serde_json::to_vec(&packet)?.len()
                                            <= crate::handoff::MAX_PACKET_BYTES,
                                    "Handoff context exceeded its metadata budget"
                                );
                                Ok::<_, anyhow::Error>(packet)
                            })()
                            .map_err(|error| error.to_string());
                            Ok(Some(Event::Handoff(token, result)))
                        }
                        Command::SwitchAgent(session, packet, model, token) => {
                            let result = store
                                .switch_agent(&session, &packet, model.as_deref())
                                .map(|()| (session, packet))
                                .map_err(|error| error.to_string());
                            // Acknowledgement follows the transaction, before
                            // the UI launches a destination process.
                            Ok(Some(Event::AgentSwitched(token, result)))
                        }
                        #[cfg(test)]
                        Command::BindHandoff(session, packet) => {
                            store.upsert_session(&session)?;
                            store.set_session_handoff(&session.id, &packet)?;
                            Ok(None)
                        }
                        Command::SaveUsage(id, usage) => {
                            store.set_session_usage(&id, &usage)?;
                            Ok(None)
                        }
                        Command::SetModel(session, model) => {
                            store.upsert_session(&session)?;
                            store.set_session_model(&session.id, model.as_deref())?;
                            Ok(None)
                        }
                        Command::Refresh => refresh(&mut store).map(Some),
                        Command::LoadProject(path, generation) => {
                            store.upsert_discovered_sessions(&discovery::discover_project(
                                &home, &path,
                            ))?;
                            Ok(Some(Event::ProjectSessions(
                                path.clone(),
                                generation,
                                store.sessions_for_project(&path)?,
                            )))
                        }
                        Command::LoadSidebarProject(path, generation) => {
                            store.upsert_discovered_sessions(&discovery::discover_project(
                                &home, &path,
                            ))?;
                            Ok(Some(Event::SidebarProject(
                                path.clone(),
                                generation,
                                store.sessions_for_project(&path)?,
                            )))
                        }
                        Command::Search(query, generation) => {
                            Ok(Some(Event::Search(generation, store.search(&query)?)))
                        }
                        Command::ContextSearch(query, generation) => Ok(Some(
                            Event::ContextSearch(generation, store.search(&query)?),
                        )),
                        Command::Metadata(ids) => Ok(Some(match store.sessions_by_id(&ids) {
                            Ok(sessions) => Event::Metadata(sessions),
                            Err(error) => Event::MetadataError(error.to_string()),
                        })),
                        Command::LoadArchived(ids, generation) => Ok(Some(Event::ArchivedLoaded(
                            generation,
                            store.sessions_by_id(&ids)?,
                        ))),
                        Command::UpdateSession(mut session) => {
                            // A fork's UI snapshot still contains the parent's
                            // rollout, including repeated SessionId events.
                            if session.agent == discovery::AgentKind::Codex
                                && session.id.starts_with("codex:")
                                && session
                                    .external_id
                                    .as_ref()
                                    .is_some_and(|id| session.id != format!("codex:{id}"))
                            {
                                session.transcript_path = None;
                            }
                            refresh_codex_transcript(&home, &mut session);
                            store.bind_runtime_session(&session)?;
                            Ok(None)
                        }
                        Command::SeekHistory(session, generation, fraction) => {
                            let session = resolve_history_session(&home, &store, session)?;
                            let cancelled =
                                || worker_latest.history.load(Ordering::Acquire) != generation;
                            let baseline = store.conversation_baseline(&session.id)?;
                            let source = baseline.as_ref().map_or(&session, |(source, _)| source);
                            let reusable = histories
                                .iter()
                                .find(|(id, state)| {
                                    id == &session.id
                                        && state.native.as_ref().is_some_and(|native| {
                                            native.source_matches()
                                                && native.matches_session(source)
                                        })
                                })
                                .and_then(|(_, state)| {
                                    let native = state.native.as_ref()?;
                                    let held = state.held_artifacts.clone();
                                    NativeHistory::restore(native.checkpoint(), &mut |path| {
                                        held.get(path).cloned().ok_or_else(|| {
                                            anyhow::anyhow!("History artifact unavailable")
                                        })
                                    })
                                    .ok()
                                });
                            let mut state = HistoryState::new_with_native(
                                &session, generation, &store, cancelled, reusable,
                            )?;
                            let page = state.seek(&store, &session, fraction, 200, &cancelled)?;
                            let has_older = state.has_older();
                            let has_newer = fraction < 1. && state.represented_range.1 < 1.;
                            let range = state.represented_range;
                            state.cache(&page, has_older)?;
                            histories.retain(|(id, _)| id != &session.id);
                            if histories.len() >= 8 {
                                histories.pop_front();
                            }
                            histories.push_back((session.id.clone(), state));
                            Ok(Some(Event::HistorySeeked(
                                session.id,
                                generation,
                                parse_messages(page),
                                has_older,
                                has_newer,
                                fraction,
                                range,
                            )))
                        }
                        Command::Load(session, generation, limit, offset) => {
                            let session = resolve_history_session(&home, &store, session)?;
                            let mut state = HistoryState::new_cancellable(
                                &session,
                                generation,
                                &store,
                                || worker_latest.history.load(Ordering::Acquire) != generation,
                            )?;
                            // Legacy callers may request a fixed earlier window. New
                            // UI navigation uses Older and never rescans from zero.
                            let mut skipped = 0;
                            while skipped < offset {
                                let page = state.next_cancellable(
                                    &store,
                                    &session,
                                    limit.min(offset - skipped),
                                    &|| worker_latest.history.load(Ordering::Acquire) != generation,
                                )?;
                                skipped += page.len();
                                if page.is_empty() {
                                    break;
                                }
                            }
                            let page = state.next_cancellable(&store, &session, limit, &|| {
                                worker_latest.history.load(Ordering::Acquire) != generation
                            })?;
                            let has_older = state.has_older();
                            state.cache(&page, has_older)?;
                            histories.retain(|(id, _)| id != &session.id);
                            if histories.len() >= 8 {
                                histories.pop_front();
                            }
                            histories.push_back((session.id.clone(), state));
                            Ok(Some(Event::Loaded(
                                session.id,
                                generation,
                                parse_messages(page),
                                has_older,
                                offset > 0,
                            )))
                        }
                        Command::Older(session, generation, limit) => {
                            let state = histories
                                .iter_mut()
                                .find(|(id, state)| {
                                    id == &session.id && state.generation == generation
                                })
                                .map(|(_, state)| state)
                                .ok_or_else(|| {
                                    anyhow::anyhow!(
                                        "History page expired; reopen this conversation"
                                    )
                                })?;
                            let page = state.next_cancellable(&store, &session, limit, &|| {
                                worker_latest.history.load(Ordering::Acquire) != generation
                            })?;
                            let has_older = state.has_older();
                            state.cache(&page, has_older)?;
                            Ok(Some(Event::OlderLoaded(
                                session.id,
                                generation,
                                parse_messages(page),
                                has_older,
                            )))
                        }
                        Command::HistoryPage(session, generation, page_number) => {
                            let state = histories
                                .iter_mut()
                                .find(|(id, state)| {
                                    id == &session.id && state.generation == generation
                                })
                                .map(|(_, state)| state)
                                .ok_or_else(|| {
                                    anyhow::anyhow!(
                                        "History page expired; reopen this conversation"
                                    )
                                })?;
                            let (page, has_older) = state.cached(page_number, &store, &session)?;
                            Ok(Some(Event::HistoryPage(
                                session.id,
                                generation,
                                page_number,
                                parse_messages(page),
                                has_older,
                            )))
                        }
                        Command::Save(session, role, text) => {
                            store.upsert_session(&session)?;
                            if let Some(text) = presentation::sanitize_transcript_text(&role, &text)
                            {
                                store.save_message(&session.id, &role, &text)?;
                            }
                            Ok(None)
                        }
                        Command::SavePrompt(session, text, input) => {
                            store.upsert_session(&session)?;
                            if let Some(text) =
                                presentation::sanitize_transcript_text("user", &text)
                            {
                                store.save_prompt(&session.id, &text, input.as_ref())?;
                            }
                            Ok(None)
                        }
                        Command::Finish(mut session, text) => {
                            // Resolve the durable binding before accepting a queued
                            // snapshot, then retry metadata created at turn completion.
                            if let Some(saved) = store.session(&session.id)? {
                                session.external_id = saved.external_id;
                                session.transcript_path = saved.transcript_path;
                            }
                            refresh_codex_transcript(&home, &mut session);
                            store.upsert_session(&session)?;
                            let text = presentation::sanitize_transcript_text("assistant", &text)
                                .unwrap_or_default();
                            if !text.trim().is_empty() {
                                store.save_message(&session.id, "assistant", &text)?;
                            }
                            Ok(Some(Event::Parsed(session.id, presentation::blocks(&text))))
                        }
                        Command::FinishSource(mut session, source) => {
                            if let Some(saved) = store.session(&session.id)? {
                                session.external_id = saved.external_id;
                                session.transcript_path = saved.transcript_path;
                            }
                            refresh_codex_transcript(&home, &mut session);
                            store.upsert_session(&session)?;
                            store.save_response_source(&session.id, &source)?;
                            // The existing lazy reader supplies the latest bounded
                            // page. Never parse an entire completed response here.
                            Ok(Some(Event::PagedFinished(session.id)))
                        }
                    })();
                    if read_generation.is_some_and(|(latest, generation)| {
                        latest.load(Ordering::Acquire) != generation
                    }) {
                        continue;
                    }
                    match result {
                        Ok(Some(event)) => {
                            let range = match &event {
                                Event::Loaded(id, generation, ..)
                                | Event::OlderLoaded(id, generation, ..)
                                | Event::ViewportRestored(id, generation, ..) => histories
                                    .iter()
                                    .find(|(key, state)| {
                                        key == id && state.generation == *generation
                                    })
                                    .map(|(_, state)| {
                                        Event::HistoryRange(
                                            id.clone(),
                                            *generation,
                                            state.represented_range.0,
                                            state.represented_range.1,
                                        )
                                    }),
                                _ => None,
                            };
                            if events_tx.send_blocking(event).is_err() {
                                break;
                            }
                            if let Some(range) = range
                                && events_tx.send_blocking(range).is_err()
                            {
                                break;
                            }
                        }
                        Err(error) => {
                            let event = match scope {
                                RequestScope::History(id, generation) => {
                                    Event::LoadError(id, generation, error.to_string())
                                }
                                RequestScope::Project(path, generation) => {
                                    Event::ProjectError(path, generation, error.to_string())
                                }
                                RequestScope::SidebarProject(path, generation) => {
                                    Event::SidebarProjectError(path, generation, error.to_string())
                                }
                                RequestScope::Archived(generation) => {
                                    Event::ArchivedError(generation, error.to_string())
                                }
                                RequestScope::Finish(id) => {
                                    Event::FinishError(id, error.to_string())
                                }
                                RequestScope::General => Event::Error(error.to_string()),
                            };
                            if events_tx.send_blocking(event).is_err() {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            })
            .expect("start state worker");
        Self { tx, events, latest }
    }
    pub fn load(&self, session: SessionSummary, generation: u64) {
        self.load_history(session, generation, 200);
    }
    pub fn load_history(&self, session: SessionSummary, generation: u64, limit: usize) {
        self.load_history_window(session, generation, limit, 0);
    }
    pub fn load_history_window(
        &self,
        session: SessionSummary,
        generation: u64,
        limit: usize,
        offset: usize,
    ) {
        self.latest.history.store(generation, Ordering::Release);
        let _ = self.tx.send(Command::Load(
            session,
            generation,
            limit.clamp(1, 400),
            offset,
        ));
    }
    /// Source-space seek: 0 opens the first visible page, 1 the latest page.
    /// Interior positions approximate byte/key locations, not rendered pixels.
    pub fn seek_history(&self, session: SessionSummary, generation: u64, fraction: f64) {
        let fraction = if fraction.is_finite() {
            fraction.clamp(0., 1.)
        } else {
            1.
        };
        self.latest.history.store(generation, Ordering::Release);
        let _ = self
            .tx
            .send(Command::SeekHistory(session, generation, fraction));
    }
    pub fn load_older(&self, session: SessionSummary, generation: u64, limit: usize) {
        let _ = self
            .tx
            .send(Command::Older(session, generation, limit.clamp(1, 400)));
    }
    pub fn load_history_page(&self, session: SessionSummary, generation: u64, page_number: usize) {
        let _ = self
            .tx
            .send(Command::HistoryPage(session, generation, page_number));
    }
    pub fn save_viewport(&self, id: String, generation: u64, page: usize, at_bottom: bool) {
        let _ = self
            .tx
            .send(Command::SaveViewport(id, generation, page, at_bottom));
    }
    pub fn restore_viewport(&self, session: SessionSummary, generation: u64, page: usize) {
        self.latest.history.store(generation, Ordering::Release);
        let _ = self
            .tx
            .send(Command::RestoreViewport(session, generation, page));
    }
    pub fn flush(&self) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel();
        let _ = self.tx.send(Command::Flush(tx));
        rx
    }
    pub fn search(&self, query: String, generation: u64) {
        self.latest.search.store(generation, Ordering::Release);
        let _ = self.tx.send(Command::Search(query, generation));
    }
    pub fn search_context(&self, query: String, generation: u64) {
        self.latest
            .context_search
            .store(generation, Ordering::Release);
        let _ = self.tx.send(Command::ContextSearch(query, generation));
    }
    pub fn load_metadata(&self, ids: Vec<String>) {
        let _ = self.tx.send(Command::Metadata(
            ids.into_iter().take(ARCHIVED_PAGE_SIZE).collect(),
        ));
    }
    pub fn load_model(&self, id: String, generation: u64) {
        self.latest.history.store(generation, Ordering::Release);
        let _ = self.tx.send(Command::LoadModel(id, generation));
    }
    pub fn set_model(&self, session: SessionSummary, model: Option<String>) {
        let _ = self.tx.send(Command::SetModel(session, model));
    }
    pub fn save_usage(&self, id: String, usage: crate::usage::Usage) {
        let _ = self.tx.send(Command::SaveUsage(id, usage));
    }
    pub fn prepare_handoff(&self, session: SessionSummary, token: uuid::Uuid, interrupted: bool) {
        let _ = self
            .tx
            .send(Command::PrepareHandoff(session, token, interrupted));
    }
    pub fn switch_agent(
        &self,
        session: SessionSummary,
        packet: Arc<crate::handoff::Packet>,
        model: Option<String>,
        token: uuid::Uuid,
    ) {
        let _ = self
            .tx
            .send(Command::SwitchAgent(session, packet, model, token));
    }
    #[cfg(test)]
    pub fn bind_handoff(&self, session: SessionSummary, packet: Arc<crate::handoff::Packet>) {
        let _ = self.tx.send(Command::BindHandoff(session, packet));
    }
    pub fn save(&self, session: SessionSummary, role: &str, text: String) {
        let _ = self.tx.send(Command::Save(session, role.into(), text));
    }
    pub fn save_prompt(
        &self,
        session: SessionSummary,
        text: String,
        input: Option<crate::store::NativeInput>,
    ) {
        let _ = self.tx.send(Command::SavePrompt(session, text, input));
    }
    pub fn finish(&self, session: SessionSummary, text: String) {
        let _ = self.tx.send(Command::Finish(session, text));
    }
    pub fn finish_source(
        &self,
        session: SessionSummary,
        source: crate::response_buffer::ResponseSource,
    ) {
        match source {
            crate::response_buffer::ResponseSource::Memory(text) => self.finish(session, text),
            source => {
                let _ = self.tx.send(Command::FinishSource(session, source));
            }
        }
    }
    /// Persist the native CLI's ID as soon as it arrives, without adding a turn.
    pub fn update_session(&self, session: SessionSummary) {
        let _ = self.tx.send(Command::UpdateSession(session));
    }
    pub fn refresh(&self) {
        let _ = self.tx.send(Command::Refresh);
    }
    pub fn load_project(&self, path: PathBuf, generation: u64) {
        self.latest.project.store(generation, Ordering::Release);
        let _ = self.tx.send(Command::LoadProject(path, generation));
    }
    /// Independent summary request for an expanded sidebar branch. The UI
    /// matches tokens per path; opening another branch never supersedes this.
    pub fn load_sidebar_project(&self, path: PathBuf, generation: u64) {
        let _ = self.tx.send(Command::LoadSidebarProject(path, generation));
    }
    pub fn load_archived(&self, ids: Vec<String>, generation: u64) {
        self.latest.archived.store(generation, Ordering::Release);
        let _ = self.tx.send(Command::LoadArchived(ids, generation));
    }
    pub(super) fn pause_live_history(&self,id:String,generation:u64,epoch:uuid::Uuid){let _=self.tx.send(Command::PauseLiveHistory(id,generation,epoch));}
    pub(super) fn resume_live_history(&self,id:String,from:u64,generation:u64,epoch:uuid::Uuid){self.latest.history.store(generation,Ordering::Release);let _=self.tx.send(Command::ResumeLiveHistory(id,from,generation,epoch));}
}

fn refresh_codex_transcript(home: &std::path::Path, session: &mut SessionSummary) {
    if session.agent == discovery::AgentKind::Codex
        && let Some(id) = &session.external_id
        && let Some(path) = discovery::codex_existing_transcript_path(home, id)
    {
        session.transcript_path = Some(path);
    }
}

/// The durable runtime binding wins over queued UI metadata. Resolve repaired
/// native locations before creating a lazy cursor, including after a restart.
fn resolve_history_session(
    home: &Path,
    store: &Store,
    session: SessionSummary,
) -> anyhow::Result<SessionSummary> {
    let mut resolved = store.session(&session.id)?.unwrap_or(session);
    if resolved.agent == discovery::AgentKind::Codex
        && let Some(id) = resolved.external_id.as_deref()
        && let Some(path) =
            discovery::recover_codex_transcript(home, id, resolved.transcript_path.as_deref())
    {
        resolved.transcript_path = Some(path);
    }
    store.upsert_session(&resolved)?;
    if let Some((mut source, _)) = store.conversation_baseline(&resolved.id)?
        && source.agent == discovery::AgentKind::Codex
        && let Some(id) = source.external_id.as_deref()
        && let Some(path) =
            discovery::recover_codex_transcript(home, id, source.transcript_path.as_deref())
        && source.transcript_path.as_ref() != Some(&path)
    {
        source.transcript_path = Some(path);
        store.update_conversation_baseline(&resolved.id, &source)?;
    }
    Ok(resolved)
}

fn parse_messages(messages: Vec<runtime::TranscriptMessage>) -> Vec<ParsedMessage> {
    coherent_history_messages(messages)
        .into_iter()
        .map(|message| {
            let text = if let Some(language) = &message.code_language
                && message.role.split(':').next() != Some("tool") {
                std::borrow::Cow::Owned(if language.starts_with("~~~") {
                    format!("{language}\n{}", message.text)
                } else {
                    format!("```{language}\n{}", message.text)
                })
            } else {
                std::borrow::Cow::Borrowed(message.text.as_str())
            };
            let canonical_role = message
                .role
                .split_once(':')
                .map_or(message.role.as_str(), |(role, _)| role);
            let blocks = presentation::message_blocks(canonical_role, &text);
            ParsedMessage {
                role: message.role,
                blocks,
            }
        })
        .collect()
}

fn coherent_history_messages(messages:Vec<runtime::TranscriptMessage>)->Vec<runtime::TranscriptMessage>{
    let speaker=|m:&runtime::TranscriptMessage|{
        if m.role.split(':').next()!=Some("tool"){return None}
        match crate::chat_interactions::decode_with_context(&m.text)?.0{crate::chat_interactions::ChatInteraction::Speaker{speaker,..}=>Some(speaker),_=>None}
    };
    let mut groups=vec![false];let mut assignments=Vec::with_capacity(messages.len());let mut group=0;
    for m in &messages{
        let role=m.role.split(':').next().unwrap_or("");
        if role=="user"||speaker(m)==Some(crate::chat_interactions::Speaker::User){group+=1;groups.push(false)}
        if matches!(role,"assistant"|"continuation")&&!m.text.trim().is_empty(){groups[group]=true}
        assignments.push(group);
    }
    messages.into_iter().zip(assignments).filter_map(|(m,g)|(!(groups[g]&&speaker(&m)==Some(crate::chat_interactions::Speaker::Assistant))).then_some(m)).collect()
}

#[cfg(test)]mod coherent_chat_tests{
 use super::*;
 #[test]fn existing_history_keeps_one_answer_and_separate_voice_followups(){
  let message=|role:&str,text:String|runtime::TranscriptMessage{role:role.into(),text,code_language:None};
  let voice=|speaker,text:&str|message("tool",crate::chat_interactions::encode(&crate::chat_interactions::ChatInteraction::Speaker{speaker,text:text.into()}));
  use crate::chat_interactions::Speaker;
  let messages=vec![message("user","News".into()),message("assistant","Written headlines".into()),voice(Speaker::Assistant,"Spoken headlines"),voice(Speaker::User,"Thanks"),voice(Speaker::Assistant,"Welcome")];
  let result=coherent_history_messages(messages);assert_eq!(result.len(),4);assert!(result.iter().all(|m|!m.text.contains("Spoken headlines")));assert!(result.iter().any(|m|m.text.contains("Welcome")));
 }
}

enum NativeHistory {
    Jsonl(Box<runtime::history::HistoryCursor>),
    OpenCode(Box<opencode_history::OpenCodeCursor>),
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
enum NativeCheckpoint {
    Jsonl(Box<runtime::history::HistoryCheckpoint>),
    OpenCode(Box<opencode_history::OpenCodeCheckpoint>),
}
impl NativeCheckpoint {
    fn rewrite(&mut self, map: &HashMap<PathBuf, PathBuf>) {
        match self {
            Self::Jsonl(c) => c.rewrite_artifacts(map),
            Self::OpenCode(c) => c.rewrite_artifacts(map),
        }
    }
}
impl NativeHistory {
    fn matches_session(&self, session: &SessionSummary) -> bool {
        match self.checkpoint() {
            NativeCheckpoint::Jsonl(c) => {
                c.agent == session.agent && session.transcript_path.as_ref() == Some(&c.source)
            }
            NativeCheckpoint::OpenCode(c) => c.matches_session(session),
        }
    }
    fn head_end(&self) -> Option<f64> {
        match self {
            Self::Jsonl(c) => c.head_end,
            Self::OpenCode(c) => c.head_end,
        }
    }
    fn seek_fraction(
        &mut self,
        fraction: f64,
        cancelled: &impl Fn() -> bool,
    ) -> anyhow::Result<()> {
        match self {
            Self::Jsonl(c) => c.seek_fraction(fraction, cancelled),
            Self::OpenCode(c) => c.seek_fraction(fraction),
        }
    }
    fn source_fraction(&self) -> anyhow::Result<f64> {
        Ok(match self {
            Self::Jsonl(c) => c.source_fraction(),
            Self::OpenCode(c) => c.source_fraction()?,
        })
    }
    fn page_budget_cancellable(
        &mut self,
        limit: usize,
        budget: usize,
        cancelled: &impl Fn() -> bool,
    ) -> anyhow::Result<Vec<runtime::TranscriptMessage>> {
        match self {
            Self::Jsonl(c) => c.page_budget_cancellable(limit, budget, cancelled),
            Self::OpenCode(c) => c.page_budget_cancellable(limit, budget, cancelled),
        }
    }
    fn source_matches(&self) -> bool {
        match self {
            Self::Jsonl(c) => c.source_matches(),
            Self::OpenCode(c) => c.source_matches(),
        }
    }
    fn checkpoint(&self) -> NativeCheckpoint {
        match self {
            Self::Jsonl(c) => NativeCheckpoint::Jsonl(Box::new(c.checkpoint())),
            Self::OpenCode(c) => NativeCheckpoint::OpenCode(Box::new(c.checkpoint())),
        }
    }
    fn artifacts(&self) -> Vec<Arc<crate::history_artifact::HistoryArtifact>> {
        match self {
            Self::Jsonl(c) => c.artifacts(),
            Self::OpenCode(c) => c.artifacts(),
        }
    }
    fn restore(
        c: NativeCheckpoint,
        resolve: &mut impl FnMut(&Path) -> anyhow::Result<Arc<crate::history_artifact::HistoryArtifact>>,
    ) -> anyhow::Result<Self> {
        Ok(match c {
            NativeCheckpoint::Jsonl(c) => Self::Jsonl(Box::new(
                runtime::history::HistoryCursor::restore(*c, None, resolve)?,
            )),
            NativeCheckpoint::OpenCode(c) => Self::OpenCode(Box::new(
                opencode_history::OpenCodeCursor::restore(*c, resolve)?,
            )),
        })
    }
    fn has_older(&self) -> bool {
        match self {
            Self::Jsonl(c) => c.has_older(),
            Self::OpenCode(c) => c.has_older(),
        }
    }
    fn skip_local_copy(
        &mut self,
        fingerprint: &crate::store::LocalFingerprint,
        expected: &rusqlite::blob::Blob<'_>,
        cancelled: &impl Fn() -> bool,
    ) -> anyhow::Result<bool> {
        match self {
            Self::Jsonl(c) => c.skip_local_copy_cancellable(fingerprint, expected, cancelled),
            Self::OpenCode(c) => c.skip_local_copy_cancellable(fingerprint, expected, cancelled),
        }
    }
}

const VIEWPORT_BYTES: usize = 2 * 1024 * 1024;
fn default_source_range() -> (f64, f64) {
    (0., 1.)
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CursorCheckpoint {
    native: Option<NativeCheckpoint>,
    local: crate::store::MessageCheckpoint,
    checked_overlap: bool,
}
impl CursorCheckpoint {
    fn rewrite(&mut self, map: &HashMap<PathBuf, PathBuf>) {
        if let Some(native) = &mut self.native {
            native.rewrite(map);
        }
        self.local.rewrite_artifacts(map);
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CursorPair {
    before: CursorCheckpoint,
    after: CursorCheckpoint,
    limit: usize,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct ViewportAsset {
    name: PathBuf,
    stamp: crate::history_artifact::FileStamp,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct SavedViewport {
    version: u8,
    id: String,
    page: usize,
    local_head: Option<i64>,
    messages: Vec<runtime::TranscriptMessage>,
    has_older: bool,
    #[serde(default = "default_source_range")]
    range: (f64, f64),
    pages: BTreeMap<usize, CursorPair>,
    assets: Vec<ViewportAsset>,
}
fn read_bounded<T: serde::de::DeserializeOwned>(path: &Path, limit: usize) -> anyhow::Result<T> {
    let file = File::open(path)?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= limit,
        "History checkpoint exceeded its budget"
    );
    Ok(serde_json::from_slice(&bytes)?)
}
fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    let result = (|| -> anyhow::Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
fn single_component(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(std::path::Component::Normal(_))
    ) && path.components().count() == 1
}
struct HistoryState {
    generation: u64,
    native: Option<NativeHistory>,
    local: crate::store::MessageCursor,
    cache_dir: PathBuf,
    pages: BTreeMap<usize, bool>,
    next_page: usize,
    checkpoints: HashSet<usize>,
    page_before: Option<CursorCheckpoint>,
    held_artifacts: HashMap<PathBuf, Arc<crate::history_artifact::HistoryArtifact>>,
    local_head: Option<i64>,
    last_persisted: Option<(usize, usize)>,
    checked_overlap: bool,
    overlap_before: i64,
    provenance_agent: Option<discovery::AgentKind>,
    represented_range: (f64, f64),
}
impl Drop for HistoryState {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.cache_dir);
    }
}
impl HistoryState {
    fn new(session: &SessionSummary, generation: u64, store: &Store) -> anyhow::Result<Self> {
        Self::new_cancellable(session, generation, store, || false)
    }
    fn new_cancellable(
        session: &SessionSummary,
        generation: u64,
        store: &Store,
        cancelled: impl Fn() -> bool,
    ) -> anyhow::Result<Self> {
        Self::new_with_native(session, generation, store, cancelled, None)
    }
    fn new_with_native(
        session: &SessionSummary,
        generation: u64,
        store: &Store,
        cancelled: impl Fn() -> bool,
        native_override: Option<NativeHistory>,
    ) -> anyhow::Result<Self> {
        let baseline = store.conversation_baseline(&session.id)?;
        let native_session = baseline.as_ref().map_or(session, |(source, _)| source);
        let native = if native_override.is_some() {
            native_override
        } else if let Some(path) = &native_session.transcript_path {
            if path.is_file() {
                Some(
                    if native_session.agent == crate::discovery::AgentKind::OpenCode {
                        NativeHistory::OpenCode(Box::new(opencode_history::OpenCodeCursor::open(
                            path,
                            native_session.external_id.as_deref().unwrap_or(&session.id),
                        )?))
                    } else {
                        NativeHistory::Jsonl(Box::new(
                            runtime::history::HistoryCursor::open_cancellable(
                                path,
                                native_session.agent,
                                &cancelled,
                            )?,
                        ))
                    },
                )
            } else if store.message_count(&session.id)? == 0 {
                return Err(anyhow::anyhow!(
                    "This conversation's transcript is missing from its saved location and could not be found in the agent's history or archive. No messages were removed."
                ));
            } else {
                None
            }
        } else {
            None
        };
        let local_head = store.viewport_head(&session.id)?;
        let local = crate::store::MessageCursor::snapshot(native.is_some(), local_head);
        let cache_dir =
            std::env::temp_dir().join(format!("orbit-history-pages-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&cache_dir)?;
        std::fs::set_permissions(&cache_dir, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            generation,
            native,
            local,
            cache_dir,
            pages: BTreeMap::new(),
            next_page: 0,
            checkpoints: HashSet::new(),
            page_before: None,
            held_artifacts: HashMap::new(),
            local_head,
            last_persisted: None,
            checked_overlap: false,
            overlap_before: baseline.as_ref().map_or(i64::MAX, |(_, before)| *before),
            provenance_agent: baseline.as_ref().map(|(source, _)| source.agent),
            represented_range: (1., 1.),
        })
    }
    fn has_older(&self) -> bool {
        self.local.has_older()
            || self
                .native
                .as_ref()
                .is_some_and(|native| native.has_older())
    }
    fn next(
        &mut self,
        store: &Store,
        session: &SessionSummary,
        limit: usize,
    ) -> anyhow::Result<Vec<runtime::TranscriptMessage>> {
        self.next_cancellable(store, session, limit, &|| false)
    }
    fn next_cancellable(
        &mut self,
        store: &Store,
        session: &SessionSummary,
        limit: usize,
        cancelled: &impl Fn() -> bool,
    ) -> anyhow::Result<Vec<runtime::TranscriptMessage>> {
        let end = self.source_position(store, session)?;
        let page = self.next_inner(store, session, limit, cancelled)?;
        let start = self.source_position(store, session)?;
        self.represented_range = (
            self.represented_range.0.min(start),
            self.represented_range.1.max(end),
        );
        Ok(page)
    }
    fn next_inner(
        &mut self,
        store: &Store,
        session: &SessionSummary,
        limit: usize,
        cancelled: &impl Fn() -> bool,
    ) -> anyhow::Result<Vec<runtime::TranscriptMessage>> {
        anyhow::ensure!(!cancelled(), "History seek was superseded");
        if let Some((source, before)) = store.conversation_baseline(&session.id)? {
            self.provenance_agent = Some(source.agent);
            self.overlap_before = before;
        }
        self.page_before = Some(self.checkpoint());
        self.retain_artifacts();
        let local =
            store.lazy_messages_cancellable(&session.id, &mut self.local, limit, cancelled)?;
        let local: Vec<runtime::TranscriptMessage> = local
            .into_iter()
            .map(|m| runtime::TranscriptMessage {
                code_language: m.code_language,
                role: if m.role == "assistant"
                    && let Some(agent) = self.provenance_agent
                {
                    format!(
                        "assistant:{}",
                        m.provider.as_deref().unwrap_or(agent.command())
                    )
                } else {
                    m.role
                },
                text: m.text,
            })
            .collect();
        let used = local.iter().map(|m| m.text.len()).sum::<usize>();
        if self.local.has_older() || local.len() >= limit {
            return Ok(local);
        }
        if !self.checked_overlap {
            self.checked_overlap = true;
            if let Some(cursor) = self.native.as_mut() {
                let mut before = self.overlap_before;
                while let Some((id, fingerprint)) = store.local_fingerprint(&session.id, before)? {
                    if !cursor.skip_local_copy(&fingerprint, &store.message_blob(id)?, cancelled)? {
                        break;
                    }
                    before = id;
                }
            }
        }
        if let Some(before) = &mut self.page_before
            && self.checked_overlap
        {
            // Rehydrating a known boundary must not repeat exact seam comparison
            // through a giant local/native duplicate. Local position stays at
            // the page start; native position adopts the already checked seam.
            if !before.checked_overlap {
                before.native = self.native.as_ref().map(NativeHistory::checkpoint);
                before.checked_overlap = true;
            }
        }
        self.retain_artifacts();
        let mut native = if let Some(cursor) = self.native.as_mut() {
            cursor.page_budget_cancellable(
                limit - local.len(),
                runtime::history::PAGE_BYTES - used,
                cancelled,
            )?
        } else {
            Vec::new()
        };
        if let Some(agent) = self.provenance_agent {
            for message in &mut native {
                if message.role == "assistant" {
                    message.role = format!("assistant:{}", agent.command());
                }
            }
        }
        native.extend(local);
        Ok(native)
    }
    fn native_split(&self, store: &Store, session: &SessionSummary) -> anyhow::Result<f64> {
        // Keep both immutable native history and appended Orbit turns reachable
        // without COUNT/SUM or decoding every source body. This is approximate.
        Ok(if self.native.is_some() {
            if store.history_extent(&session.id, true)?.is_some() {
                0.75
            } else {
                1.
            }
        } else {
            0.
        })
    }
    fn source_position(&self, store: &Store, session: &SessionSummary) -> anyhow::Result<f64> {
        let split = self.native_split(store, session)?;
        if self.local.has_older() {
            Ok(split + (1. - split) * store.history_fraction(&session.id, &self.local)?)
        } else {
            Ok(self
                .native
                .as_ref()
                .map(|c| c.source_fraction())
                .transpose()?
                .unwrap_or(0.)
                * split)
        }
    }
    fn seek(
        &mut self,
        store: &Store,
        session: &SessionSummary,
        fraction: f64,
        limit: usize,
        cancelled: &impl Fn() -> bool,
    ) -> anyhow::Result<Vec<runtime::TranscriptMessage>> {
        anyhow::ensure!(!cancelled(), "History seek was superseded");
        let split = self.native_split(store, session)?;
        if fraction >= 1. {
            if let Some(native) = &mut self.native {
                native.seek_fraction(1., cancelled)?;
            }
            return self.next_cancellable(store, session, limit, cancelled);
        }
        if fraction < split
            && let Some(native) = &mut self.native
        {
            self.local = crate::store::MessageCursor::snapshot(true, None);
            self.checked_overlap = true;
            native.seek_fraction(if split > 0. { fraction / split } else { 0. }, cancelled)?;
        } else {
            if let Some(native) = &mut self.native {
                native.seek_fraction(1., cancelled)?;
            }
            self.local = store.seek_messages(
                &session.id,
                self.native.is_some(),
                if split < 1. {
                    (fraction - split) / (1. - split)
                } else {
                    0.
                },
            )?;
            // The native source remains at its tail for seam traversal once
            // all earlier local turns in this selected window are consumed.
        }
        self.represented_range = (fraction, fraction);
        let mut page = self.next_cancellable(store, session, limit, cancelled)?;
        if fraction == 0. {
            let local_head_end = self.local.head_end;
            self.local = crate::store::MessageCursor::snapshot(true, None);
            // Native head-page cursors have already marked the beginning.
            self.represented_range.0 = 0.;
            self.represented_range.1 = if let Some(native) = &self.native {
                native.head_end().unwrap_or(0.) * split
            } else {
                local_head_end.unwrap_or(0.)
            };
        }
        // No output is silently dropped; existing bounded page budgets apply.
        page.shrink_to_fit();
        Ok(page)
    }
    fn checkpoint(&self) -> CursorCheckpoint {
        CursorCheckpoint {
            native: self.native.as_ref().map(NativeHistory::checkpoint),
            local: self.local.checkpoint(),
            checked_overlap: self.checked_overlap,
        }
    }
    fn retain_artifacts(&mut self) {
        let mut artifacts = self.local.artifacts();
        if let Some(native) = &self.native {
            artifacts.extend(native.artifacts());
        }
        for artifact in artifacts {
            self.held_artifacts
                .entry(artifact.path.clone())
                .or_insert(artifact);
        }
    }
    fn cache(
        &mut self,
        page: &[runtime::TranscriptMessage],
        has_older: bool,
    ) -> anyhow::Result<()> {
        let number = self.next_page;
        self.next_page += 1;
        fs::write(
            self.cache_dir.join(format!("{number}.json")),
            serde_json::to_vec(page)?,
        )?;
        let before = self.page_before.take().unwrap_or_else(|| self.checkpoint());
        let after = self.checkpoint();
        self.retain_artifacts();
        fs::write(
            self.cache_dir.join(format!("{number}.cursor")),
            serde_json::to_vec(&CursorPair {
                before,
                after,
                limit: page.len().clamp(1, 400),
            })?,
        )?;
        self.checkpoints.insert(number);
        self.pages.insert(number, has_older);
        Ok(())
    }
    fn cached(
        &mut self,
        page: usize,
        store: &Store,
        session: &SessionSummary,
    ) -> anyhow::Result<(Vec<runtime::TranscriptMessage>, bool)> {
        let raw = self.cache_dir.join(format!("{page}.json"));
        if let Some(older) = self.pages.get(&page) {
            let mut messages: Vec<runtime::TranscriptMessage> =
                read_bounded(&raw, 2 * 1024 * 1024)?;
            if let Some((source, _)) = store.conversation_baseline(&session.id)? {
                for message in &mut messages {
                    if message.role == "assistant" {
                        message.role = format!("assistant:{}", source.agent.command());
                    }
                }
            }
            return Ok((messages, *older));
        }
        anyhow::ensure!(
            self.checkpoints.contains(&page),
            "History page unavailable; return to the latest messages"
        );
        let pair: CursorPair = read_bounded(
            &self.cache_dir.join(format!("{page}.cursor")),
            2 * 1024 * 1024,
        )?;
        let held = self.held_artifacts.clone();
        let mut resolve = |path: &Path| {
            held.get(path)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("History artifact unavailable"))
        };
        let mut state = Self::from_checkpoint(
            pair.before,
            self.generation,
            // Cursor offsets remain valid when newer turns are appended.
            // Rehydrating an old page must not invalidate its snapshot merely
            // because the same chat continued with another agent.
            store.viewport_head(&session.id)?,
            store,
            session,
            &mut resolve,
        )?;
        let messages = state.next(store, session, pair.limit.clamp(1, 400))?;
        let older = state.has_older();
        fs::write(&raw, serde_json::to_vec(&messages)?)?;
        self.pages.insert(page, older);
        Ok((messages, older))
    }
    fn persist_viewport(
        &mut self,
        root: &Path,
        id: &str,
        page: usize,
        store: &Store,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            store.viewport_head(id)? == self.local_head,
            "Conversation changed since this viewport was loaded"
        );
        anyhow::ensure!(
            self.native
                .as_ref()
                .is_none_or(NativeHistory::source_matches),
            "Native source changed since this viewport was loaded"
        );
        if self.last_persisted == Some((page, self.checkpoints.len())) {
            return Ok(());
        }
        let (messages, has_older) = self.cached(
            page,
            store,
            &store
                .session(id)?
                .ok_or_else(|| anyhow::anyhow!("Conversation unavailable"))?,
        )?;
        let mut budget = serde_json::to_vec(&messages)?.len();
        let mut pairs = BTreeMap::new();
        for number in &self.checkpoints {
            let path = self.cache_dir.join(format!("{number}.cursor"));
            budget = budget.saturating_add(fs::metadata(&path)?.len() as usize);
            anyhow::ensure!(
                budget < VIEWPORT_BYTES && pairs.len() < 4096,
                "Saved viewport navigation exceeded its metadata budget"
            );
            pairs.insert(*number, read_bounded::<CursorPair>(&path, VIEWPORT_BYTES)?);
        }
        fs::create_dir_all(root)?;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        let name = format!("snapshot-{}", uuid::Uuid::new_v4());
        let folder = root.join(&name);
        fs::create_dir(&folder)?;
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))?;
        struct Folder(PathBuf);
        impl Drop for Folder {
            fn drop(&mut self) {
                if !self.0.as_os_str().is_empty() {
                    let _ = fs::remove_dir_all(&self.0);
                }
            }
        }
        let mut cleanup = Folder(folder.clone());
        let mut map = HashMap::new();
        let mut assets = Vec::new();
        for artifact in self.held_artifacts.values() {
            let file = PathBuf::from(format!("artifact-{}", assets.len()));
            let target = folder.join(&file);
            crate::history_artifact::HistoryArtifact::link_to(&artifact.path, &target)?;
            File::open(&target)?.sync_all()?;
            map.insert(artifact.path.clone(), file.clone());
            assets.push(ViewportAsset {
                name: file,
                stamp: crate::history_artifact::FileStamp::read(&target)?,
            });
        }
        for pair in pairs.values_mut() {
            pair.before.rewrite(&map);
            pair.after.rewrite(&map);
        }
        let saved = SavedViewport {
            version: 2,
            id: id.into(),
            page,
            local_head: self.local_head,
            messages,
            has_older,
            range: self.represented_range,
            pages: pairs,
            assets,
        };
        let encoded = serde_json::to_vec(&saved)?;
        anyhow::ensure!(
            encoded.len() <= VIEWPORT_BYTES,
            "Saved viewport exceeded its metadata budget"
        );
        write_atomic(&folder.join("manifest.json"), &encoded)?;
        File::open(&folder)?.sync_all()?;
        let old = read_bounded::<String>(&root.join("current.json"), 1024)
            .ok()
            .filter(|s| single_component(Path::new(s)));
        write_atomic(&root.join("current.json"), &serde_json::to_vec(&name)?)?;
        File::open(root)?.sync_all()?;
        cleanup.0 = PathBuf::new();
        if let Some(old) = old {
            let _ = fs::remove_dir_all(root.join(old));
        }
        self.last_persisted = Some((page, self.checkpoints.len()));
        Ok(())
    }
    fn restore_viewport(
        root: &Path,
        session: &SessionSummary,
        generation: u64,
        page: usize,
        store: &Store,
    ) -> anyhow::Result<(Self, Vec<runtime::TranscriptMessage>, bool)> {
        let name: String = read_bounded(&root.join("current.json"), 1024)?;
        anyhow::ensure!(
            single_component(Path::new(&name)),
            "Invalid viewport snapshot directory"
        );
        let folder = root.join(name);
        let saved: SavedViewport = read_bounded(&folder.join("manifest.json"), VIEWPORT_BYTES)?;
        anyhow::ensure!(
            page <= 1_000_000_000
                && matches!(saved.version,1|2)
                && saved.id == session.id
                && saved.page == page
                && saved.pages.len() <= 4096
                && saved.assets.len() <= 8192
                && saved.messages.len() <= 400
                && saved.messages.iter().map(|m| m.text.len()).sum::<usize>()
                    <= runtime::history::PAGE_BYTES,
            "Saved viewport does not match this conversation"
        );
        let mut artifacts = HashMap::new();
        let mut rewrite = HashMap::new();
        for asset in &saved.assets {
            anyhow::ensure!(
                single_component(&asset.name) && !artifacts.contains_key(&asset.name),
                "Invalid viewport artifact path"
            );
            let path = folder.join(&asset.name);
            anyhow::ensure!(
                fs::symlink_metadata(&path)?.is_file() && asset.stamp.matches(&path),
                "History artifact changed since the saved viewport"
            );
            let artifact = crate::history_artifact::HistoryArtifact::linked(&path)?;
            rewrite.insert(asset.name.clone(), artifact.path.clone());
            artifacts.insert(asset.name.clone(), artifact);
        }
        let mut resolve = |name: &Path| {
            artifacts
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Missing viewport artifact"))
        };
        let mut pairs = saved.pages;
        let mut selected = pairs
            .remove(&page)
            .ok_or_else(|| anyhow::anyhow!("Saved viewport boundary unavailable"))?;
        let mut state = Self::from_checkpoint(
            if saved.version==1{selected.before.clone()}else{selected.after.clone()},
            generation,
            saved.local_head,
            store,
            session,
            &mut resolve,
        )?;
        // Version-one viewports cached physical fragments that could cut an
        // app-owned card in half. Replay their original source checkpoint with
        // the structured pager, preserving the selected chat and scroll page.
        let (messages,has_older)=if saved.version==1{
            let messages=state.next(store,session,selected.limit)?;
            selected.after=state.checkpoint();state.retain_artifacts();
            let older=state.has_older();(messages,older)
        }else{(saved.messages,saved.has_older)};
        state.next_page = page + 1;
        state.represented_range = saved.range;
        state.pages.insert(page, has_older);
        pairs.insert(page, selected);
        for (number, mut pair) in pairs {
            pair.before.rewrite(&rewrite);
            pair.after.rewrite(&rewrite);
            fs::write(
                state.cache_dir.join(format!("{number}.cursor")),
                serde_json::to_vec(&pair)?,
            )?;
            state.checkpoints.insert(number);
        }
        for artifact in artifacts.into_values() {
            state.held_artifacts.insert(artifact.path.clone(), artifact);
        }
        fs::write(
            state.cache_dir.join(format!("{page}.json")),
            serde_json::to_vec(&messages)?,
        )?;
        Ok((state, messages, has_older))
    }
    fn from_checkpoint(
        c: CursorCheckpoint,
        generation: u64,
        local_head: Option<i64>,
        store: &Store,
        session: &SessionSummary,
        resolve: &mut impl FnMut(&Path) -> anyhow::Result<Arc<crate::history_artifact::HistoryArtifact>>,
    ) -> anyhow::Result<Self> {
        let baseline = store.conversation_baseline(&session.id)?;
        let native_session = baseline.as_ref().map_or(session, |(source, _)| source);
        if let Some(native) = &c.native {
            anyhow::ensure!(
                match native {
                    NativeCheckpoint::Jsonl(c) => c.agent == native_session.agent,
                    NativeCheckpoint::OpenCode(_) =>
                        native_session.agent == crate::discovery::AgentKind::OpenCode,
                },
                "Saved viewport uses a different agent adapter"
            );
            let source = match native {
                NativeCheckpoint::Jsonl(c) => &c.source,
                NativeCheckpoint::OpenCode(c) => c.source_path(),
            };
            anyhow::ensure!(
                native_session.transcript_path.as_ref() == Some(source),
                "Saved viewport uses a different native source"
            );
        }
        anyhow::ensure!(
            store.viewport_head(&session.id)? == local_head,
            "Saved conversation changed; reopening its latest messages"
        );
        let mut state = Self::new(session, generation, store)?;
        state.native = c
            .native
            .map(|c| NativeHistory::restore(c, resolve))
            .transpose()?;
        state.local = crate::store::MessageCursor::restore(c.local, store, &session.id, resolve)?;
        state.checked_overlap = c.checked_overlap;
        state.local_head = local_head;
        Ok(state)
    }
}

#[cfg(test)]
fn ensure_imported(store: &Store, session: &SessionSummary) -> anyhow::Result<()> {
    store.upsert_session(session)?;
    if store.transcript_fully_imported(&session.id)?
        || (store.transcript_imported(&session.id)?
            && session
                .transcript_path
                .as_ref()
                .is_none_or(|path| !path.is_file()))
    {
        return Ok(());
    }
    if let Some(path) = &session.transcript_path {
        let messages = runtime::transcript_messages(path, session.agent)?.map(|message| {
            message.map(|message| StoredMessage {
                role: message.role,
                text: message.text,
            })
        });
        store.import_messages_stream(&session.id, messages)
    } else {
        store.import_messages_once(&session.id, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::AgentKind;
    use std::fs;

    #[test]
    #[ignore="Read-only check of an explicitly selected native transcript"]
    fn selected_native_history_has_typed_tool_cards_without_escaped_envelopes(){
        let path=std::path::PathBuf::from(std::env::var("KAI_HISTORY_REGRESSION_FILE").expect("Select the native transcript"));
        let mut cursor=runtime::history::HistoryCursor::open(&path,AgentKind::Codex).unwrap();let mut cards=0;
        for _ in 0..10{if !cursor.has_older(){break}let page=cursor.page(50).unwrap();let parsed=parse_messages(page);
            for message in parsed{if message.role=="tool"{for block in message.blocks{if let presentation::MessageBlock::Activity{..}|presentation::MessageBlock::Interaction(_)=block{cards+=1;}else if let presentation::MessageBlock::Paragraph(text)|presentation::MessageBlock::RichParagraph{text,..}|presentation::MessageBlock::Code{text,..}=block{assert!(!text.contains("\"orbit_tool\""),"Tool JSON escaped into the transcript");}}}}
        }
        assert!(cards>0,"The selected fixture had no tool history");eprintln!("Actual selected native history: {cards} typed tool/interaction cards, no escaped tool envelopes");
    }
    #[test]
    fn typed_tool_envelopes_decode_before_native_fence_context(){
        let source="diff --git a/view.swift b/view.swift\n+ var label = \"Hello\"\n```swift\nlet text = 1\n";
        let parsed=parse_messages(vec![runtime::TranscriptMessage{role:"tool".into(),code_language:Some("swift".into()),text:serde_json::json!({"orbit_tool":true,"label":"Updated files","details":source}).to_string()}]);
        assert!(matches!(&parsed[0].blocks[0],presentation::MessageBlock::Activity{label,details}if label=="Updated files" && details==source));
        assert_eq!(parsed[0].blocks.len(),1);
    }
    fn receive_event(engine: &Engine) -> Event {
        loop {
            let event = engine.events.recv_blocking().unwrap();
            if !matches!(event, Event::HistoryRange(..)) {
                return event;
            }
        }
    }
    fn temp_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("orbit-engine-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn handoff_after_global_head_seek_uses_latest_context_instead_of_cached_head() {
        let root = temp_root();
        let session = imported(&root);
        let mut file = File::create(session.transcript_path.as_ref().unwrap()).unwrap();
        for n in 0..1000 {
            file.write_all(record("user", &format!("request {n:04}")).as_bytes())
                .unwrap();
        }
        drop(file);
        let engine = Engine::new(root.clone(), root.clone(), root.join("store.sqlite"));
        let _ = receive_event(&engine);
        engine.seek_history(session.clone(), 1, 0.);
        assert!(
            matches!(receive_event(&engine),Event::HistorySeeked(_,1,_,_,_,fraction,_) if fraction==0.)
        );
        let token = uuid::Uuid::new_v4();
        engine.prepare_handoff(session.clone(), token, false);
        let Event::Handoff(id, Ok(packet)) = receive_event(&engine) else {
            panic!("expected handoff");
        };
        assert_eq!(id, token);
        assert!(packet.recent.contains("request 0999"));
        assert!(!packet.recent.contains("request 0000"));
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn older_reads_are_cancelled_when_a_new_global_seek_supersedes_their_generation() {
        let latest = LatestReads::default();
        latest.history.store(2, Ordering::Release);
        let root = temp_root();
        assert!(latest.stale(&Command::Older(imported(&root), 1, 200)));
        assert!(!latest.stale(&Command::Older(imported(&root), 2, 200)));
        assert!(latest.stale(&Command::HistoryPage(imported(&root), 1, 0)));
        assert!(!latest.stale(&Command::HistoryPage(imported(&root), 2, 0)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sidebar_project_branches_load_independently_of_active_project_generations() {
        let root = temp_root();
        let first = root.join("first");
        let second = root.join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let engine = Engine::new(root.clone(), root.clone(), root.join("store.sqlite"));
        let _ = receive_event(&engine);
        for (id, path) in [("tree-first", &first), ("tree-second", &second)] {
            let mut session = imported(path);
            session.id = id.into();
            session.external_id = None;
            session.transcript_path = None;
            engine.save(session, "user", format!("A request for {id}"));
        }
        // The active-project pane has its own generation. Neither sidebar
        // request mutates it or supersedes another expanded branch's token.
        engine.latest.project.store(100, Ordering::Release);
        engine.load_sidebar_project(first.clone(), 1);
        engine.load_sidebar_project(second.clone(), 2);
        for (path, token, id) in [(first, 1, "tree-first"), (second, 2, "tree-second")] {
            let Event::SidebarProject(actual, generation, sessions) = receive_event(&engine) else {
                panic!("expected sidebar summaries");
            };
            assert_eq!(actual, path);
            assert_eq!(generation, token);
            assert_eq!(sessions.len(), 1);
            assert_eq!(sessions[0].id, id);
        }
        assert_eq!(engine.latest.project.load(Ordering::Acquire), 100);
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sidebar_project_fetch_errors_keep_their_branch_path_and_token() {
        let root = temp_root();
        let database = root.join("store.sqlite");
        let engine = Engine::new(root.clone(), root.clone(), database.clone());
        let _ = receive_event(&engine);
        // A damaged fixture schema exercises the real actor's scoped failure
        // path without touching any native database or transcript.
        let damaged = rusqlite::Connection::open(&database).unwrap();
        damaged.execute_batch("DROP TABLE sessions;").unwrap();
        drop(damaged);
        engine.load_sidebar_project(root.clone(), 42);
        let Event::SidebarProjectError(path, token, error) = receive_event(&engine) else {
            panic!("expected branch-scoped sidebar error");
        };
        assert_eq!(path, root);
        assert_eq!(token, 42);
        assert!(error.contains("sessions"));
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fractional_archive_seek_reuses_the_prepared_seekable_source() {
        let root = temp_root();
        let mut session = imported(&root);
        let archive = root.join("native.jsonl.zst");
        let mut raw = String::new();
        for n in 0..2000 {
            raw.push_str(&record("user", &format!("archived {n:04}")));
        }
        fs::write(
            &archive,
            zstd::stream::encode_all(raw.as_bytes(), 1).unwrap(),
        )
        .unwrap();
        session.transcript_path = Some(archive);
        let store = Store::open(&root.join("store.sqlite")).unwrap();
        store.upsert_session(&session).unwrap();
        let mut state = HistoryState::new(&session, 1, &store).unwrap();
        let page = state.seek(&store, &session, 0.6, 50, &|| false).unwrap();
        state.cache(&page, state.has_older()).unwrap();
        let prepared = state.native.as_ref().unwrap().artifacts()[0].path.clone();
        let held = state.held_artifacts.clone();
        let reusable =
            NativeHistory::restore(state.native.as_ref().unwrap().checkpoint(), &mut |path| {
                Ok(held.get(path).unwrap().clone())
            })
            .unwrap();
        let mut next =
            HistoryState::new_with_native(&session, 2, &store, || false, Some(reusable)).unwrap();
        let head = next.seek(&store, &session, 0., 50, &|| false).unwrap();
        assert_eq!(head[0].text, "archived 0000");
        assert_eq!(next.native.as_ref().unwrap().artifacts()[0].path, prepared);
        drop(next);
        drop(state);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn fractional_global_seek_preserves_exact_endpoints_and_mixed_source_seam() {
        let root = temp_root();
        let session = imported(&root);
        let mut file = File::create(session.transcript_path.as_ref().unwrap()).unwrap();
        for n in 0..5000 {
            file.write_all(record("user", &format!("native {n:04}")).as_bytes())
                .unwrap();
        }
        drop(file);
        let store = Store::open(&root.join("store.sqlite")).unwrap();
        store.upsert_session(&session).unwrap();
        for n in 0..50 {
            store
                .save_message(&session.id, "user", &format!("local {n:04}"))
                .unwrap();
        }
        for f in [0., 0.375, 0.9, 1.] {
            let mut state = HistoryState::new(&session, 1, &store).unwrap();
            let page = state.seek(&store, &session, f, 200, &|| false).unwrap();
            assert!(page.len() <= 200);
            assert!(
                page.iter().map(|m| m.text.len()).sum::<usize>() <= runtime::history::PAGE_BYTES
            );
            if f == 0. {
                assert_eq!(page[0].text, "native 0000");
                assert!(!state.has_older());
                assert_eq!(state.represented_range.0, 0.);
            }
            if f == 1. {
                assert_eq!(page.last().unwrap().text, "local 0049");
                assert_eq!(state.represented_range.1, 1.);
            }
            if f == 0.375 {
                assert!(page.iter().any(|m| m.text == "native 2500"));
                assert!(!page.iter().any(|m| m.text.starts_with("local")));
            }
            assert!(state.represented_range.0 <= state.represented_range.1);
        }
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn newer_fractional_seek_supersedes_queued_drag_positions() {
        let root = temp_root();
        let engine = Engine::new(root.clone(), root.clone(), root.join("store.sqlite"));
        let _ = receive_event(&engine);
        let mut session = imported(&root);
        session.transcript_path = None;
        engine.save(session.clone(), "user", "Oldest".into());
        engine.save(session.clone(), "assistant", "Newest".into());
        engine.seek_history(session.clone(), 100, 0.);
        engine.seek_history(session.clone(), 101, 1.);
        loop {
            match receive_event(&engine) {
                Event::HistorySeeked(id, generation, _, _, newer, f, _) => {
                    if generation != 101 {
                        continue;
                    }
                    assert_eq!(id, session.id);
                    assert_eq!(f, 1.);
                    assert!(!newer);
                    break;
                }
                Event::LoadError(_, generation, error) => panic!("{generation}: {error}"),
                _ => {}
            }
        }
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn completed_disk_response_returns_to_lazy_history_instead_of_parsing_all_output() {
        let root = temp_root();
        let database = root.join("history.sqlite");
        let engine = Engine::new(root.join("home"), root.clone(), database.clone());
        let _ = receive_event(&engine);
        let session = SessionSummary {
            id: "paged-finish".into(),
            title: "Paged response".into(),
            agent: AgentKind::Codex,
            project_path: root.clone(),
            execution_path: None,
            updated_at: 1,
            external_id: None,
            transcript_path: None,
        };
        let mut output = crate::response_buffer::ResponseBuffer::default();
        for index in 0..8000 {
            output.push_str(&format!("Row {index:05} {}\n", "🦀ab".repeat(500)));
        }
        let source = output.into_source().unwrap();
        let bytes = source.len();
        assert!(bytes > 20 * 1024 * 1024);
        engine.finish_source(session.clone(), source);
        assert!(matches!(receive_event(&engine),Event::PagedFinished(id) if id == session.id));
        engine.load(session.clone(), 1);
        let Event::Loaded(id, _, messages, older, _) = receive_event(&engine) else {
            panic!("Expected a lazy bounded page");
        };
        assert_eq!(id, session.id);
        assert!(older && !messages.is_empty());
        assert!(messages.iter().map(|m| m.blocks.len()).sum::<usize>() < 500);
        let store = Store::open(&database).unwrap();
        let mut cursor = crate::store::MessageCursor::new(true);
        let mut total = 0;
        while cursor.has_older() || total == 0 {
            total += store
                .lazy_messages(&session.id, &mut cursor, 400)
                .unwrap()
                .iter()
                .map(|m| m.text.len())
                .sum::<usize>();
        }
        assert_eq!(total, bytes);
        drop(engine);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    fn record(role: &str, text: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({"type":"response_item","payload":{
                "type":"message","role":role,"content":[{"type":"input_text","text":text}]
            }})
        )
    }

    fn imported(root: &std::path::Path) -> SessionSummary {
        SessionSummary {
            id: "codex:external".into(),
            title: "Original title".into(),
            agent: AgentKind::Codex,
            project_path: root.to_owned(),
            execution_path: None,
            updated_at: 10,
            external_id: Some("external".into()),
            transcript_path: Some(root.join("native.jsonl")),
        }
    }

    fn loaded(engine: &Engine) -> Vec<ParsedMessage> {
        match receive_event(engine) {
            Event::Loaded(_, _, messages, _, _) => messages,
            Event::Error(error) | Event::LoadError(_, _, error) => {
                panic!("state worker failed: {error}")
            }
            _ => panic!("unexpected state worker event"),
        }
    }

    #[test]
    fn revisiting_imported_session_preserves_history_and_does_not_duplicate_native_turns() {
        let root = temp_root();
        let session = imported(&root);
        let initial = format!(
            "{}{}",
            record("user", "Earlier question"),
            record("assistant", "Earlier answer")
        );
        fs::write(session.transcript_path.as_ref().unwrap(), &initial).unwrap();
        let engine = Engine::new(root.clone(), root.clone(), root.join("history.sqlite3"));
        assert!(matches!(receive_event(&engine), Event::Ready(_, _, _)));
        engine.load(session.clone(), 1);
        assert_eq!(loaded(&engine).len(), 2);
        engine.save(session.clone(), "user", "Follow up".into());
        // The native CLI records the same new turn while Orbit stores its own
        // response. The original Load already captured the initial snapshot.
        fs::write(
            session.transcript_path.as_ref().unwrap(),
            format!(
                "{initial}{}{}",
                record("user", "Follow up"),
                record("assistant", "New answer")
            ),
        )
        .unwrap();
        engine.finish(session.clone(), "New answer".into());
        assert!(matches!(receive_event(&engine), Event::Parsed(_, _)));
        engine.load(session.clone(), 2);
        let messages = loaded(&engine);
        assert_eq!(
            messages
                .iter()
                .map(|message| message.role.as_str())
                .collect::<Vec<_>>(),
            ["user", "assistant", "user", "assistant"]
        );
        assert_eq!(messages[0].blocks, presentation::blocks("Earlier question"));
        assert_eq!(messages[3].blocks, presentation::blocks("New answer"));
        // Locally persisted turns remain durable even if the native file moves.
        // Unopened native history stays in its original file, not a full import.
        fs::remove_file(session.transcript_path.as_ref().unwrap()).unwrap();
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        assert_eq!(store.messages(&session.id, 100).unwrap().len(), 2);
        drop(store);
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn handoff_uses_latest_saved_context_and_persists_across_native_merge_and_restart() {
        let root = temp_root();
        let source = imported(&root);
        let raw = record("user", "Original native request");
        fs::write(source.transcript_path.as_ref().unwrap(), &raw).unwrap();
        let database = root.join("handoff.sqlite");
        let engine = Engine::new(root.clone(), root.clone(), database.clone());
        assert!(matches!(receive_event(&engine), Event::Ready(_, _, _)));
        engine.save(source.clone(), "user", "Saved goal MARKER-904".into());
        engine.save(
            source.clone(),
            "assistant",
            "Existing decision about the implementation".into(),
        );
        let token = uuid::Uuid::new_v4();
        engine.prepare_handoff(source.clone(), token, true);
        let Event::Handoff(returned, Ok(packet)) = receive_event(&engine) else {
            panic!("Expected prepared handoff");
        };
        assert_eq!(returned, token);
        assert!(
            packet.objective.contains("MARKER-904") && packet.recent.contains("Existing decision")
        );
        let mut destination = source.clone();
        destination.id = "destination".into();
        destination.agent = AgentKind::Claude;
        destination.external_id = None;
        destination.transcript_path = None;
        engine.bind_handoff(destination.clone(), Arc::new(packet.clone()));
        engine.save_usage(
            destination.id.clone(),
            crate::usage::Usage {
                input: Some(1234),
                output: Some(77),
                ..Default::default()
            },
        );
        engine
            .flush()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        drop(engine);
        let store = Store::open(&database).unwrap();
        assert!(
            store
                .session_handoff(&destination.id)
                .unwrap()
                .unwrap()
                .prompt("Continue")
                .contains("MARKER-904")
        );
        assert_eq!(
            store.session_usage(&destination.id).unwrap().unwrap().input,
            Some(1234)
        );
        assert!(store.session_usage(&source.id).unwrap().is_none());
        destination.external_id = Some("new-native-id".into());
        store.bind_runtime_session(&destination).unwrap();
        let mut canonical = destination.clone();
        canonical.id = "canonical".into();
        store.upsert_session(&canonical).unwrap();
        assert!(store.session_handoff(&canonical.id).unwrap().is_some());
        assert_eq!(
            store.session_usage(&canonical.id).unwrap().unwrap().output,
            Some(77)
        );
        assert_eq!(
            fs::read_to_string(source.transcript_path.as_ref().unwrap()).unwrap(),
            raw
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn handoff_wrapper_is_hidden_and_deduplicated_when_native_history_reopens() {
        let root = temp_root();
        let source = imported(&root);
        let packet = crate::handoff::Packet::build(
            &source,
            None,
            Some("Preserved goal"),
            vec![],
            false,
            false,
        );
        let mut destination = source.clone();
        destination.id = "handoff-destination".into();
        destination.external_id = Some("owned-native".into());
        destination.transcript_path = Some(root.join("destination.jsonl"));
        fs::write(
            destination.transcript_path.as_ref().unwrap(),
            format!(
                "{}{}",
                record("user", &packet.prompt("Continue marker")),
                record("assistant", "Destination reply")
            ),
        )
        .unwrap();
        let store = Store::open(&root.join("history.sqlite")).unwrap();
        store.upsert_session(&destination).unwrap();
        store
            .save_message(&destination.id, "assistant", "Context from previous agent")
            .unwrap();
        store
            .save_message(&destination.id, "user", "Continue marker")
            .unwrap();
        store
            .save_message(&destination.id, "assistant", "Destination reply")
            .unwrap();
        let mut state = HistoryState::new(&destination, 1, &store).unwrap();
        let messages = state.next(&store, &destination, 200).unwrap();
        let visible: Vec<_> = messages
            .iter()
            .filter_map(|m| presentation::sanitize_transcript_text(&m.role, &m.text))
            .collect();
        assert_eq!(
            visible
                .iter()
                .filter(|m| m.as_str() == "Continue marker")
                .count(),
            1
        );
        assert_eq!(
            visible
                .iter()
                .filter(|m| m.as_str() == "Destination reply")
                .count(),
            1
        );
        assert!(
            visible
                .iter()
                .all(|m| !m.contains("code_handoff_context") && !m.contains("Preserved goal"))
        );
        drop(state);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovers_legacy_saved_turns_without_duplicating_them() {
        let root = temp_root();
        let session = imported(&root);
        fs::write(
            session.transcript_path.as_ref().unwrap(),
            format!(
                "{}{}{}{}",
                record("user", "Old question"),
                record("assistant", "Old answer"),
                record("user", "Saved question"),
                record("assistant", "Saved answer")
            ),
        )
        .unwrap();
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        store.upsert_session(&session).unwrap();
        store
            .save_message(&session.id, "user", "Saved question")
            .unwrap();
        store
            .save_message(&session.id, "assistant", "Saved answer")
            .unwrap();
        ensure_imported(&store, &session).unwrap();
        ensure_imported(&store, &session).unwrap();
        assert_eq!(
            store
                .messages(&session.id, 100)
                .unwrap()
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            [
                "Old question",
                "Old answer",
                "Saved question",
                "Saved answer"
            ]
        );
        assert_eq!(store.search("Old answer").unwrap()[0].id, session.id);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn imported_fork_uses_child_rollout_on_finish_and_restart_without_importing_it() {
        let root = temp_root();
        fs::create_dir(root.join(".codex")).unwrap();
        let native_db = rusqlite::Connection::open(root.join(".codex/state_5.sqlite")).unwrap();
        native_db
            .execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY, rollout_path TEXT);")
            .unwrap();
        let parent_path = root.join("native.jsonl");
        let child_path = root.join("child.jsonl");
        fs::write(&parent_path, record("user", "Parent-only source")).unwrap();
        fs::write(&child_path, record("user", "Child native history")).unwrap();
        let engine = Engine::new(root.clone(), root.clone(), root.join("history.sqlite3"));
        assert!(matches!(receive_event(&engine), Event::Ready(_, _, _)));
        let parent = imported(&root);
        engine.save(parent.clone(), "user", "New fork request".into());
        let mut fork = parent.clone();
        fork.external_id = Some("child".into());
        engine.update_session(fork.clone());
        engine.search(String::new(), 1);
        assert!(matches!(receive_event(&engine), Event::Search(_, _)));
        // Native row is committed after SessionId: Finish must retry the lookup.
        native_db
            .execute(
                "INSERT INTO threads VALUES(?1,?2)",
                rusqlite::params!["child", child_path.to_string_lossy()],
            )
            .unwrap();
        engine.finish(fork, "Local child answer".into());
        assert!(matches!(receive_event(&engine), Event::Parsed(_, _)));
        let mut repeated = parent.clone();
        repeated.external_id = Some("child".into());
        engine.update_session(repeated);
        engine.load(parent.clone(), 2);
        let Event::Loaded(id, _, messages, _, _) = receive_event(&engine) else {
            panic!("expected loaded child");
        };
        assert_eq!(id, parent.id);
        assert_eq!(messages.len(), 3);
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        let saved = store.session(&parent.id).unwrap().unwrap();
        assert_eq!(saved.external_id.as_deref(), Some("child"));
        assert_eq!(saved.transcript_path.as_ref(), Some(&child_path));
        assert_eq!(store.message_count(&parent.id).unwrap(), 2);
        assert_eq!(
            fs::read_to_string(&parent_path).unwrap(),
            record("user", "Parent-only source")
        );
        drop(store);
        drop(engine);
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        assert_eq!(
            store
                .session(&parent.id)
                .unwrap()
                .unwrap()
                .external_id
                .as_deref(),
            Some("child")
        );
        drop(store);
        drop(native_db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_id_is_persisted_before_response_completion_without_extra_messages() {
        let root = temp_root();
        let engine = Engine::new(root.clone(), root.clone(), root.join("history.sqlite3"));
        assert!(matches!(receive_event(&engine), Event::Ready(_, _, _)));
        let mut session = imported(&root);
        session.id = "orbit-local-id".into();
        session.external_id = None;
        session.transcript_path = None;
        engine.save(session.clone(), "user", "Start a task".into());
        session.external_id = Some("early-native-id".into());
        engine.update_session(session.clone());
        // Search is a barrier behind the metadata command on the same actor.
        engine.search(String::new(), 1);
        let Event::Search(_, sessions) = receive_event(&engine) else {
            panic!("expected search barrier");
        };
        assert_eq!(sessions[0].external_id.as_deref(), Some("early-native-id"));
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        assert_eq!(store.messages(&session.id, 100).unwrap().len(), 1);
        drop(store);
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovery_reuses_local_native_id_and_stale_loads_preserve_metadata() {
        let root = temp_root();
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        let mut native = imported(&root);
        let mut local = native.clone();
        local.id = "orbit-local-id".into();
        local.title = "Local first prompt".into();
        local.external_id = None;
        local.transcript_path = None;
        ensure_imported(&store, &local).unwrap();
        store
            .save_message(&local.id, "user", "Local first prompt")
            .unwrap();
        // Discovery can run before the CLI's SessionId event reaches Orbit.
        // A second sidebar ID may have its own imported history by then.
        store.upsert_discovered_session(&native).unwrap();
        store
            .import_messages_once(
                &native.id,
                &[
                    StoredMessage {
                        role: "user".into(),
                        text: "Native earlier prompt".into(),
                    },
                    StoredMessage {
                        role: "assistant".into(),
                        text: "Native earlier answer".into(),
                    },
                ],
            )
            .unwrap();
        local.external_id = native.external_id.clone();
        local.updated_at = chrono::Utc::now().timestamp();
        store.upsert_session(&local).unwrap();
        native.updated_at = local.updated_at + 1;
        store.upsert_discovered_session(&native).unwrap();
        let mut stale = local.clone();
        stale.external_id = None;
        stale.updated_at = 1;
        stale.title = "Stale".into();
        store.upsert_session(&stale).unwrap();
        let sessions = store.sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, local.id);
        assert_eq!(sessions[0].title, "Local first prompt");
        assert_eq!(sessions[0].external_id.as_deref(), Some("external"));
        assert_eq!(sessions[0].transcript_path, native.transcript_path);
        assert_eq!(
            store
                .messages(&local.id, 10)
                .unwrap()
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            [
                "Native earlier prompt",
                "Native earlier answer",
                "Local first prompt"
            ]
        );
        assert_eq!(
            store.search("Native earlier answer").unwrap()[0].id,
            local.id
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_imported_cache_is_sanitized_even_when_import_marker_exists() {
        let root = temp_root();
        let session = imported(&root);
        let database = root.join("history.sqlite3");
        let store = Store::open(&database).unwrap();
        store.upsert_session(&session).unwrap();
        store.import_messages_once(&session.id, &[
            StoredMessage { role: "user".into(), text: "Planned action JSON:\n{\"tool\":\"exec_command\"}\nAPPROVAL REQUEST END".into() },
            StoredMessage { role: "assistant".into(), text: r#"{"risk_level":"low","user_authorization":"medium","outcome":"allow","rationale":"Requested"}"#.into() },
            StoredMessage { role: "user".into(), text: "<environment_context>machine context</environment_context>\nFix the **scrolling**.".into() },
        ]).unwrap();
        assert!(store.transcript_imported(&session.id).unwrap());
        drop(store);
        // No source transcript exists. The load must use and sanitize the
        // previously cached snapshot rather than reimporting or exposing it.
        let engine = Engine::new(root.clone(), root.clone(), database);
        assert!(matches!(receive_event(&engine), Event::Ready(_, _, _)));
        engine.load(session, 1);
        let messages = loaded(&engine);
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].blocks,
            presentation::blocks("Fix the **scrolling**.")
        );
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_history_path_recovers_native_index_and_archive_without_full_import() {
        let root = temp_root();
        fs::create_dir_all(root.join(".codex/archived_sessions")).unwrap();
        let native_db = rusqlite::Connection::open(root.join(".codex/state_5.sqlite")).unwrap();
        native_db
            .execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,rollout_path TEXT);")
            .unwrap();
        let mut session = imported(&root);
        let missing = root.join("missing/rollout-2026-10-01-external.jsonl");
        session.transcript_path = Some(missing.clone());
        let database = root.join("history.sqlite3");
        let store = Store::open(&database).unwrap();
        store.upsert_session(&session).unwrap();
        drop(store);
        let engine = Engine::new(root.clone(), root.clone(), database.clone());
        assert!(matches!(receive_event(&engine), Event::Ready(_, _, _)));
        // The source moves after startup; reopening must query metadata again.
        let live = root.join("live.jsonl");
        fs::write(&live, record("user", "Recovered original history")).unwrap();
        native_db
            .execute(
                "INSERT INTO threads VALUES(?1,?2)",
                rusqlite::params!["external", live.to_string_lossy()],
            )
            .unwrap();
        engine.load(session.clone(), 1);
        assert_eq!(
            loaded(&engine)[0].blocks,
            presentation::blocks("Recovered original history")
        );
        let archived = root.join(".codex/archived_sessions/rollout-2026-10-01-external.jsonl");
        fs::rename(&live, &archived).unwrap();
        // Keep the native database's stale location. Exact filename recovery
        // must not need to write to the agent's database or import its messages.
        native_db
            .execute(
                "UPDATE threads SET rollout_path=?1 WHERE id='external'",
                [missing.to_string_lossy().as_ref()],
            )
            .unwrap();
        engine.load(session, 2);
        assert_eq!(
            loaded(&engine)[0].blocks,
            presentation::blocks("Recovered original history")
        );
        let store = Store::open(&database).unwrap();
        let saved = store.session("codex:external").unwrap().unwrap();
        assert_eq!(saved.transcript_path, Some(archived));
        assert_eq!(store.message_count(&saved.id).unwrap(), 0);
        drop(store);
        drop(engine);
        drop(native_db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn indexed_old_compacted_session_loads_without_rewriting_native_data() {
        let root = temp_root();
        fs::create_dir_all(root.join(".codex/sessions/2026/10/01")).unwrap();
        let mut session = imported(&root);
        let original = root.join(".codex/sessions/2026/10/01/rollout-2026-10-01-external.jsonl");
        let compressed = original.with_extension("jsonl.zst");
        let raw = format!(
            "{}{}",
            record("user", "Old compacted question"),
            record("assistant", "Old compacted answer")
        );
        let compressed_bytes = zstd::stream::encode_all(raw.as_bytes(), 1).unwrap();
        fs::write(&compressed, &compressed_bytes).unwrap();
        session.transcript_path = Some(original.clone());
        let native_db = rusqlite::Connection::open(root.join(".codex/state_5.sqlite")).unwrap();
        native_db
            .execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,rollout_path TEXT);")
            .unwrap();
        native_db
            .execute(
                "INSERT INTO threads VALUES(?1,?2)",
                rusqlite::params!["external", original.to_string_lossy()],
            )
            .unwrap();
        let database = root.join("history.sqlite3");
        let store = Store::open(&database).unwrap();
        store.upsert_session(&session).unwrap();
        drop(store);
        let engine = Engine::new(root.clone(), root.clone(), database.clone());
        assert!(matches!(receive_event(&engine), Event::Ready(_, _, _)));
        engine.load(session.clone(), 1);
        let messages = loaded(&engine);
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[0].blocks,
            presentation::blocks("Old compacted question")
        );
        assert_eq!(
            messages[1].blocks,
            presentation::blocks("Old compacted answer")
        );
        assert!(!original.exists());
        assert_eq!(fs::read(&compressed).unwrap(), compressed_bytes);
        let store = Store::open(&database).unwrap();
        assert_eq!(store.message_count(&session.id).unwrap(), 0);
        assert_eq!(
            store.session(&session.id).unwrap().unwrap().transcript_path,
            Some(compressed)
        );
        drop(store);
        drop(engine);
        drop(native_db);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_parent_snapshot_does_not_revert_saved_child_source_when_loading() {
        let root = temp_root();
        let parent = imported(&root);
        fs::write(
            parent.transcript_path.as_ref().unwrap(),
            record("user", "Unrelated parent history"),
        )
        .unwrap();
        let child_path = root.join("child.jsonl");
        fs::write(&child_path, record("user", "Actual continuation history")).unwrap();
        let database = root.join("history.sqlite3");
        let store = Store::open(&database).unwrap();
        store.upsert_session(&parent).unwrap();
        let mut child = parent.clone();
        child.external_id = Some("child".into());
        child.transcript_path = Some(child_path.clone());
        store.bind_runtime_session(&child).unwrap();
        drop(store);
        let engine = Engine::new(root.clone(), root.clone(), database.clone());
        assert!(matches!(receive_event(&engine), Event::Ready(_, _, _)));
        engine.load(parent, 1);
        let messages = loaded(&engine);
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].blocks,
            presentation::blocks("Actual continuation history")
        );
        let store = Store::open(&database).unwrap();
        assert_eq!(
            store.session(&child.id).unwrap().unwrap().transcript_path,
            Some(child_path)
        );
        drop(store);
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_history_request_is_scoped_and_does_not_block_the_next_session() {
        let root = temp_root();
        let missing = imported(&root);
        let mut valid = missing.clone();
        valid.id = "codex:valid".into();
        valid.external_id = Some("valid".into());
        valid.transcript_path = Some(root.join("valid.jsonl"));
        fs::write(
            valid.transcript_path.as_ref().unwrap(),
            record("user", "Keep this session responsive"),
        )
        .unwrap();
        let engine = Engine::new(root.clone(), root.clone(), root.join("history.sqlite3"));
        assert!(matches!(receive_event(&engine), Event::Ready(_, _, _)));
        // An active failure remains scoped and the following request succeeds.
        // Superseded queued requests are covered by the coalescing fixture.
        engine.load(missing.clone(), 11);
        let Event::LoadError(id, generation, error) = receive_event(&engine) else {
            panic!("expected an error scoped to the missing transcript")
        };
        assert_eq!(id, missing.id);
        assert_eq!(generation, 11);
        assert!(!error.is_empty());
        engine.load(valid.clone(), 12);
        let Event::Loaded(id, generation, messages, older, newer) = receive_event(&engine) else {
            panic!("expected the following session to load successfully")
        };
        assert_eq!(id, valid.id);
        assert_eq!(generation, 12);
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].blocks,
            presentation::blocks("Keep this session responsive")
        );
        assert!(!older && !newer);
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_saved_viewport_replays_original_checkpoint_instead_of_encoded_fragments(){
        let root=temp_root();let mut session=imported(&root);session.transcript_path=None;
        let store=Store::open(&root.join("history.sqlite3")).unwrap();store.upsert_session(&session).unwrap();
        let card=crate::chat_interactions::ChatInteraction::Status{title:"Changes".into(),details:"+let source = \"☃ literal \\n\"\n".repeat(18000),status:crate::chat_interactions::InteractionStatus::Completed};
        store.save_message(&session.id,"assistant",&crate::chat_interactions::serialize_interaction(&card)).unwrap();
        let mut state=HistoryState::new(&session,1,&store).unwrap();let first=state.next(&store,&session,200).unwrap();state.cache(&first,state.has_older()).unwrap();let expected=state.next(&store,&session,200).unwrap();state.cache(&expected,state.has_older()).unwrap();
        let saved=root.join("viewport");state.persist_viewport(&saved,&session.id,1,&store).unwrap();
        let name:String=serde_json::from_slice(&fs::read(saved.join("current.json")).unwrap()).unwrap();let folder=saved.join(name);let manifest=folder.join("manifest.json");
        let mut value:serde_json::Value=serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();value["version"]=serde_json::json!(1);value["messages"][0]["text"]=serde_json::json!("STALE ENCODED CARD FRAGMENT");
        // Version-one sparse indexes also lacked card boundaries. Retain their
        // original dense checkpoints and remove only the new disposable footer.
        for asset in value["assets"].as_array_mut().unwrap(){
            let path=folder.join(asset["name"].as_str().unwrap());let bytes=fs::read(&path).unwrap();
            if bytes.len()>=16 && &bytes[bytes.len()-16..bytes.len()-8]==b"KAICARD1"{
                let count=u64::from_le_bytes(bytes[bytes.len()-8..].try_into().unwrap()) as usize;
                fs::OpenOptions::new().write(true).open(&path).unwrap().set_len((bytes.len()-16-count*16) as u64).unwrap();
                asset["stamp"]=serde_json::to_value(crate::history_artifact::FileStamp::read(&path).unwrap()).unwrap();
            }
        }
        fs::write(manifest,serde_json::to_vec(&value).unwrap()).unwrap();drop(state);
        let(restored,actual,_)=HistoryState::restore_viewport(&saved,&session,2,1,&store).unwrap();assert_eq!(actual,expected);assert_eq!(restored.next_page,2);drop(restored);drop(store);fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn durable_viewport_restores_deep_twenty_mb_native_local_and_opencode_without_replay() {
        for kind in ["native", "compressed", "local", "opencode"] {
            let root = temp_root();
            let mut session = imported(&root);
            let store = Store::open(&root.join("history.sqlite3")).unwrap();
            let body = format!("START\n{}END\n", "let value = 123; // 🦀\n".repeat(900_000));
            assert!(body.len() > 20 * 1024 * 1024);
            if kind == "local" {
                session.transcript_path = None;
            } else if kind == "compressed" {
                session.transcript_path = Some(root.join("archived.jsonl.zst"));
            } else if kind == "opencode" {
                session.agent = AgentKind::OpenCode;
                session.id = "opencode:selected".into();
                session.external_id = Some("selected".into());
                session.transcript_path = Some(root.join("opencode.sqlite3"));
            }
            store.upsert_session(&session).unwrap();
            if kind == "local" {
                store.save_message(&session.id, "assistant", &body).unwrap();
            } else if kind == "compressed" {
                fs::write(
                    session.transcript_path.as_ref().unwrap(),
                    zstd::stream::encode_all(record("assistant", &body).as_bytes(), 1).unwrap(),
                )
                .unwrap();
            } else if kind == "native" {
                fs::write(
                    session.transcript_path.as_ref().unwrap(),
                    record("assistant", &body),
                )
                .unwrap();
            } else {
                let writer =
                    rusqlite::Connection::open(session.transcript_path.as_ref().unwrap()).unwrap();
                writer.execute_batch("CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT);CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,data TEXT);CREATE INDEX part_session ON part(session_id);").unwrap();
                writer
                    .execute(
                        "INSERT INTO message VALUES('m','selected',1,'{\"role\":\"assistant\"}')",
                        [],
                    )
                    .unwrap();
                writer
                    .execute(
                        "INSERT INTO part VALUES('p','m','selected',1,?1)",
                        [serde_json::json!({"type":"text","text":body}).to_string()],
                    )
                    .unwrap();
            }
            drop(body);
            let mut original = HistoryState::new(&session, 1, &store).unwrap();
            let mut selected = Vec::new();
            for page in 0..=20 {
                let messages = original.next(&store, &session, 200).unwrap();
                assert!(!messages.is_empty());
                let older = original.has_older();
                original.cache(&messages, older).unwrap();
                if page == 15 {
                    selected = messages;
                }
            }
            let next_newer = original.cached(14, &store, &session).unwrap().0;
            let snapshot = root.join("viewport");
            original
                .persist_viewport(&snapshot, &session.id, 15, &store)
                .unwrap();
            let source_before = crate::history_artifact::FileStamp::read(
                session
                    .transcript_path
                    .as_deref()
                    .unwrap_or(&root.join("history.sqlite3")),
            )
            .unwrap();
            drop(original);
            let (mut restored, messages, older) =
                HistoryState::restore_viewport(&snapshot, &session, 2, 15, &store).unwrap();
            assert_eq!(
                messages, selected,
                "{kind} must restore exactly the selected page"
            );
            assert!(older);
            assert_eq!(restored.next_page, 16);
            if let Some(NativeHistory::Jsonl(cursor)) = &restored.native {
                assert_eq!(
                    cursor.bytes_read, 0,
                    "No body or native index replay at restore"
                );
            }
            if let Some(NativeHistory::OpenCode(cursor)) = &restored.native {
                assert_eq!(
                    cursor.restore_io_counts(),
                    (0, 0),
                    "No SQLite part spool or body replay at restore"
                );
            }
            assert_eq!(
                restored.cached(14, &store, &session).unwrap().0,
                next_newer,
                "Newer navigation must read only its exact saved boundary"
            );
            let older_messages = restored.next(&store, &session, 200).unwrap();
            assert!(!older_messages.is_empty());
            assert!(
                older_messages.iter().map(|m| m.text.len()).sum::<usize>()
                    <= runtime::history::PAGE_BYTES
            );
            restored
                .cache(&older_messages, restored.has_older())
                .unwrap();
            assert!(restored.pages.contains_key(&16));
            let manifest: SavedViewport = read_bounded(
                &snapshot
                    .join(read_bounded::<String>(&snapshot.join("current.json"), 1024).unwrap())
                    .join("manifest.json"),
                VIEWPORT_BYTES,
            )
            .unwrap();
            assert!(serde_json::to_vec(&manifest).unwrap().len() <= VIEWPORT_BYTES);
            assert_eq!(manifest.messages, selected);
            assert!(manifest.assets.len() <= 3);
            assert!(
                source_before.matches(
                    session
                        .transcript_path
                        .as_deref()
                        .unwrap_or(&root.join("history.sqlite3"))
                ),
                "Restoring must not alter source files"
            );
            drop(restored);
            if let Some(asset) = manifest.assets.first() {
                let folder = snapshot
                    .join(read_bounded::<String>(&snapshot.join("current.json"), 1024).unwrap());
                fs::write(folder.join(&asset.name), b"tampered index").unwrap();
                assert!(
                    HistoryState::restore_viewport(&snapshot, &session, 3, 15, &store).is_err(),
                    "Changed immutable artifacts must reject restoration"
                );
            }
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn fresh_bottom_viewport_and_engine_flush_restore_new_local_conversations() {
        let root = temp_root();
        let mut session = imported(&root);
        session.transcript_path = None;
        let engine = Engine::new(root.clone(), root.clone(), root.join("history.sqlite3"));
        assert!(matches!(receive_event(&engine), Event::Ready(..)));
        engine.save(
            session.clone(),
            "user",
            "Please keep my unsent context".into(),
        );
        engine.finish(session.clone(), "Completed response".into());
        assert!(matches!(receive_event(&engine), Event::Parsed(..)));
        engine.save_viewport(session.id.clone(), 1, 0, false);
        engine
            .flush()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        assert!(
            !root.join("viewport/current.json").exists(),
            "Never fabricate a manually-scrolled checkpoint"
        );
        engine.save_viewport(session.id.clone(), 1, 0, true);
        engine
            .flush()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        engine.restore_viewport(session.clone(), 2, 0);
        let Event::ViewportRestored(id, generation, page, messages, older, newer) =
            receive_event(&engine)
        else {
            panic!("Expected restored fresh conversation");
        };
        assert_eq!(id, session.id);
        assert_eq!(generation, 2);
        assert_eq!(page, 0);
        assert_eq!(messages.len(), 2);
        assert!(!older && !newer);
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn viewport_source_changes_and_artifact_tampering_fall_back_without_replay() {
        let root = temp_root();
        let session = imported(&root);
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        store.upsert_session(&session).unwrap();
        fs::write(
            session.transcript_path.as_ref().unwrap(),
            record("user", "saved message"),
        )
        .unwrap();
        let mut state = HistoryState::new(&session, 1, &store).unwrap();
        let page = state.next(&store, &session, 200).unwrap();
        state.cache(&page, state.has_older()).unwrap();
        state
            .persist_viewport(&root.join("viewport"), &session.id, 0, &store)
            .unwrap();
        fs::write(
            session.transcript_path.as_ref().unwrap(),
            record("user", "newer message"),
        )
        .unwrap();
        assert!(
            HistoryState::restore_viewport(&root.join("viewport"), &session, 2, 0, &store).is_err()
        );
        drop(state);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_history_pages_without_an_upfront_import_and_reports_the_real_boundary() {
        let root = temp_root();
        let session = imported(&root);
        let transcript = (0..451)
            .map(|ix| record("user", &format!("Question {ix}")))
            .collect::<String>();
        fs::write(session.transcript_path.as_ref().unwrap(), transcript).unwrap();
        let engine = Engine::new(root.clone(), root.clone(), root.join("history.sqlite3"));
        assert!(matches!(receive_event(&engine), Event::Ready(_, _, _)));
        for (limit, offset, expected, first, older, newer) in [
            (200, 0, 200, 251, true, false),
            (400, 0, 400, 51, true, false),
            (400, 200, 251, 0, false, true),
        ] {
            engine.load_history_window(session.clone(), limit as u64, limit, offset);
            let Event::Loaded(_, generation, messages, has_older, has_newer) =
                receive_event(&engine)
            else {
                panic!("expected paged history")
            };
            assert_eq!(generation, limit as u64);
            assert_eq!(messages.len(), expected);
            assert_eq!(
                messages[0].blocks,
                presentation::blocks(&format!("Question {first}"))
            );
            assert_eq!(has_older, older);
            assert_eq!(has_newer, newer);
        }
        engine.load_project(root.clone(), 55);
        let Event::ProjectSessions(path, generation, sessions) = receive_event(&engine) else {
            panic!("expected project sessions")
        };
        assert_eq!(path, root);
        assert_eq!(generation, 55);
        assert_eq!(sessions.len(), 1);
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn lazy_pages_are_cached_deterministically_and_scoped_to_a_snapshot() {
        use std::io::Write;
        let root = temp_root();
        let session = imported(&root);
        let body = "x".repeat(8192);
        let transcript = (0..100)
            .map(|ix| record("user", &format!("Question {ix}: {body}")))
            .collect::<String>();
        fs::write(session.transcript_path.as_ref().unwrap(), transcript).unwrap();
        let engine = Engine::new(root.clone(), root.clone(), root.join("history.sqlite3"));
        receive_event(&engine);
        engine.load(session.clone(), 71);
        let Event::Loaded(_, _, first, mut older, _) = receive_event(&engine) else {
            panic!("expected first page");
        };
        assert!(first.len() < 100);
        let first_blocks = first.iter().map(|m| m.blocks.clone()).collect::<Vec<_>>();
        let mut count = first.len();
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        assert_eq!(store.message_count(&session.id).unwrap(), 0);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(session.transcript_path.as_ref().unwrap())
            .unwrap();
        file.write_all(record("user", "Appended after the snapshot").as_bytes())
            .unwrap();
        drop(file);
        engine.load_older(session.clone(), 70, 200);
        engine
            .flush()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        while let Ok(event) = engine.events.try_recv() {
            assert!(
                matches!(event, Event::HistoryRange(..)),
                "Superseded reads must not emit stale pages or errors"
            );
        }
        while older {
            engine.load_older(session.clone(), 71, 200);
            let Event::OlderLoaded(_, 71, page, has_older) = receive_event(&engine) else {
                panic!("expected older page");
            };
            count += page.len();
            older = has_older;
        }
        assert_eq!(count, 100);
        engine.load_history_page(session.clone(), 71, 0);
        let Event::HistoryPage(_, 71, 0, restored, _) = receive_event(&engine) else {
            panic!("expected cached page");
        };
        assert_eq!(
            restored
                .iter()
                .map(|m| m.blocks.clone())
                .collect::<Vec<_>>(),
            first_blocks
        );
        drop(store);
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn identical_earlier_native_turns_survive_local_native_seam_deduplication() {
        let root = temp_root();
        let session = imported(&root);
        let pair = format!(
            "{}{}",
            record("user", "Repeat the request"),
            record("assistant", "Same answer")
        );
        fs::write(
            session.transcript_path.as_ref().unwrap(),
            format!("{pair}{pair}"),
        )
        .unwrap();
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        store.upsert_session(&session).unwrap();
        store
            .save_message(&session.id, "user", "Repeat the request")
            .unwrap();
        store
            .save_message(&session.id, "assistant", "Same answer")
            .unwrap();
        drop(store);
        let engine = Engine::new(root.clone(), root.clone(), root.join("history.sqlite3"));
        receive_event(&engine);
        engine.load(session, 1);
        assert_eq!(loaded(&engine).len(), 4);
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]fn returning_from_rail_seek_resumes_the_original_older_history_cursor(){
        let root=temp_root();let session=imported(&root);let mut source=String::new();for n in 0..1200{source.push_str(&record("user",&format!("Prompt {n}")));source.push_str(&record("assistant",&format!("Final {n}")));}fs::write(session.transcript_path.as_ref().unwrap(),source).unwrap();
        let store=Store::open(&root.join("history.sqlite3")).unwrap();store.upsert_session(&session).unwrap();drop(store);let engine=Engine::new(root.clone(),root.clone(),root.join("history.sqlite3"));receive_event(&engine);engine.load(session.clone(),1);let tail=loaded(&engine);assert!(tail.iter().any(|m|m.blocks.iter().any(|b|matches!(b,presentation::MessageBlock::Paragraph(text) if text=="Final 1199"))));
        let epoch=uuid::Uuid::new_v4();engine.pause_live_history(session.id.clone(),1,epoch);engine.seek_history(session.clone(),2,0.);assert!(matches!(receive_event(&engine),Event::HistorySeeked(_,2,_,_,_,_,_)));
        fs::OpenOptions::new().append(true).open(session.transcript_path.as_ref().unwrap()).unwrap().write_all(record("user","The current live prompt").as_bytes()).unwrap();
        engine.resume_live_history(session.id.clone(),1,3,epoch);engine.load_older(session,3,100);let Event::OlderLoaded(_,3,older,_) = receive_event(&engine)else{panic!("The restored cursor expired")};assert!(older.iter().any(|m|m.blocks.iter().any(|b|matches!(b,presentation::MessageBlock::Paragraph(text) if text=="Final 1099"))));assert!(!older.iter().any(|m|m.blocks.iter().any(|b|matches!(b,presentation::MessageBlock::Paragraph(text) if text=="Final 1199"))));
        drop(engine);fs::remove_dir_all(root).unwrap();
    }
    #[test]fn final_phase_metadata_does_not_duplicate_a_cached_prompt_answer_pair(){
        for repeated in [1,2]{
            let root=temp_root();let session=imported(&root);
            let pair=format!("{}{}\n",record("user","Repeat the request"),serde_json::json!({"type":"response_item","payload":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"Same answer"}]}}));
            fs::write(session.transcript_path.as_ref().unwrap(),pair.repeat(repeated)).unwrap();
            let store=Store::open(&root.join("history.sqlite3")).unwrap();store.upsert_session(&session).unwrap();store.save_message(&session.id,"user","Repeat the request").unwrap();store.save_message(&session.id,"assistant","Same answer").unwrap();
            let mut state=HistoryState::new(&session,1,&store).unwrap();let mut all=Vec::new();while state.has_older(){let mut earlier=state.next(&store,&session,100).unwrap();earlier.append(&mut all);all=earlier;}
            assert_eq!(all.iter().filter(|m|m.text=="Repeat the request").count(),repeated);assert_eq!(all.iter().filter(|m|m.text=="Same answer").count(),repeated);
            drop(state);drop(store);fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn checkpoint_marked_prompts_merge_after_restart_without_losing_repeat_associations() {
        let root = temp_root();
        let session = imported(&root);
        let pair = format!(
            "{}{}",
            record("user", "Repeat the request"),
            record("assistant", "Same answer")
        );
        fs::write(session.transcript_path.as_ref().unwrap(), pair.repeat(3)).unwrap();
        let ids = [uuid::Uuid::new_v4(), uuid::Uuid::new_v4()];
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        store.upsert_session(&session).unwrap();
        for id in ids {
            store
                .save_message(
                    &session.id,
                    "user",
                    &format!("Repeat the request\n\n<!--code-checkpoint:{id}-->"),
                )
                .unwrap();
            store
                .save_message(&session.id, "assistant", "Same answer")
                .unwrap();
        }
        drop(store);
        // Reopen the actual state actor twice: local SQLite rows retain the
        // Undo association while the native JSONL contains the plain prompt.
        for generation in 1..=2 {
            let engine = Engine::new(
                root.join("home"),
                root.clone(),
                root.join("history.sqlite3"),
            );
            receive_event(&engine);
            engine.load(session.clone(), generation);
            let messages = loaded(&engine);
            assert_eq!(
                messages.len(),
                6,
                "only the two completed local pairs overlap"
            );
            let associations: Vec<_> = messages
                .iter()
                .enumerate()
                .flat_map(|(index, message)| {
                    message.blocks.iter().filter_map(move |block| match block {
                        MessageBlock::Activity { label, details } if label == "Prompt changes" => {
                            Some((index, details.clone()))
                        }
                        _ => None,
                    })
                })
                .collect();
            assert_eq!(
                associations,
                vec![(2, ids[0].to_string()), (4, ids[1].to_string())]
            );
            drop(engine);
        }
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        let saved = store.messages(&session.id, 100).unwrap();
        assert!(
            saved[0]
                .text
                .ends_with(&format!("<!--code-checkpoint:{}-->", ids[0]))
        );
        assert!(
            saved[2]
                .text
                .ends_with(&format!("<!--code-checkpoint:{}-->", ids[1]))
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn checkpoint_suffix_only_affects_valid_user_fingerprints() {
        let root = temp_root();
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        let session = imported(&root);
        store.upsert_session(&session).unwrap();
        let id = uuid::Uuid::new_v4();
        let valid = format!("🦀 request\n\n<!--code-checkpoint:{id}-->");
        for (role, text, stripped) in [
            ("user", valid.clone(), true),
            ("assistant", valid.clone(), false),
            ("user", format!("{valid}\n"), false),
            (
                "user",
                valid.replace(&id.to_string(), "not-a-checkpoint"),
                false,
            ),
            ("user", valid.replace("\n\n<!--", "\n<!--"), false),
            ("user", format!("{valid}literal"), false),
        ] {
            store.save_message(&session.id, role, &text).unwrap();
            let (_, fingerprint) = store
                .local_fingerprint(&session.id, i64::MAX)
                .unwrap()
                .unwrap();
            let expected = if stripped { "🦀 request" } else { &text };
            assert_eq!(fingerprint.bytes, expected.len() as u64);
            assert_eq!(fingerprint.prefix, expected);
            assert_eq!(fingerprint.tail, expected);
        }
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_input_association_preserves_readable_context_team_and_handoff_prompts_on_restart() {
        for kind in ["context", "team", "handoff"] {
            let root = temp_root();
            let session = imported(&root);
            let task = "Review the existing changes";
            let reference = crate::context_refs::Reference {
                id: "other-chat".into(),
                title: "Related findings".into(),
                project: "/other/project".into(),
                provider: "Claude".into(),
                excerpt: "Quoted findings with <tags> and literal JSON: {\"state\":\"done\"}"
                    .into(),
                older_omitted: true,
            };
            let context = crate::context_refs::with_context(task, &[reference]).unwrap();
            let actual = match kind {
                "context" => context,
                "team" => format!(
                    "You are a member of a user-created coding team. Your assigned task follows. Other team members may be investigating this project; stay within your assigned scope and the current native permissions. Report actual findings, checks, changed files and blockers.\n\n{context}"
                ),
                "handoff" => crate::handoff::Packet::build(
                    &session,
                    None,
                    Some("Preserved objective"),
                    vec![],
                    false,
                    false,
                )
                .prompt(&context),
                _ => unreachable!(),
            };
            let pair = format!(
                "{}{}",
                record("user", &actual),
                record("assistant", "Result")
            );
            fs::write(session.transcript_path.as_ref().unwrap(), pair.repeat(3)).unwrap();
            let database = root.join("history.sqlite3");
            let engine = Engine::new(root.join("home"), root.clone(), database.clone());
            receive_event(&engine);
            let ids = [uuid::Uuid::new_v4(), uuid::Uuid::new_v4()];
            for id in ids {
                engine.save_prompt(session.clone(), format!("{task}\n\nAttached chat: Related findings · Claude · /other/project (bounded excerpt)\n\n<!--code-checkpoint:{id}-->"), crate::store::NativeInput::from_prompt(&actual));
                engine.save(session.clone(), "assistant", "Result".into());
            }
            engine
                .flush()
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            drop(engine);
            let engine = Engine::new(root.join("home"), root.clone(), database);
            receive_event(&engine);
            engine.load(session, 1);
            let messages = loaded(&engine);
            assert_eq!(
                messages.len(),
                6,
                "exact transformed input association: {kind}"
            );
            for (at, id) in [(2, ids[0]), (4, ids[1])] {
                assert!(messages[at].blocks.iter().any(|block| matches!(block,
                    MessageBlock::Activity { label, details } if label == "Prompt changes" && details == &id.to_string())));
                let visible = format!("{:?}", messages[at].blocks);
                assert!(visible.contains(task) && visible.contains("Attached chat:"));
                assert!(
                    !visible.contains("coding team") && !visible.contains("Attached chats (JSON)")
                );
            }
            drop(engine);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn native_input_digest_streams_giant_transformed_prompts_and_rejects_changed_middles() {
        for changed_middle in [false, true] {
            let root = temp_root();
            let session = imported(&root);
            let actual = format!(
                "{}{}{}",
                "🦀 Prefix ".repeat(100),
                "A".repeat(2_000_000),
                " tail 界".repeat(100)
            );
            let native = if changed_middle {
                actual.replacen(&"A".repeat(1000), &"B".repeat(1000), 1)
            } else {
                actual.clone()
            };
            let local = format!(
                "Readable task\n\n<!--code-checkpoint:{}-->",
                uuid::Uuid::new_v4()
            );
            fs::write(
                session.transcript_path.as_ref().unwrap(),
                record("user", &native),
            )
            .unwrap();
            let store = Store::open(&root.join("history.sqlite3")).unwrap();
            store.upsert_session(&session).unwrap();
            store
                .save_prompt(
                    &session.id,
                    &local,
                    crate::store::NativeInput::from_prompt(&actual).as_ref(),
                )
                .unwrap();
            let mut state = HistoryState::new(&session, 1, &store).unwrap();
            let mut pages = Vec::new();
            while state.has_older() {
                let page = state.next(&store, &session, 200).unwrap();
                assert!(
                    page.iter().map(|m| m.text.len()).sum::<usize>()
                        <= runtime::history::PAGE_BYTES
                );
                pages.push(page);
            }
            pages.reverse();
            let rendered: String = pages.into_iter().flatten().map(|m| m.text).collect();
            assert_eq!(
                rendered.len(),
                local.len() + if changed_middle { native.len() } else { 0 }
            );
            assert!(rendered.ends_with(&local));
            drop(state);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn giant_checkpoint_prompt_seam_is_bounded_and_compares_the_entire_body() {
        for changed_middle in [false, true] {
            let root = temp_root();
            let session = imported(&root);
            let body = format!(
                "{}{}{}",
                "🦀 prefix ".repeat(100),
                "A".repeat(2_000_000),
                " tail 界".repeat(100)
            );
            let native = if changed_middle {
                body.replacen(&"A".repeat(1000), &"B".repeat(1000), 1)
            } else {
                body.clone()
            };
            let local = format!("{body}\n\n<!--code-checkpoint:{}-->", uuid::Uuid::new_v4());
            fs::write(
                session.transcript_path.as_ref().unwrap(),
                record("user", &native),
            )
            .unwrap();
            let store = Store::open(&root.join("history.sqlite3")).unwrap();
            store.upsert_session(&session).unwrap();
            store.save_message(&session.id, "user", &local).unwrap();
            let (_, fingerprint) = store
                .local_fingerprint(&session.id, i64::MAX)
                .unwrap()
                .unwrap();
            assert_eq!(fingerprint.bytes, body.len() as u64);
            assert!(fingerprint.prefix.len() <= 256 && fingerprint.tail.len() <= 256);
            let mut state = HistoryState::new(&session, 1, &store).unwrap();
            let mut pages = Vec::new();
            while state.has_older() {
                let page = state.next(&store, &session, 200).unwrap();
                assert!(
                    page.iter().map(|message| message.text.len()).sum::<usize>()
                        <= runtime::history::PAGE_BYTES
                );
                assert!(page.iter().all(|message| message.text.len() <= 64 * 1024));
                pages.push(page);
            }
            pages.reverse();
            let rendered: String = pages
                .into_iter()
                .flatten()
                .map(|message| message.text)
                .collect();
            assert_eq!(
                rendered.len(),
                local.len() + if changed_middle { native.len() } else { 0 }
            );
            assert!(rendered.ends_with(&local));
            if changed_middle {
                assert!(rendered.starts_with(&native));
            }
            drop(state);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn preferred_project_is_validated_on_the_worker_and_supplied_with_ready() {
        let root = temp_root();
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        let mut session = imported(&root);
        session.transcript_path = None;
        session.project_path = root.clone();
        store.upsert_session(&session).unwrap();
        drop(store);
        let engine = Engine::new(root.clone(), root.clone(), root.join("history.sqlite3"));
        let Event::Ready(_, _, preferred) = receive_event(&engine) else {
            panic!("expected ready");
        };
        assert_eq!(preferred, Some(root.clone()));
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn archived_summary_lookup_recovers_old_ids_and_supersedes_queued_pages() {
        let root = temp_root();
        let database = root.join("history.sqlite3");
        let store = Store::open(&database).unwrap();
        let mut old = imported(&root);
        old.id = format!("old-archive:{}", "界".repeat(150));
        old.updated_at = 0;
        old.external_id = None;
        old.transcript_path = Some(root.join("missing-transcript.jsonl"));
        let mut sessions = vec![old.clone()];
        sessions.extend((1..6002).map(|index| {
            let mut chat = old.clone();
            chat.id = format!("recent-{index}");
            chat.updated_at = index;
            chat.transcript_path = None;
            chat
        }));
        store.upsert_sessions(&sessions).unwrap();
        drop(store);
        let engine = Engine::new(root.clone(), root.clone(), database);
        let Event::Ready(_, recent, _) = receive_event(&engine) else {
            panic!("ready");
        };
        assert!(!recent.iter().any(|chat| chat.id == old.id));
        // Only the latest queued page may produce UI metadata. A stale page
        // with an invalid oversized query must not emit even an error event.
        engine.latest.archived.store(100, Ordering::Release);
        for generation in 1..100 {
            engine
                .tx
                .send(Command::LoadArchived(
                    vec![old.id.clone(); ARCHIVED_PAGE_SIZE + 1],
                    generation,
                ))
                .unwrap();
        }
        engine.load_archived(vec![old.id.clone(), "not-saved".into()], 100);
        let Event::ArchivedLoaded(100, found) = receive_event(&engine) else {
            panic!("only current archive metadata should be delivered");
        };
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, old.id);
        assert_eq!(found[0].transcript_path, old.transcript_path);
        engine.load_archived(vec![old.id.clone(); ARCHIVED_PAGE_SIZE + 1], 101);
        assert!(matches!(
            receive_event(&engine),
            Event::ArchivedError(101, _)
        ));
        // The failed metadata page must not poison subsequent bounded reads.
        engine.load_archived(vec![old.id.clone()], 102);
        assert!(
            matches!(receive_event(&engine),Event::ArchivedLoaded(102,sessions) if sessions.len()==1 && sessions[0].id==old.id)
        );
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn superseded_queued_reads_are_skipped_while_all_writes_are_preserved() {
        let root = temp_root();
        let engine = Engine::new(root.clone(), root.clone(), root.join("history.sqlite3"));
        receive_event(&engine);
        let mut session = imported(&root);
        session.transcript_path = None;
        engine.latest.history.store(100, Ordering::Release);
        engine.latest.search.store(100, Ordering::Release);
        engine.latest.project.store(100, Ordering::Release);
        let mut stale = session.clone();
        stale.id = "superseded".into();
        stale.transcript_path = Some(root.join("missing.jsonl"));
        for generation in 1..100 {
            engine
                .tx
                .send(Command::Load(stale.clone(), generation, 200, 0))
                .unwrap();
            engine
                .tx
                .send(Command::Search("stale".into(), generation))
                .unwrap();
            engine
                .tx
                .send(Command::LoadProject(root.join("missing"), generation))
                .unwrap();
        }
        engine.save(session.clone(), "user", "Saved despite stale reads".into());
        engine.finish(session.clone(), "Finished despite stale reads".into());
        engine.load(session.clone(), 100);
        assert!(matches!(receive_event(&engine), Event::Parsed(_, _)));
        let Event::Loaded(_, 100, messages, _, _) = receive_event(&engine) else {
            panic!("only the newest queued load should run");
        };
        assert_eq!(messages.len(), 2);
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        assert_eq!(store.messages(&session.id, 10).unwrap().len(), 2);
        assert!(
            store
                .sessions()
                .unwrap()
                .iter()
                .all(|s| s.id != "superseded")
        );
        while let Ok(event) = engine.events.try_recv() {
            assert!(
                matches!(event, Event::HistoryRange(..)),
                "only range metadata may follow the last page"
            );
        }
        drop(store);
        drop(engine);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn giant_same_length_and_bookends_with_changed_middle_are_not_deduplicated() {
        let root = temp_root();
        let session = imported(&root);
        let local = format!(
            "{}{}{}",
            "Common prefix ".repeat(1000),
            "A".repeat(200_000),
            "Common tail ".repeat(1000)
        );
        let native = local.replacen(&"A".repeat(1000), &"B".repeat(1000), 1);
        assert_eq!(local.len(), native.len());
        fs::write(
            session.transcript_path.as_ref().unwrap(),
            record("assistant", &native),
        )
        .unwrap();
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        store.upsert_session(&session).unwrap();
        store
            .save_message(&session.id, "assistant", &local)
            .unwrap();
        let mut state = HistoryState::new(&session, 1, &store).unwrap();
        let mut pages = Vec::new();
        while state.has_older() {
            pages.push(state.next(&store, &session, 200).unwrap());
        }
        pages.reverse();
        let rendered: String = pages
            .into_iter()
            .flatten()
            .map(|message| message.text)
            .collect();
        assert!(rendered == format!("{native}{local}"));
        drop(state);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn giant_fenced_code_chunks_are_formatted_and_native_local_seam_is_not_duplicated() {
        let root = temp_root();
        let session = imported(&root);
        let code = format!("```rust\n{}\n```", "let value = 42;\n".repeat(80_000));
        fs::write(
            session.transcript_path.as_ref().unwrap(),
            record("assistant", &code),
        )
        .unwrap();
        let store = Store::open(&root.join("history.sqlite3")).unwrap();
        store.upsert_session(&session).unwrap();
        store.save_message(&session.id, "assistant", &code).unwrap();
        let mut state = HistoryState::new(&session, 22, &store).unwrap();
        let mut total = 0;
        while state.has_older() {
            let messages = state.next(&store, &session, 200).unwrap();
            total += messages.iter().map(|m| m.text.len()).sum::<usize>();
            for parsed in parse_messages(messages) {
                assert!(
                    parsed
                        .blocks
                        .iter()
                        .any(|b| matches!(b,MessageBlock::Code{language,..} if language=="rust")),
                    "every code continuation retains fence context"
                );
            }
        }
        assert_eq!(total, code.len());
        drop(state);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_only_giant_code_preserves_fences_and_literal_harness_tags() {
        for fence in ["```", "~~~"] {
            let root = temp_root();
            let session = imported(&root);
            let code = format!(
                "{fence}rust\n{}\n{fence}",
                "let x = \"<environment_context>literal</environment_context>\";\n".repeat(6000)
            );
            fs::write(
                session.transcript_path.as_ref().unwrap(),
                record("assistant", &code),
            )
            .unwrap();
            let store = Store::open(&root.join("history.sqlite3")).unwrap();
            store.upsert_session(&session).unwrap();
            let mut state = HistoryState::new(&session, 4, &store).unwrap();
            let mut total = 0;
            while state.has_older() {
                let messages = state.next(&store, &session, 200).unwrap();
                total += messages.iter().map(|m| m.text.len()).sum::<usize>();
                for parsed in parse_messages(messages) {
                    assert!(
                        parsed.blocks.iter().any(
                            |b| matches!(b,MessageBlock::Code{language,..} if language=="rust")
                        )
                    );
                }
            }
            assert_eq!(total, code.len());
            drop(state);
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    #[ignore = "read-only audit of explicitly selected installed Code/Codex history"]
    fn installed_old_sessions_resolve_and_selected_archives_load() {
        let home =
            PathBuf::from(std::env::var_os("CODE_HISTORY_AUDIT_HOME").expect("select native home"));
        let database = PathBuf::from(
            std::env::var_os("CODE_HISTORY_AUDIT_DB").expect("select Code history database"),
        );
        let source = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut query = source.prepare("SELECT id,external_id,transcript_path,project_path FROM sessions WHERE agent='codex' AND transcript_path IS NOT NULL").unwrap();
        let sessions = query
            .query_map([], |row| {
                Ok(SessionSummary {
                    id: row.get(0)?,
                    title: "Read-only history audit".into(),
                    agent: discovery::AgentKind::Codex,
                    external_id: row.get(1)?,
                    transcript_path: row.get::<_, Option<String>>(2)?.map(PathBuf::from),
                    project_path: PathBuf::from(row.get::<_, String>(3)?),
                    execution_path: None,
                    updated_at: 0,
                })
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let missing: Vec<_> = sessions
            .into_iter()
            .filter(|s| !s.transcript_path.as_ref().unwrap().is_file())
            .collect();
        assert!(!missing.is_empty(), "Need genuinely compacted old sessions");
        let mut recovered = Vec::new();
        for mut session in missing {
            let path = discovery::recover_codex_transcript(
                &home,
                session.external_id.as_deref().unwrap(),
                session.transcript_path.as_deref(),
            );
            assert!(path.is_some(), "Unresolved old session {}", session.id);
            session.transcript_path = path;
            recovered.push(session);
        }
        println!(
            "HISTORY_AUDIT_RESOLVED {} old sessions using read-only metadata",
            recovered.len()
        );
        recovered.sort_by_key(|s| {
            fs::metadata(s.transcript_path.as_ref().unwrap())
                .unwrap()
                .len()
        });
        let root = temp_root();
        let store = Store::open(&root.join("audit.sqlite")).unwrap();
        // Ignore genuine metadata-only helper threads using indexed native
        // metadata, rather than mistaking their intentional emptiness for loss.
        let native = rusqlite::Connection::open_with_flags(
            home.join(".codex/thread_history_1.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut loaded_count = 0;
        for session in recovered
            .into_iter()
            .filter(|session| {
                native
                    .query_row(
                        "SELECT 1 FROM thread_items WHERE thread_id=?1 LIMIT 1",
                        [session.external_id.as_deref().unwrap()],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap_or(0)
                    != 0
            })
            .take(3)
        {
            let path = session.transcript_path.as_ref().unwrap();
            let stamp = crate::history_artifact::FileStamp::read(path).unwrap();
            let mut state = HistoryState::new(&session, 1, &store).unwrap();
            let page = state.next(&store, &session, 40).unwrap();
            assert!(
                !page.is_empty(),
                "No visible history in selected archive {}",
                session.id
            );
            assert!(
                page.iter().map(|m| m.text.len()).sum::<usize>() <= runtime::history::PAGE_BYTES
            );
            loaded_count += 1;
            println!(
                "HISTORY_AUDIT_LOAD_OK rows={} compressed_bytes={}",
                page.len(),
                fs::metadata(path).unwrap().len()
            );
            assert!(stamp.matches(path));
        }
        assert_eq!(
            loaded_count, 3,
            "Actually load three existing conversations"
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn same_chat_handoffs_keep_lazy_history_provenance_native_ids_and_viewport_after_restart() {
        let root = temp_root();
        let database = root.join("continuous.sqlite");
        let mut source = imported(&root);
        let mut raw = String::new();
        for index in 0..650 {
            raw.push_str(&record("assistant", &format!("Native history {index}")));
        }
        raw.push_str(&record("user", "Code original follow-up"));
        raw.push_str(&record("assistant", "Code original response"));
        fs::write(source.transcript_path.as_ref().unwrap(), &raw).unwrap();
        let store = Store::open(&database).unwrap();
        store.upsert_session(&source).unwrap();
        store
            .save_message(&source.id, "user", "Code original follow-up")
            .unwrap();
        store
            .save_message(&source.id, "assistant", "Code original response")
            .unwrap();
        let first = crate::handoff::Packet::build(
            &source,
            None,
            Some("Original task"),
            Vec::new(),
            true,
            false,
        );
        let mut destination = source.clone();
        destination.agent = AgentKind::Claude;
        destination.external_id = None;
        destination.transcript_path = None;
        store
            .switch_agent(&destination, &first, Some("claude-fixture-model"))
            .unwrap();
        destination.external_id = Some("claude-native".into());
        destination.transcript_path = Some(root.join("claude-native.jsonl"));
        // This native destination includes the portable context wrapper. It must
        // never replace the original source or duplicate the unified local turns.
        fs::write(
            destination.transcript_path.as_ref().unwrap(),
            "not used as chat history",
        )
        .unwrap();
        store.bind_runtime_session(&destination).unwrap();
        for index in 0..230 {
            store
                .save_message(&source.id, "assistant", &format!("Claude response {index}"))
                .unwrap();
        }
        let second = crate::handoff::Packet::build(
            &destination,
            Some(&first),
            None,
            Vec::new(),
            true,
            false,
        );
        assert_eq!(second.sources.len(), 2);
        let mut final_session = destination.clone();
        final_session.agent = AgentKind::OpenCode;
        final_session.external_id = None;
        final_session.transcript_path = None;
        store
            .switch_agent(&final_session, &second, Some("open-fixture-model"))
            .unwrap();
        final_session.external_id = Some("opencode-native".into());
        store.bind_runtime_session(&final_session).unwrap();
        store
            .save_message(&source.id, "assistant", "OpenCode response")
            .unwrap();
        // Rediscovering old native sessions must not produce sidebar duplicates
        // or revert the current adapter, resume ID, or model.
        store.upsert_discovered_session(&source).unwrap();
        store.upsert_discovered_session(&destination).unwrap();
        assert_eq!(store.sessions().unwrap().len(), 1);
        assert_eq!(
            store.session_model(&source.id).unwrap().as_deref(),
            Some("open-fixture-model")
        );
        drop(store);
        let store = Store::open(&database).unwrap();
        source = store.session(&source.id).unwrap().unwrap();
        assert_eq!(source.agent, AgentKind::OpenCode);
        assert_eq!(source.external_id.as_deref(), Some("opencode-native"));
        let mut state = HistoryState::new(&source, 1, &store).unwrap();
        let tail = state.next(&store, &source, 100).unwrap();
        state.cache(&tail, state.has_older()).unwrap();
        let viewport = root.join("viewport");
        state
            .persist_viewport(&viewport, &source.id, 0, &store)
            .unwrap();
        drop(state);
        let (mut state, restored, has_older) =
            HistoryState::restore_viewport(&viewport, &source, 2, 0, &store).unwrap();
        assert!(has_older);
        assert_eq!(restored.last().unwrap().role, "assistant:opencode");
        let mut all = restored;
        while state.has_older() {
            let mut earlier = state.next(&store, &source, 100).unwrap();
            earlier.extend(all);
            all = earlier;
        }
        assert_eq!(all.len(), 650 + 2 + 230 + 1);
        assert_eq!(all[0].text, "Native history 0");
        assert_eq!(all[0].role, "assistant:codex");
        assert_eq!(all[651].text, "Code original response");
        assert_eq!(all[651].role, "assistant:codex");
        assert_eq!(all[652].role, "assistant:claude");
        assert_eq!(all.last().unwrap().text, "OpenCode response");
        assert_eq!(
            fs::read_to_string(
                store
                    .conversation_baseline(&source.id)
                    .unwrap()
                    .unwrap()
                    .0
                    .transcript_path
                    .unwrap()
            )
            .unwrap(),
            raw
        );
        drop(state);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_same_chat_handoff_rolls_back_binding_history_model_and_usage() {
        let root = temp_root();
        let store = Store::open(&root.join("rollback.sqlite")).unwrap();
        let source = imported(&root);
        store.upsert_session(&source).unwrap();
        store
            .save_message(&source.id, "assistant", "Keep this work")
            .unwrap();
        store
            .set_session_model(&source.id, Some("source-model"))
            .unwrap();
        store
            .set_session_usage(
                &source.id,
                &crate::usage::Usage {
                    input: Some(42),
                    ..Default::default()
                },
            )
            .unwrap();
        let mut packet = crate::handoff::Packet::build(
            &source,
            None,
            Some("Original task"),
            Vec::new(),
            true,
            false,
        );
        packet.objective = "x".repeat(4097);
        let mut destination = source.clone();
        destination.agent = AgentKind::Claude;
        destination.external_id = None;
        destination.transcript_path = None;
        assert!(
            store
                .switch_agent(&destination, &packet, Some("destination-model"))
                .is_err()
        );
        let restored = store.session(&source.id).unwrap().unwrap();
        assert_eq!(restored.agent, AgentKind::Codex);
        assert_eq!(restored.external_id, source.external_id);
        assert_eq!(
            store.session_model(&source.id).unwrap().as_deref(),
            Some("source-model")
        );
        assert_eq!(
            store.session_usage(&source.id).unwrap().unwrap().input,
            Some(42)
        );
        assert!(store.conversation_baseline(&source.id).unwrap().is_none());
        assert_eq!(store.message_count(&source.id).unwrap(), 1);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn evicted_source_pages_rehydrate_after_same_chat_agent_switch_and_new_turns() {
        let root = temp_root();
        let store = Store::open(&root.join("evicted-handoff.sqlite")).unwrap();
        let source = imported(&root);
        fs::write(
            source.transcript_path.as_ref().unwrap(),
            format!(
                "{}{}",
                record("assistant", "Earlier source answer"),
                record("assistant", "Latest source answer")
            ),
        )
        .unwrap();
        store.upsert_session(&source).unwrap();
        let mut state = HistoryState::new(&source, 1, &store).unwrap();
        let messages = state.next(&store, &source, 1).unwrap();
        state.cache(&messages, state.has_older()).unwrap();
        state.pages.remove(&0);
        fs::remove_file(state.cache_dir.join("0.json")).unwrap();
        let packet =
            crate::handoff::Packet::build(&source, None, Some("Task"), Vec::new(), true, false);
        let mut destination = source.clone();
        destination.agent = AgentKind::Claude;
        destination.external_id = None;
        destination.transcript_path = None;
        store.switch_agent(&destination, &packet, None).unwrap();
        store
            .save_message(&source.id, "assistant", "New agent response")
            .unwrap();
        let (rehydrated, _) = state.cached(0, &store, &destination).unwrap();
        assert_eq!(rehydrated.len(), 1);
        assert_eq!(rehydrated[0].text, "Latest source answer");
        assert_eq!(rehydrated[0].role, "assistant:codex");
        let older = state.next(&store, &destination, 1).unwrap();
        assert_eq!(older[0].text, "Earlier source answer");
        assert_eq!(older[0].role, "assistant:codex");
        drop(state);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

//! Bounded, durable team plans. Native run handles remain the authority for
//! execution; restart holds unfinished tasks rather than resubmitting prompts.
use crate::discovery::AgentKind;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

pub const MAX_TEAMS: usize = 8;
pub const MAX_MEMBERS: usize = 4;
pub const MAX_TASK_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Starting,
    Working,
    Complete,
    Failed,
    Stopped,
    Held,
}
impl Status {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Starting => "Connecting",
            Self::Working => "Working",
            Self::Complete => "Completed",
            Self::Failed => "Needs attention",
            Self::Stopped => "Stopped",
            Self::Held => "Held · resume explicitly",
        }
    }
    pub fn active(&self) -> bool {
        matches!(self, Self::Starting | Self::Working)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    pub session: String,
    pub agent: AgentKind,
    pub model: Option<String>,
    pub task: String,
    pub status: Status,
    #[serde(default)]
    pub attempted: bool,
    #[serde(skip)]
    pub cancel: Arc<AtomicBool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub title: String,
    pub project: PathBuf,
    pub allow_edits: bool,
    pub created_at: i64,
    pub members: Vec<Member>,
    #[serde(default)]
    pub context: Vec<crate::context_refs::Reference>,
}
impl Team {
    pub fn valid(&self) -> bool {
        uuid::Uuid::parse_str(&self.id).is_ok()
            && self.title.len() <= 256
            && self.project.is_absolute()
            && self.project.as_os_str().len() <= 4096
            && (2..=MAX_MEMBERS).contains(&self.members.len())
            && self.context.len() <= crate::context_refs::MAX_REFERENCES
            && self
                .context
                .iter()
                .all(crate::context_refs::Reference::valid)
            && self.members.iter().all(|m| {
                uuid::Uuid::parse_str(&m.session).is_ok()
                    && !m.task.trim().is_empty()
                    && m.task.len() <= MAX_TASK_BYTES
                    && m.model.as_ref().is_none_or(|s| s.len() <= 256)
            })
            && self.members.iter().enumerate().all(|(i, m)| {
                !self.members[..i]
                    .iter()
                    .any(|other| other.session == m.session)
            })
    }
    pub fn hold_on_restart(&mut self) {
        for member in &mut self.members {
            if member.status.active() || member.status == Status::Queued {
                member.status = Status::Held;
            }
        }
    }
}
pub fn overlapping(a: &std::path::Path, b: &std::path::Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_never_replays_work_and_terminal_states_stay_terminal() {
        let mut team = Team {
            id: uuid::Uuid::new_v4().to_string(),
            title: "Team".into(),
            project: "/tmp/project".into(),
            allow_edits: false,
            created_at: 0,
            context: Vec::new(),
            members: vec![
                Member {
                    session: uuid::Uuid::new_v4().to_string(),
                    agent: AgentKind::Codex,
                    model: None,
                    task: "Inspect".into(),
                    status: Status::Working,
                    attempted: false,
                    cancel: Arc::new(AtomicBool::new(false)),
                },
                Member {
                    session: uuid::Uuid::new_v4().to_string(),
                    agent: AgentKind::Claude,
                    model: None,
                    task: "Review".into(),
                    status: Status::Complete,
                    attempted: false,
                    cancel: Arc::new(AtomicBool::new(false)),
                },
            ],
        };
        assert!(team.valid());
        team.hold_on_restart();
        assert_eq!(team.members[0].status, Status::Held);
        assert_eq!(team.members[1].status, Status::Complete);
        team.members[1].session = team.members[0].session.clone();
        assert!(!team.valid());
        assert!(overlapping("/tmp/p".as_ref(), "/tmp/p/sub".as_ref()));
        assert!(!overlapping("/tmp/p".as_ref(), "/tmp/project".as_ref()));
    }
}

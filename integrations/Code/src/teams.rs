//! Bounded, durable team plans. Native run handles remain the authority for
//! execution; restart holds unfinished tasks rather than resubmitting prompts.
use crate::discovery::AgentKind;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

pub const MAX_TEAMS: usize = 8;
pub const MAX_ORCHESTRATED_TEAMS:usize=64;
pub const MAX_MEMBERS: usize = 4;
pub const MAX_REMOTE_DELIVERY_HISTORY: usize = 256;
pub const MAX_TASK_BYTES: usize = 4096;
pub const MAX_REMOTE_PROMPT_BYTES: usize = crate::input::MAX_DRAFT_BYTES;

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
    #[serde(default)]
    pub orchestrator: bool,
    #[serde(default)]
    pub service_tier: Option<String>,
    pub session: String,
    pub agent: AgentKind,
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort:Option<String>,
    pub task: String,
    pub status: Status,
    #[serde(default)]
    pub attempted: bool,
    #[serde(skip)]
    pub cancel: Arc<AtomicBool>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Team {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remote_prompt: bool,
    #[serde(default)]
    pub owner:Option<String>,
    #[serde(default)]
    pub permissions:Option<(crate::runtime::CodexAccess,crate::runtime::CodexApprovals)>,
    pub id: String,
    pub title: String,
    pub project: PathBuf,
    pub allow_edits: bool,
    pub created_at: i64,
    pub members: Vec<Member>,
    #[serde(default)]
    pub context: Vec<crate::context_refs::Reference>,
}
/// Acceptance metadata for a prompt that no longer needs an execution slot.
/// The transport keeps the authenticated client's full receipt separately.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RemoteDelivery {
    pub owner: uuid::Uuid,
    pub chat: String,
    pub project: PathBuf,
    pub status: Status,
}

impl RemoteDelivery {
    pub fn valid(&self) -> bool {
        !self.chat.is_empty() && self.chat.len() <= 512 && !self.chat.chars().any(char::is_control)
            && self.project.is_absolute() && self.project.as_os_str().len() <= 4096
            && matches!(self.status, Status::Complete | Status::Failed | Status::Stopped)
    }

    fn from_team(team: &Team) -> Option<Self> {
        let member = team.members.first().filter(|member|
            team.remote_prompt && matches!(member.status, Status::Complete | Status::Failed | Status::Stopped))?;
        Some(Self {
            owner: uuid::Uuid::parse_str(team.owner.as_deref()?).ok()?,
            chat: member.session.clone(),
            project: team.project.clone(),
            status: member.status.clone(),
        })
    }
}

pub fn retire_remote_deliveries(teams: &mut Vec<Team>, history: &mut Vec<RemoteDelivery>) {
    teams.retain(|team| {
        if let Some(delivery) = RemoteDelivery::from_team(team) {
            history.push(delivery);
            false
        } else {
            true
        }
    });
    history.drain(..history.len().saturating_sub(MAX_REMOTE_DELIVERY_HISTORY));
}

impl Team {
    pub fn valid(&self) -> bool {
        uuid::Uuid::parse_str(&self.id).is_ok()
            && (!self.remote_prompt || self.owner.is_some() && self.members.len() == 1)
            && self.title.len() <= 256
            && self.project.is_absolute()
            && self.project.as_os_str().len() <= 4096
            && (if self.owner.is_some(){1}else{2}..=MAX_MEMBERS).contains(&self.members.len())
            && self.owner.as_ref().is_none_or(|id|uuid::Uuid::parse_str(id).is_ok())
            && self.context.len() <= crate::context_refs::MAX_REFERENCES
            && self
                .context
                .iter()
                .all(crate::context_refs::Reference::valid)
            && self.members.iter().all(|m| {
                (uuid::Uuid::parse_str(&m.session).is_ok()||self.owner.is_some()&&!m.session.is_empty()&&m.session.len()<=512&&!m.session.chars().any(char::is_control))
                    && !m.task.trim().is_empty()
                    && m.task.len() <= if self.remote_prompt { MAX_REMOTE_PROMPT_BYTES } else { MAX_TASK_BYTES }
                    && m.model.as_ref().is_none_or(|s| crate::runtime::valid_model_id(s))
                    && m.reasoning_effort.as_ref().is_none_or(|s|!s.is_empty()&&s.len()<=64&&!s.chars().any(char::is_control))
            })
            && self.members.iter().enumerate().all(|(i, m)| {
                !self.members[..i]
                    .iter()
                    .any(|other| other.session == m.session)
            })
    }
    pub fn hold_on_restart(&mut self) {
        for member in &mut self.members {
            if member.status.active() || member.status == Status::Queued && (!self.remote_prompt || member.attempted) {
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
            remote_prompt: false,
            owner:None, permissions:None, id: uuid::Uuid::new_v4().to_string(),
            title: "Team".into(),
            project: "/tmp/project".into(),
            allow_edits: false,
            created_at: 0,
            context: Vec::new(),
            members: vec![
                Member {
                    orchestrator: false,
                    service_tier: None,
                    session: uuid::Uuid::new_v4().to_string(),
                    agent: AgentKind::Codex,
                    model: None,
                    reasoning_effort:None,
                    task: "Inspect".into(),
                    status: Status::Working,
                    attempted: false,
                    cancel: Arc::new(AtomicBool::new(false)),
                },
                Member {
                    orchestrator: false,
                    service_tier: None,
                    session: uuid::Uuid::new_v4().to_string(),
                    agent: AgentKind::Claude,
                    model: None,
                    reasoning_effort:None,
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

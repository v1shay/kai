//! Explicit follow-ups, held on restart or failure. Queue payloads are bounded
//! and reuse imported images; no hidden scheduler or duplicate turn submission.
use crate::{context_refs::Reference, discovery::AgentKind};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub const MAX_ITEMS: usize = 16;
pub const MAX_TEXT: usize = 8192;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Prompt {
    pub id: String,
    pub session: String,
    pub agent: AgentKind,
    pub model: Option<String>,
    pub allow_edits: bool,
    pub text: String,
    pub images: Vec<PathBuf>,
    pub context: Vec<Reference>,
    pub held: bool,
}
impl Prompt {
    pub fn valid(&self) -> bool {
        uuid::Uuid::parse_str(&self.id).is_ok()
            && !self.session.is_empty()
            && self.session.len() <= 512
            && (!self.text.trim().is_empty() || !self.images.is_empty())
            && self.text.len() <= MAX_TEXT
            && self.images.len() <= crate::attachments::MAX_ATTACHMENTS
            && self
                .images
                .iter()
                .all(|p| p.is_absolute() && p.as_os_str().len() <= 4096)
            && self.context.len() <= crate::context_refs::MAX_REFERENCES
            && self.context.iter().all(Reference::valid)
            && self.model.as_ref().is_none_or(|s| s.len() <= 256)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_payloads_are_bounded_and_hold_is_durable() {
        let mut prompt = Prompt {
            id: uuid::Uuid::new_v4().to_string(),
            session: "chat".into(),
            agent: AgentKind::Codex,
            model: None,
            allow_edits: false,
            text: "Review the result".into(),
            images: vec![],
            context: vec![],
            held: true,
        };
        assert!(prompt.valid());
        let restored: Prompt =
            serde_json::from_str(&serde_json::to_string(&prompt).unwrap()).unwrap();
        assert!(restored.held);
        prompt.text = "x".repeat(MAX_TEXT + 1);
        assert!(!prompt.valid());
    }
}

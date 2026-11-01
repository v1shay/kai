//! Capabilities implemented by Code's bridges, rather than an assertion that
//! every feature in a provider's terminal UI has a native equivalent.
use crate::discovery::AgentKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub image_input: bool,
    pub native_delegation: bool,
}
pub fn for_provider(agent: AgentKind) -> Capabilities {
    Capabilities {
        image_input: matches!(
            agent,
            AgentKind::Ninja | AgentKind::Codex
                | AgentKind::Claude
                | AgentKind::OpenCode
                | AgentKind::Gemini
                | AgentKind::Cursor
                | AgentKind::Grok
        ),
        native_delegation: matches!(
            agent,
            AgentKind::Ninja | AgentKind::Codex | AgentKind::Claude | AgentKind::OpenCode | AgentKind::Cursor
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bridge_features_require_explicit_provider_support() {
        for agent in [
            AgentKind::Codex,
            AgentKind::Claude,
            AgentKind::OpenCode,
            AgentKind::Cursor,
        ] {
            assert!(for_provider(agent).image_input && for_provider(agent).native_delegation);
        }
        assert!(for_provider(AgentKind::Gemini).image_input);
        assert!(!for_provider(AgentKind::Gemini).native_delegation);
        assert!(for_provider(AgentKind::Grok).image_input);
        assert!(!for_provider(AgentKind::Grok).native_delegation);
        for agent in [AgentKind::Antigravity] {
            assert!(!for_provider(agent).image_input && !for_provider(agent).native_delegation);
        }
    }
}

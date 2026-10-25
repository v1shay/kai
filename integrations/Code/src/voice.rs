//! Codex conversational voice, using Kai's authenticated WebRTC workflow.
//! Schema checked with installed codex 0.162.1 generate-json-schema --experimental.
//! WebRTC carries audio directly; Rust sees only signaling and text/status events.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const MAX_SDP_BYTES: usize = 128 * 1024;
/// Realtime V3 currently uses the V1 family, as verified by Kai's real sessions.
pub const VOICES: &[&str] = &[
    "arbor", "breeze", "cove", "ember", "juniper", "maple", "sol", "spruce", "vale",
];
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Signal {
    Started {
        session_id: String,
    },
    Answer {
        session_id: String,
        sdp: String,
    },
    Closed {
        session_id: String,
        reason: Option<String>,
    },
    Transcript {
        session_id: String,
        id: String,
        role: String,
        text: String,
        final_result: bool,
    },
}
impl Signal {
    pub fn weight(&self) -> usize {
        match self {
            Self::Started { session_id } => session_id.len() + 64,
            Self::Answer { session_id, sdp } => session_id.len() + sdp.len() + 64,
            Self::Closed { session_id, reason } => {
                session_id.len() + reason.as_ref().map_or(0, String::len) + 64
            }
            Self::Transcript {
                session_id,
                id,
                role,
                text,
                ..
            } => session_id.len() + id.len() + role.len() + text.len() + 128,
        }
    }
}
#[derive(Clone, Debug)]
pub struct Start {
    pub session_id: String,
    pub sdp: String,
    pub voice: Option<String>,
}
impl Start {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            uuid::Uuid::parse_str(&self.session_id).is_ok(),
            "Invalid voice session identity"
        );
        ensure!(
            valid_sdp(&self.sdp),
            "A complete WebRTC offer is required for Codex voice"
        );
        ensure!(
            self.voice
                .as_ref()
                .is_none_or(|v| VOICES.contains(&v.as_str())),
            "Choose a supported Codex voice"
        );
        Ok(())
    }
    pub fn params(&self) -> Result<Value> {
        self.validate()?;
        let mut params = json!({
            "realtimeSessionId":self.session_id,"outputModality":"audio",
            "transport":{"type":"webrtc","sdp":self.sdp},"version":"v3",
            "model":"gpt-live-1-codex","flushTranscriptTailOnSessionEnd":true,
            "clientManagedHandoffs":false
        });
        if let Some(voice) = &self.voice {
            params["voice"] = json!(voice);
        }
        Ok(params)
    }
}
pub fn valid_sdp(sdp: &str) -> bool {
    sdp.starts_with("v=0") && sdp.len() <= MAX_SDP_BYTES && !sdp.contains('\0')
}
pub fn session_id(params: &Value) -> Result<String> {
    let id = params["realtimeSessionId"].as_str().unwrap_or("");
    ensure!(
        uuid::Uuid::parse_str(id).is_ok(),
        "Invalid Codex voice session identity"
    );
    ensure!(
        params["transport"]["type"] == "webrtc"
            && params["version"] == "v3"
            && params["outputModality"] == "audio",
        "Codex voice requires the native V3 WebRTC audio transport"
    );
    ensure!(
        params["transport"]["sdp"].as_str().is_some_and(valid_sdp),
        "A complete WebRTC offer is required"
    );
    Ok(id.into())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_start_is_webrtc_and_keeps_codex_auth_inside_server() {
        let start = Start {
            session_id: uuid::Uuid::new_v4().to_string(),
            sdp: "v=0\r\nOFFER".into(),
            voice: Some("cove".into()),
        };
        let p = start.params().unwrap();
        assert_eq!(session_id(&p).unwrap(), start.session_id);
        assert_eq!(p["model"], "gpt-live-1-codex");
        assert_eq!(p["transport"]["type"], "webrtc");
        assert!(!p.to_string().contains("api_key"));
        assert_eq!(p["clientManagedHandoffs"], false);
    }
    #[test]
    fn stale_or_invalid_signaling_is_rejected() {
        assert!(!valid_sdp("not-sdp"));
        assert!(!valid_sdp(&format!("v=0{}", "x".repeat(MAX_SDP_BYTES))));
        assert!(
            Start {
                session_id: "old".into(),
                sdp: "v=0".into(),
                voice: None
            }
            .validate()
            .is_err()
        );
    }
}

/// An explicit default omits the wire option so Codex uses its own setting.
pub fn configured_voice(choice: &str) -> Option<String> {
    (choice != "default").then(|| choice.to_owned())
}
#[cfg(test)]
mod unified_voice_tests {
    #[test]
    fn unified_default_voice_omits_the_wire_override() {
        assert_eq!(super::configured_voice("default"), None);
        assert_eq!(super::configured_voice("cove"), Some("cove".into()));
    }
}

//! Local inference selection, remote inventory and harness transport for Ninja.
mod catalog;
mod gateway;
mod http;
use anyhow::{Result, ensure};
pub use catalog::*;
pub use gateway::{configure_claude, configure_codex};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    #[default]
    Ollama,
    LlamaCpp,
}
impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::LlamaCpp => "llama.cpp",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Harness {
    #[default]
    Codex,
    Claude,
}
impl Harness {
    pub fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Selection {
    pub machine: String,
    pub endpoint: String,
    pub engine: Engine,
    pub harness: Harness,
    pub model: String,
    pub thinking: Option<String>,
    pub context: usize,
    pub node: bool,
}
impl Default for Selection {
    fn default() -> Self {
        Self {
            machine: "local".into(),
            endpoint: "http://127.0.0.1:11434".into(),
            engine: Engine::Ollama,
            harness: Harness::Codex,
            model: String::new(),
            thinking: None,
            context: 65536,
            node: false,
        }
    }
}
impl Selection {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.model.is_empty()
                && self.model.len() <= 256
                && !self.model.chars().any(char::is_control),
            "Choose a Ninja model"
        );
        ensure!(
            !self.model.ends_with(":cloud") && !self.model.ends_with("-cloud"),
            "Choose a downloadable local model tag"
        );
        http::validate_endpoint(&self.endpoint)?;
        ensure!(
            (2048..=2_097_152).contains(&self.context),
            "Invalid model context limit"
        );
        if let Some(v) = &self.thinking {
            ensure!(
                v.len() <= 32 && !v.chars().any(char::is_control),
                "Invalid thinking value"
            );
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(serde_json::to_vec(self).unwrap_or_default());
        digest.iter().map(|v| format!("{v:02x}")).collect()
    }
    pub fn api(&self, path: &str) -> String {
        format!("{}{}", self.endpoint.trim_end_matches('/'), path)
    }
}
pub fn data_dir() -> PathBuf {
    std::env::var_os("ORBIT_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_local_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join("Kai Unified")
        })
        .join("ninja")
}
pub fn executable(harness: Harness, home: &Path) -> Option<PathBuf> {
    let name = match harness {
        Harness::Codex => "codex",
        Harness::Claude => "claude",
    };
    crate::process_env::dirs(home)
        .into_iter()
        .map(|p| p.join(name))
        .find(|p| p.is_file())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    pub name: String,
    pub host: String,
    pub online: bool,
    pub local: bool,
    pub os: String,
}
pub fn local_machine() -> Machine {
    Machine {
        id: "local".into(),
        name: "This Mac".into(),
        host: "127.0.0.1".into(),
        online: true,
        local: true,
        os: std::env::consts::OS.into(),
    }
}
pub fn machines() -> Result<Vec<Machine>> {
    let mut found = vec![local_machine()];
    let Some(cli) = crate::process_env::dirs(&dirs::home_dir().unwrap_or_default())
        .into_iter()
        .map(|p| p.join("tailscale"))
        .find(|p| p.is_file())
        .or_else(|| {
            [
                "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
                "/usr/local/bin/tailscale",
            ]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
        })
    else {
        return Ok(found);
    };
    let value = http::command_json(&cli, &["status", "--json"], Duration::from_secs(4))?;
    for p in value["Peer"]
        .as_object()
        .into_iter()
        .flat_map(|v| v.values())
    {
        let Some(ip) = p["TailscaleIPs"].as_array().and_then(|v| {
            v.iter()
                .filter_map(|v| v.as_str())
                .find(|s| s.contains('.'))
        }) else {
            continue;
        };
        let name = p["HostName"].as_str().unwrap_or(ip);
        let host = p["DNSName"]
            .as_str()
            .map(|s| s.trim_end_matches('.'))
            .filter(|s| !s.is_empty())
            .unwrap_or(ip);
        found.push(Machine {
            id: ip.into(),
            name: name.into(),
            host: host.into(),
            online: p["Online"].as_bool().unwrap_or(false),
            local: false,
            os: p["OS"].as_str().unwrap_or("").into(),
        });
    }
    found[1..].sort_by(|a, b| {
        b.online
            .cmp(&a.online)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(found)
}
pub fn detect(machine: &Machine, engine: Engine) -> Result<(String, bool)> {
    ensure!(machine.online, "{} is offline", machine.name);
    let mut endpoints = if machine.local {
        vec![]
    } else {
        vec![
            (format!("http://{}:32187", machine.id), true),
            (format!("https://{}/ninja", machine.host), true),
        ]
    };
    endpoints.push((
        format!(
            "http://{}:{}",
            machine.id.replace("local", "127.0.0.1"),
            if engine == Engine::Ollama {
                11434
            } else {
                8080
            }
        ),
        false,
    ));
    for (endpoint, node) in endpoints {
        let path = if node {
            "/status"
        } else if engine == Engine::Ollama {
            "/api/version"
        } else {
            "/v1/models"
        };
        if http::json(
            "GET",
            &format!("{endpoint}{path}"),
            None,
            Duration::from_secs(2),
        )
        .is_ok()
        {
            return Ok((endpoint, node));
        }
    }
    anyhow::bail!(
        "No inference server is reachable on {}. Start Ollama/llama.cpp or install the Ninja host helper.",
        machine.name
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selections_reject_public_routes_and_keep_harnesses_isolated() {
        let mut s = Selection {
            model: "qwen3:8b".into(),
            ..Default::default()
        };
        assert!(s.validate().is_ok());
        let first = s.fingerprint();
        s.harness = Harness::Claude;
        assert_ne!(s.fingerprint(), first);
        for url in [
            "https://api.openai.com",
            "http://8.8.8.8:11434",
            "http://127.0.0.1@attacker.test",
            "http://127.0.0.1/../secret",
        ] {
            s.endpoint = url.into();
            assert!(s.validate().is_err(), "{url}");
        }
        for url in [
            "http://127.0.0.1:11434",
            "http://100.101.102.103:11434",
            "https://inference.example-tailnet.ts.net/ninja",
        ] {
            s.endpoint = url.into();
            assert!(s.validate().is_ok(), "{url}");
        }
    }
    #[test]
    fn installed_models_sort_before_registry_and_by_lab() {
        let make = |id: &str, installed: bool| Model {
            id: id.into(),
            lab: lab(id, ""),
            label: id.into(),
            installed,
            size: 0,
            format: "gguf".into(),
            tools: false,
            vision: false,
            thinking: vec![],
            thinking_default: None,
            context: 0,
        };
        let mut models = vec![
            make("qwen3", false),
            make("qwen3:8b", true),
            make("deepseek-r1:8b", true),
        ];
        sort(&mut models);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["deepseek-r1:8b", "qwen3:8b", "qwen3"]
        );
    }
    #[test]
    #[ignore = "Explicit local native-harness integration; requires KAI_NINJA_TEST_ENDPOINT"]
    fn installed_harness_uses_ninja_inference_and_native_tools() {
        use crate::runtime::{self, RunRequest, RuntimeEvent};
        struct NativeCleanup;
        impl Drop for NativeCleanup {
            fn drop(&mut self) {
                let done = runtime::broker::shutdown();
                let until = std::time::Instant::now() + Duration::from_secs(12);
                while !done.is_closed() && std::time::Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        let _cleanup = NativeCleanup;
        let endpoint = std::env::var("KAI_NINJA_TEST_ENDPOINT")
            .expect("Use an isolated inference fixture or explicitly selected test host");
        let harness = if std::env::var("KAI_NINJA_TEST_HARNESS").as_deref() == Ok("claude") {
            Harness::Claude
        } else {
            Harness::Codex
        };
        let selection = Selection {
            endpoint,
            node: std::env::var("KAI_NINJA_TEST_NODE").as_deref() == Ok("1"),
            engine: if std::env::var("KAI_NINJA_TEST_ENGINE").as_deref() == Ok("llama_cpp") {
                Engine::LlamaCpp
            } else {
                Engine::Ollama
            },
            model: std::env::var("KAI_NINJA_TEST_MODEL").unwrap_or("ninja-fixture".into()),
            harness,
            context: 65536,
            ..Default::default()
        };
        let root = std::env::temp_dir().join(format!("kai-ninja-live-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("proof.txt"), "NINJA_LOCAL_TOOLS_OK").unwrap();
        let request = RunRequest {
            ninja: Some(selection),
            orchestrator: false,
            agent: crate::discovery::AgentKind::Ninja,
            executable: executable(harness, &dirs::home_dir().unwrap()).unwrap(),
            project: root.clone(),
            prompt: format!(
                "Read {} with your file or shell tool. Reply with its contents only.",
                root.join("proof.txt").display()
            ),
            resume_id: None,
            fork_session: false,
            model: None,
            service_tier: None,
            reasoning_effort: None,
            images: vec![],
            allow_edits: false,
            trust_workspace: true,
            codex_approvals: runtime::CodexApprovals::Never,
            codex_access: Default::default(),
        };
        let mut followup = request.clone();
        let handle = runtime::launch(request).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(100);
        let mut output = String::new();
        let mut done = false;
        let mut native_id = None;
        while std::time::Instant::now() < until {
            match handle.events.try_recv() {
                Ok(RuntimeEvent::SessionId(id)) => native_id = Some(id),
                Ok(RuntimeEvent::Text(s)) => output.push_str(&s),
                Ok(RuntimeEvent::ReplaceText(s)) => output = s,
                Ok(RuntimeEvent::Failed(error)) => {
                    handle.cancel();
                    panic!("Ninja {harness:?}: {error}")
                }
                Ok(RuntimeEvent::Finished) => {
                    done = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(done, "Native harness did not complete: {output}");
        assert!(
            output.contains("NINJA_LOCAL_TOOLS_OK"),
            "Wrong native output: {output}"
        );
        std::fs::remove_file(root.join("proof.txt")).unwrap();
        followup.resume_id = Some(native_id.expect("Native session identity was not reported"));
        followup.prompt="What exact text was in the file you just read? Reply with it again from this conversation; do not use tools.".into();
        followup.ninja.as_mut().unwrap().context = 69632;
        let second = runtime::launch(followup).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(100);
        let mut result = String::new();
        let mut done = false;
        while std::time::Instant::now() < until {
            match second.events.try_recv() {
                Ok(RuntimeEvent::Text(s)) => result.push_str(&s),
                Ok(RuntimeEvent::ReplaceText(s)) => result = s,
                Ok(RuntimeEvent::Failed(e)) => {
                    second.cancel();
                    panic!("Native resume failed: {e}")
                }
                Ok(RuntimeEvent::Finished) => {
                    done = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        second.cancel();
        handle.cancel();
        assert!(
            done && result.contains("NINJA_LOCAL_TOOLS_OK"),
            "Context did not survive a new inference profile: {result}"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

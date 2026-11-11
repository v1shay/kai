//! Interactive provider commands share the same working directory and session IDs
//! as their GUI adapters. Preparation can inspect provider state and must run off
//! the UI thread.
use crate::{discovery::{self, AgentKind}, ninja};
use std::{path::Path, process::Command};

pub(super) struct PreparedLaunch {
    pub arguments: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub retained: Option<Box<dyn std::marker::Send>>,
}

fn checked_value(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 4096 || value.starts_with('-') || value.chars().any(char::is_control) {
        return Err(format!("The {label} is invalid."));
    }
    Ok(())
}

pub(super) fn interactive_arguments(kind: AgentKind, session: Option<&str>, model: Option<&str>) -> Result<Vec<String>, String> {
    if kind == AgentKind::Ninja {
        return Err("Ninja requires its configured native harness.".into());
    }
    let mut args = Vec::new();
    if let Some(session) = session {
        checked_value(session, "session ID")?;
        let flag = match kind {
            AgentKind::Codex => "resume",
            AgentKind::Claude | AgentKind::Gemini | AgentKind::Cursor | AgentKind::Grok => "--resume",
            AgentKind::OpenCode => "--session",
            AgentKind::Antigravity => "--conversation",
            AgentKind::Ninja => unreachable!(),
        };
        args.extend([flag.into(), session.into()]);
    }
    if let Some(model) = model.filter(|model| !model.is_empty()) {
        checked_value(model, "model")?;
        args.extend(["--model".into(), model.into()]);
    }
    Ok(args)
}

pub(super) fn shell_arguments(executable: &Path, arguments: Vec<String>) -> Vec<String> {
    // All provider/user values remain positional arguments. A failed startup
    // stays readable until Return instead of disappearing with its pane.
    let mut shell = vec![
        "-c".into(),
        r#""$@"; result=$?; if [ "$result" -ne 0 ]; then printf '\nAgent CLI exited with status %s. Press Return to go back.\n' "$result"; read answer; fi; exit "$result""#.into(),
        "kai-chat-cli".into(),
        executable.to_string_lossy().into_owned(),
    ];
    shell.extend(arguments);
    shell
}

pub(super) fn prepare(kind: AgentKind, executable: &Path, directory: &Path, session: Option<&str>, model: Option<&str>, ninja_selection: Option<&ninja::Selection>) -> Result<PreparedLaunch, String> {
    if !directory.is_dir() { return Err("The chat’s working directory is unavailable.".into()); }
    if !executable.is_file() { return Err(format!("{} CLI is unavailable.", kind.name())); }
    let mut environment = Vec::new();
    let mut remove_environment = Vec::new();
    let mut retained: Option<Box<dyn std::marker::Send>> = None;
    let arguments = if kind == AgentKind::Ninja {
        let selection = ninja_selection.ok_or("Choose a Ninja model and route before opening its CLI.")?;
        let harness = match selection.harness {
            ninja::Harness::Codex => AgentKind::Codex,
            ninja::Harness::Claude => AgentKind::Claude,
        };
        let mut command = Command::new(executable);
        command.current_dir(directory);
        let lease = match selection.harness {
            ninja::Harness::Codex => ninja::configure_codex(&mut command, selection),
            ninja::Harness::Claude => ninja::configure_claude(&mut command, selection),
        }.map_err(|error| format!("Could not prepare the Ninja CLI: {error}"))?;
        let mut arguments = command.get_args().map(|value| value.to_string_lossy().into_owned()).collect::<Vec<_>>();
        arguments.extend(interactive_arguments(harness, session, Some(&selection.model))?);
        for (name, value) in command.get_envs() {
            let name = name.to_str().ok_or("A Ninja environment variable name is invalid.")?;
            if let Some(value) = value {
                let value = value.to_str().ok_or("A Ninja environment variable is invalid.")?;
                environment.push((name.to_owned(), value.to_owned()));
            } else {
                if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    || name.as_bytes()[0].is_ascii_digit() {
                    return Err("A Ninja environment variable name is invalid.".into());
                }
                remove_environment.push(name.to_owned());
            }
        }
        retained = Some(Box::new(lease));
        arguments
    } else {
        let mut arguments = interactive_arguments(kind, session, model)?;
        if kind == AgentKind::Codex && let Some(session) = session {
            let home = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty())
                .map(std::path::PathBuf::from).map(|path| if path.is_absolute() { path } else { directory.join(path) })
                .or_else(|| dirs::home_dir().map(|home| home.join(".codex")));
            if let Some(provider) = home.and_then(|home| discovery::codex_model_provider_in(&home, session)) {
                for setting in ["model_providers.kai-openai-https.name=\"OpenAI\"",
                                "model_providers.kai-openai-https.wire_api=\"responses\"",
                                "model_providers.kai-openai-https.requires_openai_auth=true",
                                "model_providers.kai-openai-https.supports_websockets=false"] {
                    arguments.extend(["-c".into(), setting.into()]);
                }
                arguments.extend(["-c".into(), format!("model_provider={}", serde_json::json!(provider))]);
            }
        }
        arguments
    };
    let mut arguments = shell_arguments(executable, arguments);
    if !remove_environment.is_empty() {
        // The native Ghostty environment inherits process variables. Unset only
        // validated names in the child shell; credential values stay in env.
        arguments[1].insert_str(0, &format!("unset {}; ", remove_environment.join(" ")));
    }
    Ok(PreparedLaunch { arguments, environment, retained })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_standalone_provider_uses_its_own_resume_protocol() {
        for (provider, flag) in [(AgentKind::Codex, "resume"), (AgentKind::Claude, "--resume"),
            (AgentKind::Gemini, "--resume"), (AgentKind::OpenCode, "--session"),
            (AgentKind::Antigravity, "--conversation"), (AgentKind::Cursor, "--resume"),
            (AgentKind::Grok, "--resume")] {
            assert_eq!(interactive_arguments(provider, Some("provider-session_123"), None).unwrap(), [flag, "provider-session_123"]);
            assert!(interactive_arguments(provider, None, None).unwrap().is_empty());
            assert_eq!(interactive_arguments(provider, None, Some("provider/model")).unwrap(), ["--model", "provider/model"]);
        }
    }
    #[test]
    fn shell_launch_preserves_working_directory_and_literal_arguments() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("kai-cli-args-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let executable = root.join("agent with spaces");
        std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\"\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let session = "session with ' quotes";
        let model = "provider/model;$(touch should-not-run)";
        let args = shell_arguments(&executable, interactive_arguments(AgentKind::OpenCode, Some(session), Some(model)).unwrap());
        let output = std::process::Command::new("/bin/sh").args(args).current_dir(&root).output().unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap().lines().collect::<Vec<_>>(), [root.to_str().unwrap(), "--session", session, "--model", model]);
        assert!(!root.join("should-not-run").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn values_cannot_turn_into_options_or_shell_commands() {
        assert!(interactive_arguments(AgentKind::Claude, Some("--help"), None).is_err());
        assert!(interactive_arguments(AgentKind::Claude, Some("bad\nvalue"), None).is_err());
        let model = "provider/model;$(touch should-not-run)";
        let args = shell_arguments(Path::new("/tmp/my agent"), interactive_arguments(AgentKind::OpenCode, Some("session 123"), Some(model)).unwrap());
        assert_eq!(&args[3..], ["/tmp/my agent", "--session", "session 123", "--model", model]);
    }
}

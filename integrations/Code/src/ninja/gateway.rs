use super::*;
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Instant,
};
struct Shared {
    child: Mutex<Child>,
    root: PathBuf,
    port: u16,
    token: String,
}
impl Drop for Shared {
    fn drop(&mut self) {
        let child = self.child.get_mut().unwrap_or_else(|e| e.into_inner());
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}
static GATEWAY: OnceLock<Mutex<Weak<Shared>>> = OnceLock::new();
pub struct GatewayLease {
    shared: Arc<Shared>,
    pub route: Selection,
}
impl GatewayLease {
    pub fn acquire(selection: &Selection) -> Result<Self> {
        selection.validate()?;
        let mut slot = GATEWAY
            .get_or_init(|| Mutex::new(Weak::new()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let shared = if let Some(s) = slot.upgrade() {
            anyhow::ensure!(
                s.child
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .try_wait()?
                    .is_none(),
                "Ninja bridge exited; retry the turn"
            );
            s
        } else {
            let root = data_dir().join(format!("bridge-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root)?;
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
            let script = root.join("bridge.py");
            private_write(&script, include_bytes!("../../native_helpers/kai_ninja.py"))?;
            private_write(&root.join("kai_inference_stream.py"),include_bytes!("../../native_helpers/kai_inference_stream.py"))?;
            let ready = root.join("ready");
            let token = format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            );
            let mut child = Command::new("/usr/bin/python3")
                .arg(&script)
                .arg("--routes")
                .arg(&root)
                .arg("--ready")
                .arg(&ready)
                .args(["--token", "stdin"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            let mut stdin = child
                .stdin
                .take()
                .context("Ninja bridge input unavailable")?;
            writeln!(stdin, "{token}")?;
            drop(stdin);
            let until = Instant::now() + Duration::from_secs(4);
            let port = loop {
                if let Ok(p) = fs::read_to_string(&ready)
                    .unwrap_or_default()
                    .trim()
                    .parse::<u16>()
                {
                    break p;
                }
                if child.try_wait()?.is_some() || Instant::now() >= until {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = fs::remove_dir_all(&root);
                    anyhow::bail!("Ninja bridge could not start");
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            let s = Arc::new(Shared {
                child: Mutex::new(child),
                root,
                port,
                token,
            });
            *slot = Arc::downgrade(&s);
            s
        };
        private_write(
            &shared
                .root
                .join(format!("{}.json", selection.fingerprint())),
            &serde_json::to_vec(selection)?,
        )?;
        Ok(Self {
            shared,
            route: selection.clone(),
        })
    }
    fn base(&self) -> String {
        format!(
            "http://127.0.0.1:{}/route/{}",
            self.shared.port,
            self.route.fingerprint()
        )
    }
}
use anyhow::Context;
fn private_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    f.write_all(bytes)?;
    drop(f);
    fs::rename(temp, path)?;
    Ok(())
}
fn scoped_home(harness: Harness, selection: &Selection) -> Result<PathBuf> {
    // Two chats can create the same model/harness profile simultaneously.
    // Serialize only the short filesystem setup, never inference or metadata IO.
    static PROFILE_SETUP: Mutex<()> = Mutex::new(());
    let _setup = PROFILE_SETUP.lock().unwrap_or_else(|e| e.into_inner());
    let home = dirs::home_dir().context("Home folder unavailable")?;
    let name = if harness == Harness::Codex {
        "codex"
    } else {
        "claude"
    };
    let shared = data_dir().join(name).join("history");
    fs::create_dir_all(&shared)?;
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o700))?;
    let root = data_dir()
        .join(name)
        .join("profiles")
        .join(selection.fingerprint());
    fs::create_dir_all(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    for entry in if harness == Harness::Codex {
        vec![
            "sessions",
            "archived_sessions",
            "memories",
            "shell_snapshots",
        ]
    } else {
        vec!["projects", "session-env", "tasks", "todos"]
    } {
        let target = shared.join(entry);
        fs::create_dir_all(&target)?;
        let link = root.join(entry);
        if !link.exists() {
            symlink(&target, &link)?;
        }
    }
    let source = if harness == Harness::Codex {
        std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"))
    } else {
        std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"))
    };
    let config = if harness == Harness::Codex {
        "config.toml"
    } else {
        "settings.json"
    };
    if let Ok(meta) = fs::metadata(source.join(config)) {
        if meta.len() < 1024 * 1024 {
            let mut bytes = fs::read(source.join(config))?;
            if harness == Harness::Claude {
                if let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    if let Some(map) = v.as_object_mut() {
                        for key in [
                            "apiKeyHelper",
                            "awsAuthRefresh",
                            "awsCredentialExport",
                            "forceLoginMethod",
                        ] {
                            map.remove(key);
                        }
                    }
                    bytes = serde_json::to_vec(&v)?;
                }
            }
            private_write(&root.join(config), &bytes)?;
        }
    }
    for entry in if harness == Harness::Codex {
        vec!["skills", "rules", "plugins", "AGENTS.md"]
    } else {
        vec!["skills", "agents", "commands", "plugins", "CLAUDE.md"]
    } {
        let target = root.join(entry);
        if !target.exists() && source.join(entry).exists() {
            let _ = symlink(source.join(entry), target);
        }
    }
    Ok(root)
}
fn codex_catalog(selection: &Selection, vision: bool) -> serde_json::Value {
    serde_json::json!({"models":[{"slug":selection.model,"display_name":selection.model,"description":"Ninja local inference","model_messages":{"instructions_template":"You are Ninja, a coding agent inside Kai Unified, using the Codex harness. Model inference can run on another machine; the workspace, shell, MCP tools, and file edits run locally in the displayed working directory. Use the provided native tools to inspect and change files, run checks, and report verified results. Read and follow applicable repository instructions. Preserve the user’s work and obey the native permission and approval controls. Invoke tools with their documented schemas, and wait for real tool results before claiming success. When delegating, use the harness’s native subagent tools and collect their results. Keep responses clear and concise."},"supported_reasoning_levels":[],"shell_type":"unified_exec","visibility":"list","supported_in_api":true,"priority":0,"include_skills_usage_instructions":true,"include_plugin_usage_instructions":true,"include_apps_usage_instructions":true,"supports_reasoning_summary_parameter":true,"support_verbosity":false,"apply_patch_tool_type":"freeform","truncation_policy":{"mode":"tokens","limit":10000.min(selection.context/2)},"context_window":selection.context,"max_context_window":selection.context,"effective_context_window_percent":90,"experimental_supported_tools":[],"input_modalities":if vision{vec!["text","image"]}else{vec!["text"]},"tool_mode":"direct","multi_agent_version":"v1","supports_search_tool":false}]})
}
/// Copy role instructions and limits, pinning all role inference to this route.
/// Cloud credentials/configuration in the user's real Codex home stay untouched.
fn route_codex_roles(root: &Path, selection: &Selection, base: &str, catalog: &Path) -> Result<()> {
    let path = root.join("config.toml");
    let mut config = if path.exists() {
        toml::from_str::<toml::Value>(&fs::read_to_string(&path)?)?
    } else {
        toml::Value::Table(Default::default())
    };
    let source = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".codex"));
    if let Some(roles) = config.get_mut("agents").and_then(toml::Value::as_table_mut) {
        for (name, role) in roles.iter_mut() {
            if let Some(role) = role.as_table_mut() {
                if let Some(file) = role.get("config_file").and_then(toml::Value::as_str) {
                    let file = PathBuf::from(file);
                    let file = if file.is_absolute() {
                        file
                    } else {
                        source.join(file)
                    };
                    anyhow::ensure!(
                        fs::metadata(&file)?.len() < 1024 * 1024,
                        "Ninja role configuration exceeds 1 MiB"
                    );
                    let mut v: toml::Value = toml::from_str(&fs::read_to_string(&file)?)?;
                    let map = v
                        .as_table_mut()
                        .context("Invalid agent role configuration")?;
                    map.insert(
                        "model_provider".into(),
                        toml::Value::String("kai-ninja".into()),
                    );
                    map.insert("model".into(), toml::Value::String(selection.model.clone()));
                    map.remove("model_reasoning_effort");
                    map.remove("service_tier");
                    map.remove("agents");
                    let file = root.join(format!(
                        "{}-role-{}.toml",
                        selection.fingerprint(),
                        name.chars()
                            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                            .take(64)
                            .collect::<String>()
                    ));
                    private_write(&file, toml::to_string(&v)?.as_bytes())?;
                    role.insert(
                        "config_file".into(),
                        toml::Value::String(file.to_string_lossy().into_owned()),
                    );
                }
            }
        }
    }
    let map = config.as_table_mut().context("Invalid Codex config")?;
    map.insert(
        "model_provider".into(),
        toml::Value::String("kai-ninja".into()),
    );
    map.insert("model".into(), toml::Value::String(selection.model.clone()));
    map.remove("model_reasoning_effort");
    map.remove("service_tier");
    map.insert(
        "model_catalog_json".into(),
        toml::Value::String(catalog.to_string_lossy().into_owned()),
    );
    map.insert(
        "sqlite_home".into(),
        toml::Value::String(
            data_dir()
                .join("codex/history")
                .to_string_lossy()
                .into_owned(),
        ),
    );
    map.insert("web_search".into(), toml::Value::String("disabled".into()));
    let providers = map
        .entry("model_providers")
        .or_insert_with(|| toml::Value::Table(Default::default()))
        .as_table_mut()
        .context("Invalid model providers")?;
    let mut provider = toml::map::Map::new();
    for (k, v) in [
        ("name", toml::Value::String("Ninja".into())),
        ("base_url", toml::Value::String(base.into())),
        ("wire_api", toml::Value::String("responses".into())),
        ("env_key", toml::Value::String("KAI_NINJA_TOKEN".into())),
        ("requires_openai_auth", toml::Value::Boolean(false)),
        ("supports_websockets", toml::Value::Boolean(false)),
    ] {
        provider.insert(k.into(), v);
    }
    providers.insert("kai-ninja".into(), toml::Value::Table(provider));
    private_write(&path, toml::to_string(&config)?.as_bytes())?;
    Ok(())
}
pub fn configure_codex(command: &mut Command, selection: &Selection) -> Result<GatewayLease> {
    let lease = GatewayLease::acquire(selection)?;
    let base = format!("{}/v1", lease.base());
    let home = scoped_home(Harness::Codex, selection)?;
    let metadata = inspect(selection)?;
    let catalog = lease
        .shared
        .root
        .join(format!("{}-models.json", selection.fingerprint()));
    private_write(
        &catalog,
        &serde_json::to_vec(&codex_catalog(selection, metadata.vision))?,
    )?;
    route_codex_roles(&home, selection, &base, &catalog)?;
    command
        .args([
            "-c",
            "features.multi_agent=true",
            "-c",
            "agents.enabled=true",
            "-c",
            "web_search=\"disabled\"",
        ])
        .arg("-c")
        .arg(format!(
            "agents.default_subagent_model={}",
            serde_json::json!(selection.model)
        ))
        .arg("-c")
        .arg(format!("model_catalog_json={}", serde_json::json!(catalog)));
    for (key, value) in [
        ("model_provider", serde_json::json!("kai-ninja")),
        ("model", serde_json::json!(&selection.model)),
        ("model_providers.kai-ninja.name", serde_json::json!("Ninja")),
        (
            "model_providers.kai-ninja.wire_api",
            serde_json::json!("responses"),
        ),
        (
            "model_providers.kai-ninja.base_url",
            serde_json::json!(base),
        ),
        (
            "model_providers.kai-ninja.env_key",
            serde_json::json!("KAI_NINJA_TOKEN"),
        ),
        (
            "model_providers.kai-ninja.requires_openai_auth",
            serde_json::json!(false),
        ),
        (
            "model_providers.kai-ninja.supports_websockets",
            serde_json::json!(false),
        ),
        ("model_context_window", serde_json::json!(selection.context)),
        (
            "model_providers.kai-ninja.request_max_retries",
            serde_json::json!(0),
        ),
        (
            "model_providers.kai-ninja.stream_max_retries",
            serde_json::json!(0),
        ),
    ] {
        command.arg("-c").arg(format!("{key}={value}"));
    }
    command
        .env("CODEX_SQLITE_HOME", data_dir().join("codex/history"))
        .env("OPENAI_API_KEY", "")
        .env("OPENAI_BASE_URL", &base)
        .env("CODEX_HOME", home)
        .env("KAI_NINJA_TOKEN", &lease.shared.token);
    Ok(lease)
}
pub fn configure_claude(command: &mut Command, selection: &Selection) -> Result<GatewayLease> {
    let lease = GatewayLease::acquire(selection)?;
    let mut settings = serde_json::json!({"env":{"ANTHROPIC_BASE_URL":lease.base(),"ANTHROPIC_AUTH_TOKEN":lease.shared.token,"ANTHROPIC_API_KEY":"","CLAUDE_CODE_USE_BEDROCK":"","CLAUDE_CODE_USE_VERTEX":"","CLAUDE_CODE_USE_FOUNDRY":""}});
    for (key, value) in [
        (
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS",
            selection.context.to_string(),
        ),
        (
            "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
            (selection.context / 4).clamp(256, 32768).to_string(),
        ),
        ("CLAUDE_CODE_MAX_RETRIES", "0".into()),
        ("CLAUDE_CODE_RETRY_WATCHDOG", String::new()),
    ] {
        settings["env"][key] = serde_json::json!(value);
        command.env(key, settings["env"][key].as_str().unwrap());
    }
    for key in [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
    ] {
        settings["env"][key] = serde_json::json!(selection.model);
    }
    // CLI settings outrank project/user environment so an existing cloud profile
    // cannot redirect a Ninja request. This private invocation is the only change.
    let settings_file = lease
        .shared
        .root
        .join(format!("{}-claude-settings.json", selection.fingerprint()));
    private_write(&settings_file, &serde_json::to_vec(&settings)?)?;
    command.arg("--settings").arg(settings_file);
    let source = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".claude"));
    let source_file = if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        source.join(".claude.json")
    } else {
        dirs::home_dir().unwrap_or_default().join(".claude.json")
    };
    if fs::metadata(&source_file).is_ok_and(|m| m.len() < 2 * 1024 * 1024) {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&fs::read(source_file)?) {
            if v["mcpServers"].is_object() {
                let file = lease
                    .shared
                    .root
                    .join(format!("{}-mcp.json", selection.fingerprint()));
                private_write(
                    &file,
                    &serde_json::to_vec(&serde_json::json!({"mcpServers":v["mcpServers"]}))?,
                )?;
                command.arg("--mcp-config").arg(file);
            }
        }
    }
    let profile = scoped_home(Harness::Claude, selection)?;
    let path = profile.join("settings.json");
    let mut user: serde_json::Value = fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !user["env"].is_object() {
        user["env"] = serde_json::json!({});
    }
    for (k, v) in settings["env"].as_object().unwrap() {
        user["env"][k] = v.clone();
    }
    private_write(&path, &serde_json::to_vec(&user)?)?;
    command
        .env("ANTHROPIC_BASE_URL", lease.base())
        .env("ANTHROPIC_AUTH_TOKEN", &lease.shared.token)
        .env("ANTHROPIC_API_KEY", "")
        .env("CLAUDE_CONFIG_DIR", profile)
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
    for key in [
        "ANTHROPIC_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "CLAUDE_CODE_SUBAGENT_MODEL",
    ] {
        command.env(key, &selection.model);
    }
    for key in [
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
        "ANTHROPIC_CUSTOM_HEADERS",
    ] {
        command.env_remove(key);
    }
    Ok(lease)
}

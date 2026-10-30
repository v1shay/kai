//! On-demand native account/model controls and bounded metadata RPCs.
//! Credential-backed quota reads stay in an isolated worker; only normalized
//! usage windows return to the owner, never credentials or raw HTTP responses.
use crate::{discovery::AgentKind, runtime::valid_model_id};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{fs::OpenOptionsExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const PROBE_BYTES: usize = 64 * 1024;
const METADATA_BYTES: u64 = 2 * 1024 * 1024;
const MODEL_LIMIT: usize = 128;
const PROBE_TIMEOUT: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthState {
    VerifiedSignedIn,
    SignedOut,
    /// The native CLI reports stored credentials, without validating them.
    Configured,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    pub label: String,
    pub reasoning_efforts:Vec<String>,
}

pub use crate::usage::LimitWindow;

#[derive(Clone, Debug)]
pub struct CatalogItem {
    pub label: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct ModelDefaults {
    pub reset_credits:Option<Value>,
    pub fast_models:std::collections::HashMap<String,String>,
    pub model:Option<String>,
    pub effort:Option<String>,
    pub efforts:std::collections::HashMap<String,String>,
    pub aliases:std::collections::HashMap<String,String>,
    pub project:PathBuf,
    pub capabilities_model:Option<String>,
    pub capabilities_checked:bool,
}
#[derive(Clone, Debug)]
pub struct ControlInfo {
    pub defaults:ModelDefaults,
    pub auth: AuthState,
    pub models: Vec<ModelChoice>,
    pub note: Option<String>,
    pub account: Option<String>,
    pub limits: Vec<LimitWindow>,
    pub catalog: Option<(PathBuf, CatalogKind, Vec<CatalogItem>)>,
}

/// Call on a worker only when controls are opened/refreshed. No recurring probe.
#[cfg(test)]
pub fn inspect(kind: AgentKind, executable: &Path, home: &Path) -> ControlInfo {inspect_project(kind,executable,home,home)}
pub fn inspect_project(kind:AgentKind,executable:&Path,home:&Path,project:&Path)->ControlInfo{
    let mut info = ControlInfo {
        defaults:Default::default(),
        auth: AuthState::Unknown,
        models: Vec::new(),
        note: None,
        account: None,
        limits: Vec::new(),
        catalog: None,
    };
    if kind == AgentKind::Codex {
        match codex_metadata(executable, project) {
            Ok(mut snapshot) => {snapshot.defaults.project=project.into();return snapshot},
            Err(error) => {
                info.note = Some(format!(
                    "Live Codex metadata unavailable ({error}); showing the last local model catalog. Refresh to try again."
                ))
            }
        }
    }
    let status_args: Option<&[&str]> = match kind {
        AgentKind::Codex => None,
        AgentKind::Claude => Some(&["auth", "status", "--json"]),
        AgentKind::Cursor => Some(&["status", "--format", "json"]),
        AgentKind::OpenCode => Some(&["auth", "list", "--pure"]),
        _ => None,
    };
    if let Some(args) = status_args {
        match probe(executable, args, home, PROBE_TIMEOUT) {
            Ok(output) => info.auth = auth_state(kind, &output),
            Err(_) => {
                info.note =
                    Some("Account status could not be checked. Open the CLI to inspect it.".into())
            }
        }
    } else if kind != AgentKind::Codex {
        info.note = Some("This CLI does not expose a safe account-status command. Your existing CLI sign-in is used.".into());
    }
    match kind {
        AgentKind::Codex => {
            if let Some(cache) = metadata(&home.join(".codex/models_cache.json"))
                && let Some(models) = cache.get("models").and_then(Value::as_array)
            {
                for model in models.iter().take(512) {
                    if model.get("visibility").and_then(Value::as_str) == Some("hide") {
                        continue;
                    }
                    if let Some(id) = model.get("slug").and_then(Value::as_str) {
                        add_model(
                            &mut info.models,
                            id,
                            model
                                .get("display_name")
                                .and_then(Value::as_str)
                                .unwrap_or(id),
                        );
                    }
                }
            }
        }
        AgentKind::Claude => {
            if let Ok((models,defaults)) = claude_models(executable, project) {
                info.models = models;info.defaults=defaults;
            }
            // Help aliases are a fallback when this installed SDK handshake is unavailable.
            if info.models.is_empty()
                && let Ok(output) = probe(executable, &["--help"], home, PROBE_TIMEOUT)
                && output.success
            {
                let text = strip_ansi(&output.stdout);
                if let Some(line) = text
                    .lines()
                    .find(|line| line.trim_start().starts_with("--model "))
                {
                    for id in advertised_model_names(line) {
                        add_model(&mut info.models, &id, &id);
                    }
                }
            }
            configured_model(
                &mut info.models,
                &home.join(".claude/settings.json"),
                &["/model"],
            );
        }
        AgentKind::Gemini => configured_model(
            &mut info.models,
            &home.join(".gemini/settings.json"),
            &["/model/name"],
        ),
        AgentKind::Antigravity => {
            // `agy models` can initiate automatic sign-in. Read configuration only.
            configured_model(
                &mut info.models,
                &home.join(".gemini/antigravity-cli/settings.json"),
                &["/model"],
            );
        }
        AgentKind::Cursor | AgentKind::OpenCode | AgentKind::Grok => {
            // Cursor's models command starts login when credentials are absent.
            if kind != AgentKind::Cursor || info.auth == AuthState::VerifiedSignedIn {
                let args: &[&str] = if kind == AgentKind::OpenCode {
                    &["models", "--pure"]
                } else {
                    &["models"]
                };
                if let Ok(output) = probe(executable, args, home, PROBE_TIMEOUT)
                    && output.success
                {
                    info.models = catalog_lines(kind, &output.stdout);
                    let native=strip_ansi(&output.stdout);
                    info.defaults.model=native.lines().find_map(|line|{
                        let trimmed=line.trim();
                        let id=trimmed.strip_prefix("* ").and_then(|s|s.split_whitespace().next()).or_else(||(trimmed.rsplit_once('(').is_some_and(|(_,flags)|flags.trim_end_matches(')').split(',').any(|flag|matches!(flag.trim(),"default"|"current")))).then(||trimmed.split_whitespace().next()).flatten());
                        id.filter(|id|info.models.iter().any(|m|m.id==*id)).map(str::to_owned)
                    });
                }
            }
        }
    }
    crate::provider_defaults::resolve_configured_defaults(kind,home,project,&mut info);
    if kind==AgentKind::Gemini && info.defaults.model.is_none(){info.defaults.model=crate::provider_defaults::gemini_build_default(executable);}
    info.defaults.project=project.into();info
}

/// Read-only RPC metadata. No thread is resumed, no inference is requested,
/// and account email/token values are never retained. RPC failures are isolated:
/// a missing rate-limit endpoint does not discard a successful model catalog.
fn codex_metadata(executable: &Path, home: &Path) -> Result<ControlInfo> {
    let responses = codex_rpc(
        executable,
        home,
        &[
            (1, "account/read", serde_json::json!({"refreshToken":false})),
            (
                2,
                "model/list",
                serde_json::json!({"limit":128,"includeHidden":false}),
            ),
            (3, "account/rateLimits/read", Value::Null),
            (4,"config/read",serde_json::json!({"cwd":home,"includeLayers":false})),
        ],
    )?;
    Ok(codex_snapshot(&responses))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogKind {
    Skills,
    Mcp,
    Plugins,
    Tools,
    Hooks,
    Apps,
}
impl CatalogKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Skills => "codex-skills",
            Self::Mcp => "codex-mcp",
            Self::Plugins => "codex-plugins",
            Self::Tools => "codex-tools",
            Self::Hooks => "codex-hooks",
            Self::Apps => "codex-apps",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Skills => "Skills",
            Self::Mcp => "MCP servers",
            Self::Plugins => "Local plugins",
            Self::Tools => "Configured MCP tools",
            Self::Hooks => "Lifecycle hooks",
            Self::Apps => "Connected apps",
        }
    }
}
/// Explicit, on-demand discovery scoped to the selected project. Local plugin
/// discovery does not fetch marketplaces or install/enable anything.
pub fn inspect_catalog(
    executable: &Path,
    project: &Path,
    kind: CatalogKind,
) -> Result<Vec<CatalogItem>> {
    let (method, params) = match kind {
        CatalogKind::Skills => (
            "skills/list",
            serde_json::json!({"cwds":[project],"forceReload":false}),
        ),
        CatalogKind::Mcp | CatalogKind::Tools => (
            "mcpServerStatus/list",
            serde_json::json!({"limit":128,"detail":"toolsAndAuthOnly"}),
        ),
        CatalogKind::Plugins => (
            "plugin/list",
            serde_json::json!({"cwds":[project],"forceRefetch":false,"marketplaceKinds":["local"]}),
        ),
        CatalogKind::Hooks => ("hooks/list", serde_json::json!({"cwds":[project]})),
        CatalogKind::Apps => (
            "app/list",
            serde_json::json!({"limit":128,"forceRefetch":false}),
        ),
    };
    let responses = codex_rpc(executable, project, &[(1, method, params)])?;
    let result = responses
        .get(&1)
        .and_then(|r| r.get("result"))
        .context("The installed Codex did not provide this catalog")?;
    let mut items = Vec::new();
    let clean = |value: &str| {
        value
            .chars()
            .filter(|c| !c.is_control())
            .take(512)
            .collect::<String>()
    };
    let mut add = |value: &Value, detail: String| {
        if items.len() < 128
            && let Some(name) = value.get("name").and_then(Value::as_str)
        {
            items.push(CatalogItem {
                label: clean(name),
                detail: clean(&detail),
            });
        }
    };
    match kind {
        CatalogKind::Skills => {
            if let Some(data) = result.get("data").and_then(Value::as_array) {
                for entry in data.iter().take(16) {
                    if let Some(skills) = entry.get("skills").and_then(Value::as_array) {
                        for skill in skills.iter().take(128) {
                            let description = skill
                                .get("shortDescription")
                                .or_else(|| skill.get("description"))
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            add(
                                skill,
                                format!(
                                    "{} · {description}",
                                    if skill.get("enabled") == Some(&Value::Bool(true)) {
                                        "Enabled"
                                    } else {
                                        "Disabled"
                                    }
                                ),
                            );
                        }
                    }
                }
            }
        }
        CatalogKind::Mcp => {
            if let Some(data) = result.get("data").and_then(Value::as_array) {
                for server in data.iter().take(128) {
                    let tools = server
                        .get("tools")
                        .and_then(Value::as_object)
                        .map_or(0, |v| v.len());
                    add(
                        server,
                        format!(
                            "{} · {tools} tools",
                            server
                                .get("authStatus")
                                .and_then(Value::as_str)
                                .unwrap_or("Auth unknown")
                        ),
                    );
                }
            }
        }
        CatalogKind::Tools => {
            for server in result["data"].as_array().into_iter().flatten().take(128) {
                let server_name = server["name"].as_str().unwrap_or("MCP");
                for (name, tool) in server["tools"].as_object().into_iter().flatten().take(128) {
                    let label = format!("{} · {}", clean(server_name), clean(name));
                    let description = tool["description"].as_str().unwrap_or("");
                    let properties = tool
                        .pointer("/inputSchema/properties")
                        .and_then(Value::as_object);
                    let fields = properties
                        .map(|fields| {
                            fields
                                .keys()
                                .take(12)
                                .map(|name| clean(name))
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    let detail = format!(
                        "{}{}{}",
                        clean(description),
                        if fields.is_empty() {
                            ""
                        } else {
                            " · Inputs: "
                        },
                        fields
                    );
                    add(&serde_json::json!({"name":label}), detail);
                }
            }
        }
        CatalogKind::Hooks => {
            for entry in result["data"].as_array().into_iter().flatten().take(16) {
                for hook in entry["hooks"].as_array().into_iter().flatten().take(128) {
                    let label = format!(
                        "{} · {}",
                        clean(hook["eventName"].as_str().unwrap_or("Hook")),
                        clean(hook["key"].as_str().unwrap_or("Configured"))
                    );
                    let detail = format!(
                        "{} · {} · {}",
                        if hook["enabled"] == true {
                            "Enabled"
                        } else {
                            "Disabled"
                        },
                        clean(hook["trustStatus"].as_str().unwrap_or("Trust unknown")),
                        clean(
                            hook["statusMessage"]
                                .as_str()
                                .or_else(|| hook["handlerType"].as_str())
                                .unwrap_or("")
                        )
                    );
                    add(&serde_json::json!({"name":label}), detail);
                }
            }
        }
        CatalogKind::Apps => {
            for app in result["data"].as_array().into_iter().flatten().take(128) {
                add(
                    app,
                    format!(
                        "{} · {} · {}",
                        if app["isAccessible"] == true {
                            "Accessible"
                        } else {
                            "Access not available"
                        },
                        if app["isEnabled"] == true {
                            "Enabled"
                        } else {
                            "Disabled"
                        },
                        clean(app["description"].as_str().unwrap_or(""))
                    ),
                );
            }
        }
        CatalogKind::Plugins => {
            if let Some(marketplaces) = result.get("marketplaces").and_then(Value::as_array) {
                for market in marketplaces.iter().take(16) {
                    if let Some(plugins) = market.get("plugins").and_then(Value::as_array) {
                        for plugin in plugins.iter().take(128) {
                            add(
                                plugin,
                                if plugin.get("installed") == Some(&Value::Bool(true)) {
                                    if plugin.get("enabled") == Some(&Value::Bool(true)) {
                                        "Installed · enabled"
                                    } else {
                                        "Installed · disabled"
                                    }
                                } else {
                                    "Available locally"
                                }
                                .into(),
                            );
                        }
                    }
                }
            }
        }
    }
    let discovery_errors = match kind {
        CatalogKind::Skills | CatalogKind::Hooks => result
            .get("data")
            .and_then(Value::as_array)
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry
                        .get("errors")
                        .and_then(Value::as_array)
                        .is_some_and(|errors| !errors.is_empty())
                })
            }),
        CatalogKind::Plugins => result
            .get("marketplaceLoadErrors")
            .and_then(Value::as_array)
            .is_some_and(|errors| !errors.is_empty()),
        CatalogKind::Mcp | CatalogKind::Tools => result
            .get("data")
            .and_then(Value::as_array)
            .is_some_and(|servers| {
                servers.iter().any(|server| {
                    server
                        .get("toolsError")
                        .is_some_and(|error| !error.is_null())
                })
            }),
        CatalogKind::Apps => false,
    };
    if discovery_errors {
        if items.is_empty() {
            bail!("Native catalog discovery reported errors");
        }
        items.push(CatalogItem {
            label: "Partial inventory".into(),
            detail:
                "Codex reported discovery errors for some entries. Other entries are preserved."
                    .into(),
        });
    }
    if items.len() >= 128 || result.get("nextCursor").is_some_and(|v| !v.is_null()) {
        items.push(CatalogItem {
            label: "Bounded inventory".into(),
            detail: "Showing at most 128 entries. Additional entries remain available in the native CLI.".into(),
        });
    }
    Ok(items)
}

/// Create the durable provider branch before exposing it as an editable chat.
pub(super) fn fork_native_thread(executable:&Path,project:&Path,id:&str)->Result<String>{
    let records=codex_rpc(executable,project,&[(1,"thread/fork",serde_json::json!({"threadId":id,"cwd":project,"excludeTurns":true,"ephemeral":false}))])?;
    let record=records.get(&1).context("Codex did not acknowledge the branch")?;
    if let Some(error)=record.get("error"){bail!("Codex could not branch this chat: {}",error["message"].as_str().unwrap_or("provider rejected the branch"))}
    let fork=record["result"]["thread"]["id"].as_str().context("Codex returned no branch ID")?;
    anyhow::ensure!(fork!=id,"Codex returned the source ID instead of a branch");Ok(fork.into())
}

fn codex_rpc(
    executable: &Path,
    home: &Path,
    requests: &[(u64, &str, Value)],
) -> Result<std::collections::BTreeMap<u64, Value>> {
    let mut command = Command::new(executable);
    crate::process_env::apply(&mut command);
    command
        .args(["app-server", "--listen", "stdio://"])
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = ProbeChild(command.spawn().context("Codex metadata unavailable")?);
    let mut input = child
        .0
        .stdin
        .take()
        .context("Codex RPC input unavailable")?;
    let mut output = child
        .0
        .stdout
        .take()
        .context("Codex RPC output unavailable")?;
    let fd = output.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        bail!("Codex RPC output unavailable");
    }
    let write = |input: &mut std::process::ChildStdin, value: Value| -> Result<()> {
        serde_json::to_writer(&mut *input, &value)?;
        input.write_all(b"\n")?;
        input.flush()?;
        Ok(())
    };
    write(
        &mut input,
        serde_json::json!({"id":0,"method":"initialize","params":{
        "clientInfo":{"name":"code_native_controls","version":env!("CARGO_PKG_VERSION")},
        "capabilities":{"experimentalApi":true}}}),
    )?;
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut pending = Vec::<u8>::new();
    let mut total = 0usize;
    let mut initialized = false;
    let mut closed = false;
    let mut responses = std::collections::BTreeMap::<u64, Value>::new();
    while Instant::now() < deadline {
        let mut buffer = [0u8; 8192];
        match output.read(&mut buffer) {
            Ok(0) => {
                closed = true;
                break;
            }
            Ok(count) => {
                total += count;
                if total > METADATA_BYTES as usize {
                    if !responses.is_empty() {
                        break;
                    }
                    bail!("Codex metadata exceeded bounds");
                }
                pending.extend_from_slice(&buffer[..count]);
                while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                    let record = serde_json::from_slice::<Value>(&pending[..end]);
                    pending.drain(..=end);
                    let Ok(record) = record else {
                        continue;
                    };
                    // Never execute server requests while inspecting metadata.
                    if record.get("method").is_some() {
                        if let Some(id) = record.get("id").filter(|id| {
                            id.is_u64() || id.as_str().is_some_and(|id| id.len() <= 128)
                        }) {
                            write(
                                &mut input,
                                serde_json::json!({"id":id,"error":{"code":-32601,"message":"Metadata client does not execute tools"}}),
                            )?;
                        }
                        continue;
                    }
                    let Some(id) = record.get("id").and_then(Value::as_u64) else {
                        continue;
                    };
                    if id == 0 && !initialized {
                        if record.get("error").is_some() {
                            bail!("Codex metadata handshake rejected");
                        }
                        initialized = true;
                        write(&mut input, serde_json::json!({"method":"initialized"}))?;
                        for (id, method, params) in requests {
                            write(
                                &mut input,
                                serde_json::json!({"id":id,"method":method,"params":params}),
                            )?;
                        }
                    } else if requests.iter().any(|(expected, _, _)| *expected == id) {
                        responses.insert(id, record);
                    }
                }
                if responses.len() == requests.len() {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let mut poll = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                unsafe {
                    libc::poll(&mut poll, 1, 100);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                if !responses.is_empty() {
                    break;
                }
                return Err(error.into());
            }
        }
    }
    if !initialized || responses.is_empty() {
        if closed {
            bail!("Codex App Server closed before returning metadata");
        }
        bail!("Codex metadata check timed out");
    }
    Ok(responses)
}

fn codex_snapshot(responses: &std::collections::BTreeMap<u64, Value>) -> ControlInfo {
    let mut info = ControlInfo {
        defaults:Default::default(),
        auth: AuthState::Unknown,
        models: Vec::new(),
        note: None,
        account: None,
        limits: Vec::new(),
        catalog: None,
    };
    if let Some(account) = responses.get(&1).and_then(|v| v.get("result")) {
        info.auth = match account.pointer("/account/type").and_then(Value::as_str) {
            Some("chatgpt" | "apiKey" | "amazonBedrock") => AuthState::VerifiedSignedIn,
            _ if account.get("account").is_some_and(Value::is_null)
                && account.get("requiresOpenaiAuth") == Some(&Value::Bool(false)) =>
            {
                AuthState::Configured
            }
            _ if account.get("account").is_some_and(Value::is_null) => AuthState::SignedOut,
            _ => AuthState::Unknown,
        };
        info.account = account
            .pointer("/account/planType")
            .or_else(|| account.pointer("/account/type"))
            .and_then(Value::as_str)
            .map(|v| {
                v.chars()
                    .filter(|c| !c.is_control())
                    .take(80)
                    .collect::<String>()
            })
            .filter(|value| !value.trim().is_empty() && value != "unknown");
    }
    if let Some(models) = responses
        .get(&2)
        .and_then(|v| v.pointer("/result/data"))
        .and_then(Value::as_array)
    {
        for model in models.iter().take(128) {
            if model.get("hidden").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            if let Some(id) = model
                .get("model")
                .or_else(|| model.get("id"))
                .and_then(Value::as_str)
            {
                add_model(
                    &mut info.models,
                    id,
                    model
                        .get("displayName")
                        .and_then(Value::as_str)
                        .unwrap_or(id),
                );
                if let Some(choice)=info.models.iter_mut().find(|m|m.id==id){
                    choice.reasoning_efforts=model["supportedReasoningEfforts"].as_array().into_iter().flatten().filter_map(|v|v["reasoningEffort"].as_str()).filter(|v|matches!(*v,"none"|"minimal"|"low"|"medium"|"high"|"xhigh"|"max"|"ultra")).map(str::to_owned).collect();
                }
            }
        }
    }
    if let Some(models)=responses.get(&2).and_then(|r|r.pointer("/result/data")).and_then(Value::as_array){
        for model in models{
            if let Some(id)=model.get("model").or_else(||model.get("id")).and_then(Value::as_str).filter(|id|valid_model_id(id)){
                if let Some(tier)=["serviceTiers","additionalSpeedTiers"].iter().flat_map(|key|model[*key].as_array().into_iter().flatten()).filter_map(|v|v.as_str().or_else(||v["id"].as_str())).find(|id|matches!(*id,"fast"|"priority")){info.defaults.fast_models.insert(id.into(),tier.into());}
                if model["isDefault"]==true{info.defaults.model=Some(id.into());}
                if let Some(effort)=model["defaultReasoningEffort"].as_str().filter(|v|valid_model_id(v)){info.defaults.efforts.insert(id.into(),effort.into());}
            }
        }
    }
    if let Some(config)=responses.get(&4).and_then(|r|r.pointer("/result/config")){
        if let Some(id)=config["model"].as_str().filter(|id|valid_model_id(id)){info.defaults.model=Some(id.into());}
        info.defaults.effort=config["model_reasoning_effort"].as_str().filter(|v|valid_model_id(v)).map(str::to_owned);
    }
    if let Some(result) = responses.get(&3).and_then(|v| v.get("result")) {
        info.limits = crate::usage::codex_limit_windows(result, false);
        if let Some(raw)=result.get("rateLimitResetCredits").filter(|v|v["availableCount"].as_u64().is_some()){
            let text=|v:&Value,key:&str|v[key].as_str().map(|s|s.chars().filter(|c|!c.is_control()).take(500).collect::<String>());
            let credits:Vec<_>=raw["credits"].as_array().into_iter().flatten().take(32).filter(|c|c["id"].as_str().is_some_and(|id|valid_model_id(id))).map(|c|serde_json::json!({"id":c["id"],"title":text(c,"title"),"description":text(c,"description"),"expiresAt":c["expiresAt"].as_i64().or_else(||c["expiresAt"].as_str().and_then(|s|chrono::DateTime::parse_from_rfc3339(s).ok()).map(|t|t.timestamp())),"status":c["status"]})).collect();
            info.defaults.reset_credits=Some(serde_json::json!({"availableCount":raw["availableCount"],"credits":credits}));
        }
    }

    let missing: Vec<_> = [(1, "account"), (2, "models"), (3, "rate limits")]
        .into_iter()
        .filter(|(id, _)| {
            responses
                .get(id)
                .is_none_or(|v| v.get("result").is_none_or(Value::is_null))
        })
        .map(|(_, label)| label)
        .collect();
    if !missing.is_empty() {
        info.note = Some(format!(
            "Codex did not return {}. Other live metadata remains available.",
            missing.join(", ")
        ));
    }
    info
}
pub fn consume_reset(executable:&Path,project:&Path,id:&str,key:&str)->Result<String>{
    anyhow::ensure!(valid_model_id(id),"Invalid reset credit");
    let responses=codex_rpc(executable,project,&[(1,"account/rateLimitResetCredit/consume",serde_json::json!({"creditId":id,"idempotencyKey":key}))])?;
    let reply=responses.get(&1).context("Reset response missing")?;
    let outcome=reply["result"]["outcome"].as_str().context("Reset was not accepted")?;
    anyhow::ensure!(matches!(outcome,"reset"|"nothingToReset"|"noCredit"|"alreadyRedeemed"),"Unexpected reset result");Ok(outcome.into())
}

/// Native SDK initialize returns a model catalog without creating a prompt.
/// Keep this metadata process separate from the active chat's control channel.
fn claude_models(executable: &Path, home: &Path) -> Result<(Vec<ModelChoice>,ModelDefaults)> {
    let mut command = Command::new(executable);
    crate::process_env::apply(&mut command);
    command
        .args([
            "--print",
            "--verbose",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            "default",
        ])
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = ProbeChild(
        command
            .spawn()
            .context("Claude model catalog unavailable")?,
    );
    let mut input = child
        .0
        .stdin
        .take()
        .context("Claude catalog input unavailable")?;
    input.write_all(b"{\"type\":\"control_request\",\"request_id\":\"code-models\",\"request\":{\"subtype\":\"initialize\",\"hooks\":null}}\n")?;
    input.flush()?;
    let mut output = child
        .0
        .stdout
        .take()
        .context("Claude catalog output unavailable")?;
    let fd = output.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        bail!("Claude catalog output unavailable");
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut pending = Vec::new();
    let mut total = 0usize;
    let mut buffer = [0u8; 8192];
    loop {
        if Instant::now() >= deadline {
            bail!("Claude model catalog timed out");
        }
        match output.read(&mut buffer) {
            Ok(0) => bail!("Claude catalog closed before initialization"),
            Ok(n) => {
                total += n;
                if total > METADATA_BYTES as usize {
                    bail!("Claude catalog exceeded bounds");
                }
                pending.extend_from_slice(&buffer[..n]);
                while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                    let value: Value = serde_json::from_slice(&pending[..end])?;
                    pending.drain(..=end);
                    if value["type"] == "control_response"
                        && value["response"]["request_id"] == "code-models"
                    {
                        if value["response"]["subtype"] != "success" {
                            bail!("Claude catalog initialization rejected");
                        }
                        let mut models = Vec::new();let mut defaults=ModelDefaults::default();
                        if let Some(items) = value
                            .pointer("/response/response/models")
                            .and_then(Value::as_array)
                        {
                            for item in items.iter().take(MODEL_LIMIT) {
                                if let Some(id) = item["value"].as_str() {
                                    let resolved=item["resolvedModel"].as_str().filter(|id|valid_model_id(id));
                                    add_model(&mut models,id,resolved.unwrap_or_else(||item["displayName"].as_str().unwrap_or(id)));
                                    if let Some(resolved)=resolved{defaults.aliases.insert(id.into(),resolved.into());if id=="default"{defaults.model=Some(resolved.into());}}
                                    if let Some(choice)=models.iter_mut().find(|m|m.id==id){choice.reasoning_efforts=item["supportedEffortLevels"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|v|["low","medium","high","xhigh","max"].contains(v)).take(8).map(str::to_owned).collect();}
                                }
                            }
                        }
                        return Ok((models,defaults));
                    }
                    // No tools, hooks or SDK MCP callbacks are registered by
                    // this metadata client. Never leave unsolicited requests hanging.
                    if value["type"] == "control_request"
                        && let Some(id) = value["request_id"].as_str().filter(|id| id.len() <= 128)
                    {
                        serde_json::to_writer(
                            &mut input,
                            &serde_json::json!({"type":"control_response","response":{"subtype":"error","request_id":id,"error":"Metadata inspection does not execute tools"}}),
                        )?;
                        input.write_all(b"\n")?;
                        input.flush()?;
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let mut poll = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                unsafe {
                    libc::poll(&mut poll, 1, 100);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

fn metadata(path: &Path) -> Option<Value> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() > METADATA_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(METADATA_BYTES + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > METADATA_BYTES {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// CLI help is versioned evidence. Import its quoted examples/aliases instead
/// of maintaining a list of model names that goes stale with CLI releases.
fn advertised_model_names(line: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find(['\'', '\"']) {
        let quote = rest.as_bytes()[start] as char;
        rest = &rest[start + 1..];
        let Some(end) = rest.find(quote) else {
            break;
        };
        let id = &rest[..end];
        if valid_model_id(id) && !values.iter().any(|value| value == id) {
            values.push(id.to_owned());
        }
        rest = &rest[end + 1..];
        if values.len() == MODEL_LIMIT {
            break;
        }
    }
    values
}

fn configured_model(models: &mut Vec<ModelChoice>, path: &Path, pointers: &[&str]) {
    if let Some(settings) = metadata(path) {
        for pointer in pointers {
            if let Some(id) = settings.pointer(pointer).and_then(Value::as_str) {
                add_model(models, id, id);
            }
        }
    }
}

fn add_model(models: &mut Vec<ModelChoice>, id: &str, label: &str) {
    if models.len() < MODEL_LIMIT && valid_model_id(id) && !models.iter().any(|m| m.id == id) {
        let label: String = label.chars().filter(|c| !c.is_control()).take(80).collect();
        models.push(ModelChoice { reasoning_efforts:vec![],
            id: id.into(),
            label: if label.is_empty() { id.into() } else { label },
        });
    }
}

fn catalog_lines(kind: AgentKind, bytes: &[u8]) -> Vec<ModelChoice> {
    let text = strip_ansi(bytes);
    let mut models = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let line = if kind == AgentKind::Grok {
            let Some(line) = line.strip_prefix("* ").or_else(|| line.strip_prefix("- ")) else {
                continue;
            };
            line
        } else {
            line
        };
        let Some(id) = line.split_whitespace().next() else {
            continue;
        };
        let accepted = match kind {
            AgentKind::OpenCode => line == id && id.contains('/'),
            AgentKind::Cursor => line.contains(" - "),
            AgentKind::Grok => id.starts_with("grok-"),
            _ => false,
        };
        if accepted {
            let label = if kind == AgentKind::Cursor {
                line.split_once(" - ").map(|(_, label)| label).unwrap_or(id)
            } else {
                id
            };
            add_model(&mut models, id, label);
        }
    }
    models
}

fn auth_state(kind: AgentKind, output: &ProbeOutput) -> AuthState {
    let json: Option<Value> = serde_json::from_slice(&output.stdout).ok();
    match kind {
        AgentKind::Claude => match json
            .as_ref()
            .and_then(|v| v.get("loggedIn"))
            .and_then(Value::as_bool)
        {
            Some(true) if output.success => AuthState::VerifiedSignedIn,
            Some(false) => AuthState::SignedOut,
            _ => AuthState::Unknown,
        },
        AgentKind::Cursor => match json
            .as_ref()
            .and_then(|v| v.get("status"))
            .and_then(Value::as_str)
        {
            Some("authenticated") if output.success => AuthState::VerifiedSignedIn,
            Some("unauthenticated") => AuthState::SignedOut,
            Some("partially-authenticated") => AuthState::Configured,
            _ => AuthState::Unknown,
        },
        AgentKind::Codex => {
            let text = format!(
                "{}{}",
                strip_ansi(&output.stdout),
                strip_ansi(&output.stderr)
            )
            .to_lowercase();
            if text.contains("not logged in") {
                AuthState::SignedOut
            } else if output.success && text.contains("logged in") {
                AuthState::VerifiedSignedIn
            } else {
                AuthState::Unknown
            }
        }
        AgentKind::OpenCode if output.success => {
            let text = strip_ansi(&output.stdout).to_lowercase();
            // Native list summarizes counts; no provider names/credentials retained.
            let mut found_count = false;
            for line in text.lines() {
                let words: Vec<_> = line.split_whitespace().collect();
                for pair in words.windows(2) {
                    if (pair[1].starts_with("credential") || pair[1] == "environment")
                        && let Ok(count) = pair[0].parse::<usize>()
                    {
                        found_count = true;
                        if count > 0 {
                            return AuthState::Configured;
                        }
                    }
                }
            }
            if found_count {
                AuthState::SignedOut
            } else {
                AuthState::Unknown
            }
        }
        _ => AuthState::Unknown,
    }
}

fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else if !c.is_control() || c == '\n' || c == '\t' {
            result.push(c);
        }
    }
    result
}

struct ProbeOutput {
    success: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Kill the entire probe group before reaping, including children holding pipes.
struct ProbeChild(std::process::Child);
impl Drop for ProbeChild {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.wait();
    }
}

#[cfg(test)]
pub(crate) fn archive_test_threads(executable:&Path,root:&Path,ids:&[String])->Result<()>{
    let root=root.canonicalize()?;anyhow::ensure!(root.parent()==Some(Path::new("/private/tmp"))&&root.file_name().and_then(|s|s.to_str()).is_some_and(|s|s.starts_with("kai-agent-ui-")),"Not a probe workspace");
    let reads:Vec<_>=ids.iter().enumerate().map(|(i,id)|(i as u64+1,"thread/read",serde_json::json!({"threadId":id,"includeTurns":false}))).collect();
    let values=codex_rpc(executable,&root,&reads)?;
    for (i,id) in ids.iter().enumerate(){let thread=&values[&(i as u64+1)]["result"]["thread"];anyhow::ensure!(thread["id"]==*id&&thread["cwd"].as_str().is_some_and(|cwd|Path::new(cwd).canonicalize().ok().as_ref()==Some(&root)),"Probe thread ownership mismatch");}
    let archives:Vec<_>=ids.iter().enumerate().map(|(i,id)|(i as u64+1,"thread/archive",serde_json::json!({"threadId":id}))).collect();let results=codex_rpc(executable,&root,&archives)?;for value in results.values(){anyhow::ensure!(value.get("error").is_none(),"Could not archive owned probe thread");}Ok(())
}

/// One ephemeral, read-only request. No native user thread or transcript is
/// created, and user MCP servers/hooks/config are not loaded into title work.
pub fn generate_chat_title(executable:&Path,model:&str,prompt:&str,cancel:&std::sync::atomic::AtomicBool)->Option<String>{
    if !valid_model_id(model){return None}
    struct Scratch(PathBuf);impl Drop for Scratch{fn drop(&mut self){let _=fs::remove_dir_all(&self.0);}}
    let scratch=Scratch(std::env::temp_dir().join(format!("kai-title-{}",uuid::Uuid::new_v4())));
    fs::create_dir(&scratch.0).ok()?;
    let request=crate::chat_titles::prompt(prompt);
    let output=probe_with_limit_cancel(executable,&["exec","--ephemeral","--ignore-user-config","--ignore-rules","--sandbox","read-only","--skip-git-repo-check","--json","--color","never","--model",model,"--config","approval_policy=\"never\"","--config","model_provider=\"kai-openai-https\"","--config","model_providers.kai-openai-https.name=\"OpenAI\"","--config","model_providers.kai-openai-https.wire_api=\"responses\"","--config","model_providers.kai-openai-https.requires_openai_auth=true","--config","model_providers.kai-openai-https.supports_websockets=false",&request],&scratch.0,Duration::from_secs(45),128*1024,Some(cancel)).ok()?;
    if !output.success{return None}
    let mut result=None;
    for line in output.stdout.split(|b|*b==b'\n'){
        if let Ok(value)=serde_json::from_slice::<Value>(line){if value["type"]=="item.completed"&&value["item"]["type"]=="agent_message"{result=value["item"]["text"].as_str().and_then(crate::chat_titles::parse);}}
    }result
}
fn probe(executable: &Path, args: &[&str], home: &Path, timeout: Duration) -> Result<ProbeOutput> {
    probe_with_limit(executable,args,home,timeout,PROBE_BYTES)
}
fn probe_with_limit(executable:&Path,args:&[&str],home:&Path,timeout:Duration,limit:usize)->Result<ProbeOutput>{
    probe_with_limit_cancel(executable,args,home,timeout,limit,None)
}
fn probe_with_limit_cancel(executable:&Path,args:&[&str],home:&Path,timeout:Duration,limit:usize,cancel:Option<&std::sync::atomic::AtomicBool>)->Result<ProbeOutput>{
    let mut command = Command::new(executable);
    crate::process_env::apply(&mut command);
    command
        .args(args)
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    // Catalog lookups need neither downloads nor project plugin initialization.
    command
        .env("NO_COLOR", "1")
        .env("OPENCODE_DISABLE_MODELS_FETCH", "true");
    let mut child = ProbeChild(command.spawn().context("CLI status unavailable")?);
    let mut stdout = child.0.stdout.take().context("CLI output unavailable")?;
    let mut stderr = child
        .0
        .stderr
        .take()
        .context("CLI diagnostics unavailable")?;
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            bail!("CLI output unavailable");
        }
    }
    let deadline = Instant::now() + timeout;
    let mut output = ProbeOutput {
        success: false,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    let mut eof = [false, false];
    loop {
        if cancel.is_some_and(|c|c.load(std::sync::atomic::Ordering::Acquire)){bail!("Title request cancelled")}
        for (index, (reader, bytes)) in [
            (&mut stdout as &mut dyn Read, &mut output.stdout),
            (&mut stderr as &mut dyn Read, &mut output.stderr),
        ]
        .into_iter()
        .enumerate()
        {
            if eof[index] {
                continue;
            }
            let mut buffer = [0_u8; 8192];
            for _ in 0..4 {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        eof[index] = true;
                        break;
                    }
                    Ok(count) => {
                        if bytes.len() + count > if index==0{limit}else{PROBE_BYTES} {
                            bail!("CLI status exceeded its output limit");
                        }
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => bail!("CLI status output unavailable"),
                }
            }
        }
        // Use waitid(WNOWAIT) so the PID cannot be reused before group cleanup.
        let mut status: libc::siginfo_t = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::waitid(
                libc::P_PID,
                child.0.id(),
                &mut status,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } == 0
            && status.si_pid != 0
            && eof.iter().all(|eof| *eof)
        {
            output.success = status.si_code == libc::CLD_EXITED && status.si_status == 0;
            return Ok(output);
        }
        let now = Instant::now();
        if now >= deadline {
            bail!("CLI status check timed out");
        }
        let mut fds = [
            libc::pollfd {
                fd: if eof[0] { -1 } else { stdout.as_raw_fd() },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if eof[1] { -1 } else { stderr.as_raw_fd() },
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let wait_ms = (deadline - now).as_millis().clamp(1, 100) as i32;
        unsafe {
            libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, wait_ms);
        }
    }
}

/// Creates a private, safely quoted command for Terminal; never starts it here.
/// A verified sign-in is left alone. No logout command exists in this module.
pub fn login_script(
    kind: AgentKind,
    executable: &Path,
    cache_dir: &Path,
    auth: AuthState,
) -> Result<Option<PathBuf>> {
    if auth == AuthState::VerifiedSignedIn {
        return Ok(None);
    }
    if !executable.is_absolute() || !executable.is_file() {
        bail!("Agent executable is unavailable");
    }
    let args: &[&str] = match kind {
        AgentKind::Codex | AgentKind::Cursor | AgentKind::Grok => &["login"],
        AgentKind::Claude => &["auth", "login"],
        AgentKind::OpenCode => &["auth", "login"],
        // Native interactive startup owns their browser authentication flow.
        AgentKind::Gemini | AgentKind::Antigravity => &[],
    };
    let executable = executable
        .to_str()
        .context("Agent executable path is not UTF-8")?;
    if executable.chars().any(|c| c.is_control()) {
        bail!("Agent executable path is invalid");
    }
    let mut native_command = Command::new(executable);
    crate::process_env::apply(&mut native_command);
    let native_path = native_command
        .get_envs()
        .find(|(key, _)| *key == std::ffi::OsStr::new("PATH"))
        .and_then(|(_, value)| value)
        .and_then(std::ffi::OsStr::to_str)
        .context("Native CLI PATH is unavailable")?;
    fs::create_dir_all(cache_dir)?;
    let path = cache_dir.join(format!(
        "{}-{}.command",
        kind.command(),
        uuid::Uuid::new_v4()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)?;
    writeln!(
        file,
        "#!/bin/sh\n# Authentication stays in the native CLI.\nPATH={}\nexport PATH\ncd \"$HOME\" || exit 1\nexec {} {}",
        shell_quote(native_path),
        shell_quote(executable),
        args.iter()
            .map(|arg| shell_quote(arg))
            .collect::<Vec<_>>()
            .join(" ")
    )?;
    Ok(Some(path))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("orbit-controls-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn cli(&self, body: &str) -> PathBuf {
            let path = self.0.join("native ' fixture");
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn codex_metadata_uses_rpc_auth_catalog_and_all_rate_limit_windows() {
        let fixture = Fixture::new();
        let cli=fixture.cli(r#"
read -r initialize
printf '%s\n' '{"id":0,"result":{"userAgent":"fixture"}}'
read -r initialized
read -r account
read -r models
read -r limits
printf '%s\n' '{"id":2,"result":{"data":[{"id":"picker-id","model":"native-id","displayName":"Native model","hidden":false},{"model":"hidden-id","hidden":true}]}}'
printf '%s\n' '{"id":1,"result":{"account":{"type":"chatgpt","email":"private@example.test","planType":"pro"}}}'
printf '%s\n' '{"id":3,"result":{"rateLimits":{"primary":{"usedPercent":99}},"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":1800000000},"secondary":{"usedPercent":42,"windowDurationMins":10080}}}}}'
"#);
        let info = inspect(AgentKind::Codex, &cli, &fixture.0);
        assert_eq!(info.auth, AuthState::VerifiedSignedIn);
        assert_eq!(info.account.as_deref(), Some("pro"));
        assert_eq!(
            info.models,
            vec![ModelChoice { reasoning_efforts:vec![],
                id: "native-id".into(),
                label: "Native model".into()
            }]
        );
        assert_eq!(info.limits.len(), 2);
        assert_eq!(info.limits[0].used_percent, 0.);
        assert_eq!(info.limits[1].duration_minutes, Some(10080));
        assert!(!format!("{info:?}").contains("private@example"));
        assert!(info.note.is_none());
    }

    #[test]
    fn codex_catalogs_use_genuine_rpc_shapes_and_keep_project_scope() {
        let fixture = Fixture::new();
        for (kind, result, expected) in [
            (
                CatalogKind::Skills,
                r#"{"data":[{"cwd":"/project","skills":[{"name":"review","enabled":true,"description":"Inspect a change"}]}]}"#,
                "review",
            ),
            (
                CatalogKind::Mcp,
                r#"{"data":[{"name":"native-tools","authStatus":"oAuth","tools":{"inspect":{}}}]}"#,
                "native-tools",
            ),
            (
                CatalogKind::Plugins,
                r#"{"marketplaces":[{"name":"local","plugins":[{"name":"helper","installed":true,"enabled":false}]}]}"#,
                "helper",
            ),
            (
                CatalogKind::Tools,
                r#"{"data":[{"name":"native-tools","tools":{"inspect":{"description":"Inspect safely","inputSchema":{"properties":{"path":{"type":"string"}}}}}}]}"#,
                "native-tools · inspect",
            ),
            (
                CatalogKind::Hooks,
                r#"{"data":[{"hooks":[{"eventName":"preToolUse","key":"check","enabled":true,"trustStatus":"trusted","handlerType":"command"}],"errors":[] }]}"#,
                "preToolUse · check",
            ),
            (
                CatalogKind::Apps,
                r#"{"data":[{"name":"Connected app","isEnabled":true,"isAccessible":true,"description":"Native app"}],"nextCursor":null}"#,
                "Connected app",
            ),
        ] {
            let body = format!(
                "read -r initialize\nprintf '%s\\n' '{{\"id\":0,\"result\":{{}}}}'\nread -r initialized\nread -r query\nprintf '%s\\n' '{{\"id\":1,\"result\":{result}}}'"
            );
            let cli = fixture.cli(&body);
            let items = inspect_catalog(&cli, &fixture.0, kind).unwrap();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].label, expected);
        }
    }

    #[test]
    fn codex_rpc_partial_failures_preserve_successes_and_reject_invalid_quotas() {
        let responses=[(1,serde_json::json!({"result":{"account":null}})),
            (2,serde_json::json!({"result":{"data":[{"model":"real-model","displayName":"Real"}]}})),
            (3,serde_json::json!({"error":{"message":"private-token"}}))].into_iter().collect();
        let info = codex_snapshot(&responses);
        assert_eq!(info.auth, AuthState::SignedOut);
        assert_eq!(info.models.len(), 1);
        assert!(info.note.unwrap().contains("rate limits"));
        let responses=[(3,serde_json::json!({"result":{"rateLimits":{"primary":{"usedPercent":-1},"secondary":{"usedPercent":101}}}}))].into_iter().collect();
        assert!(codex_snapshot(&responses).limits.is_empty());
    }

    #[test]
    #[ignore = "Read-only installed Codex metadata probe; opt in explicitly"]
    fn installed_codex_metadata_returns_native_catalog_and_account() {
        let executable = std::env::var_os("CODE_METADATA_CLI")
            .map(PathBuf::from)
            .expect("CODE_METADATA_CLI");
        let home = std::env::var_os("CODE_METADATA_HOME")
            .map(PathBuf::from)
            .expect("CODE_METADATA_HOME");
        let info = codex_metadata(&executable, &home).unwrap();
        assert!(
            !info.models.is_empty(),
            "installed model/list returned no models"
        );
        assert_ne!(
            info.auth,
            AuthState::Unknown,
            "installed account/read returned no account state"
        );
        eprintln!(
            "Native Codex metadata: {} models, {} limit windows; no account identifier logged",
            info.models.len(),
            info.limits.len()
        );
    }

    #[test]
    #[ignore="One short ephemeral Codex subscription title request; no user chat is created"]
    fn installed_ephemeral_title_is_generated_from_a_real_available_model(){
        let executable=PathBuf::from(std::env::var("CODE_METADATA_CLI").expect("CODE_METADATA_CLI"));let home=PathBuf::from(std::env::var("CODE_METADATA_HOME").expect("CODE_METADATA_HOME"));
        let info=codex_metadata(&executable,&home).unwrap();let model=info.models.iter().filter(|m|m.id.contains("-luna")).last().or_else(||info.models.first()).expect("actual available model");
        let started=Instant::now();let title=generate_chat_title(&executable,&model.id,"Make the native workspace glass readable and add simple keyboard controls. Preserve the existing voice and approval flows.",&Default::default()).expect("semantic title from ephemeral request");
        assert!(title.len()<160);eprintln!("KAI_TITLE_PROBE_OK model={} title={title:?} elapsed={:.2}s",model.id,started.elapsed().as_secs_f64());
    }
    #[test]
    fn claude_catalog_uses_live_sdk_model_values_without_inference() {
        let fixture = Fixture::new();
        let cli=fixture.cli(r#"read -r initialize
case "$initialize" in *'"subtype":"initialize"'*) ;; *) exit 8;; esac
printf '%s\n' '{"type":"control_response","response":{"request_id":"code-models","subtype":"success","response":{"models":[{"value":"future-model","displayName":"Future"},{"value":"--invalid"},{"value":"future-model"}]}}}'
if read -r unexpected_prompt; then
printf '%s' "$unexpected_prompt" > unexpected-prompt
fi
"#);
        let (models,_) = claude_models(&cli, &fixture.0).unwrap();
        assert_eq!(
            models,
            vec![ModelChoice { reasoning_efforts:vec![],
                id: "future-model".into(),
                label: "Future".into()
            }]
        );
        assert!(!fixture.0.join("unexpected-prompt").exists());
    }

    #[test]
    fn model_aliases_follow_installed_help_instead_of_a_static_vendor_list() {
        assert_eq!(
            advertised_model_names(
                "--model <model> latest alias 'new-alias' or \"fresh-model\"; 'two words' and '--flag' rejected"
            ),
            ["new-alias", "fresh-model"]
        );
    }

    #[test]
    fn status_requires_native_evidence_and_never_treats_failures_as_signed_in() {
        let output = |success, stdout: &str, stderr: &str| ProbeOutput {
            success,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        };
        assert_eq!(
            auth_state(
                AgentKind::Codex,
                &output(true, "", "Logged in using ChatGPT")
            ),
            AuthState::VerifiedSignedIn
        );
        assert_eq!(
            auth_state(AgentKind::Codex, &output(false, "", "Not logged in")),
            AuthState::SignedOut
        );
        assert_eq!(
            auth_state(AgentKind::Codex, &output(false, "Logged in", "failed")),
            AuthState::Unknown
        );
        assert_eq!(
            auth_state(AgentKind::Claude, &output(true, r#"{"loggedIn":true}"#, "")),
            AuthState::VerifiedSignedIn
        );
        assert_eq!(
            auth_state(
                AgentKind::Claude,
                &output(false, r#"{"loggedIn":false}"#, "")
            ),
            AuthState::SignedOut
        );
        assert_eq!(
            auth_state(
                AgentKind::Cursor,
                &output(
                    true,
                    r#"{"status":"unauthenticated","isAuthenticated":false}"#,
                    ""
                )
            ),
            AuthState::SignedOut
        );
        assert_eq!(
            auth_state(
                AgentKind::Cursor,
                &output(
                    true,
                    r#"{"status":"authenticated","isAuthenticated":true}"#,
                    ""
                )
            ),
            AuthState::VerifiedSignedIn
        );
        assert_eq!(
            auth_state(AgentKind::OpenCode, &output(true, "└  2 credentials\n", "")),
            AuthState::Configured
        );
        assert_eq!(
            auth_state(AgentKind::OpenCode, &output(true, "└  0 credentials\n", "")),
            AuthState::SignedOut
        );
        assert_eq!(
            auth_state(
                AgentKind::OpenCode,
                &output(true, "└  0 credentials\n└  1 environment variable\n", "")
            ),
            AuthState::Configured
        );
        assert_eq!(
            auth_state(
                AgentKind::OpenCode,
                &output(false, "└  2 credentials\n", "")
            ),
            AuthState::Unknown
        );
    }

    #[test]
    fn bounded_probe_times_out_and_kills_descendants_holding_pipes() {
        let fixture = Fixture::new();
        let cli = fixture.cli("sleep 30 &\nwait");
        let start = Instant::now();
        let error = probe(&cli, &[], &fixture.0, Duration::from_millis(70))
            .err()
            .unwrap();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn probe_output_is_bounded_and_sensitive_diagnostics_do_not_escape() {
        let fixture = Fixture::new();
        let cli = fixture.cli("while :; do printf 'private-secret-00000000000000000000000000000000000000000000000000000'; done");
        // Deadline behavior has a separate test. Leave scheduling headroom
        // while this test competes with large history fixtures.
        let error = probe(&cli, &[], &fixture.0, Duration::from_secs(5))
            .err()
            .unwrap();
        assert!(error.to_string().contains("output limit"));
        assert!(!error.to_string().contains("private-secret"));
        let cli = fixture.cli("printf 'private-secret' >&2\nexit 7");
        let info = inspect(AgentKind::Codex, &cli, &fixture.0);
        assert_eq!(info.auth, AuthState::Unknown);
        assert!(!format!("{info:?}").contains("private-secret"));
    }

    #[test]
    fn cursor_signed_out_does_not_initiate_model_login() {
        let fixture = Fixture::new();
        let cli = fixture.cli("if [ \"$1\" = status ]; then printf '{\"status\":\"unauthenticated\"}'; else printf 'unexpected command' > marker; fi");
        let info = inspect(AgentKind::Cursor, &cli, &fixture.0);
        assert_eq!(info.auth, AuthState::SignedOut);
        assert!(info.models.is_empty());
        assert!(!fixture.0.join("marker").exists());
        let cli=fixture.cli("if [ \"$1\" = status ]; then printf '{\"status\":\"authenticated\"}'; elif [ \"$1\" = models ]; then printf 'Available models\\nauto - Auto (current, default)\\nconcrete-model - Concrete model\\n'; else touch marker; fi");
        let info=inspect(AgentKind::Cursor,&cli,&fixture.0);
        assert_eq!(info.defaults.model.as_deref(),Some("auto"));
        assert!(!fixture.0.join("marker").exists(),"Model discovery must not create an ACP session");
    }

    #[test]
    fn only_visible_native_models_are_imported_with_ids_and_bounds() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.0.join(".codex")).unwrap();
        fs::write(fixture.0.join(".codex/models_cache.json"), r#"{"models":[{"slug":"native-model","display_name":"Native Model","visibility":"list"},{"slug":"hidden-model","visibility":"hide"},{"slug":"--invalid"},{"slug":"native-model"}]}"#).unwrap();
        let cli = fixture.cli("printf 'Logged in' >&2");
        let info = inspect(AgentKind::Codex, &cli, &fixture.0);
        assert_eq!(info.auth, AuthState::Unknown);
        assert!(info.note.as_ref().unwrap().contains("local model catalog"));
        assert_eq!(
            info.models,
            vec![ModelChoice { reasoning_efforts:vec![],
                id: "native-model".into(),
                label: "Native Model".into()
            }]
        );
        let bytes = (0..200)
            .map(|i| format!("provider/model-{i}\n"))
            .collect::<String>();
        assert_eq!(
            catalog_lines(AgentKind::OpenCode, bytes.as_bytes()).len(),
            MODEL_LIMIT
        );
        assert_eq!(
            catalog_lines(
                AgentKind::Grok,
                b"Default model: grok-current\n  * grok-current (default)\n  - grok-other\n"
            )
            .len(),
            2
        );
        assert_eq!(
            catalog_lines(
                AgentKind::Cursor,
                b"Available models\n\x1b[32mmodel-id\x1b[0m - Model name\nTip: use --model\n"
            )[0]
            .id,
            "model-id"
        );
    }

    #[test]
    fn gemini_and_agy_use_only_configured_models_without_auth_probes() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.0.join(".gemini/antigravity-cli")).unwrap();
        fs::write(
            fixture.0.join(".gemini/settings.json"),
            r#"{"model":{"name":"native-gemini"}}"#,
        )
        .unwrap();
        fs::write(
            fixture.0.join(".gemini/antigravity-cli/settings.json"),
            r#"{"model":"native-agy"}"#,
        )
        .unwrap();
        let cli = fixture.cli("touch marker");
        for (kind, id) in [
            (AgentKind::Gemini, "native-gemini"),
            (AgentKind::Antigravity, "native-agy"),
        ] {
            let info = inspect(kind, &cli, &fixture.0);
            assert_eq!(info.auth, AuthState::Unknown);
            assert_eq!(info.models[0].id, id);
        }
        assert!(!fixture.0.join("marker").exists());
    }

    #[test]
    fn login_scripts_quote_paths_remain_private_and_preserve_verified_accounts() {
        let fixture = Fixture::new();
        let cli = fixture.cli("printf '%s\\n' \"$@\"");
        assert!(
            login_script(
                AgentKind::Codex,
                &cli,
                &fixture.0,
                AuthState::VerifiedSignedIn
            )
            .unwrap()
            .is_none()
        );
        let script = login_script(AgentKind::Claude, &cli, &fixture.0, AuthState::SignedOut)
            .unwrap()
            .unwrap();
        assert_eq!(
            fs::metadata(&script).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let output = Command::new(&script).output().unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"auth\nlogin\n");
        assert!(!fs::read_to_string(&script).unwrap().contains("logout"));
    }
    #[test]
    fn partial_metadata_survives_malformed_later_records_and_missing_rpc_results() {
        let fixture = Fixture::new();
        let cli = fixture.cli(r#"read -r initialize
printf '%s\n' '{"id":0,"result":{}}'
read -r initialized
read -r account
read -r models
read -r limits
printf '%s\n' '{"id":1,"result":{"account":{"type":"chatgpt","planType":"pro"}}}' '{"id":2,"result":{"data":[{"model":"native-model"}]}}' 'not JSON' '{"id":3,"result":null}'
"#);
        let info = inspect(AgentKind::Codex, &cli, &fixture.0);
        assert_eq!(info.auth, AuthState::VerifiedSignedIn);
        assert_eq!(info.models.len(), 1);
        assert!(info.limits.is_empty());
        assert!(info.note.unwrap().contains("rate limits"));
        let partial = codex_snapshot(&[(1,serde_json::json!({"result":{"account":{"type":"chatgpt","planType":""}}})),(3,serde_json::json!({"result":{"rateLimits":{"primary":null,"secondary":{"usedPercent":0,"resetsAt":1800000000}}}}))].into_iter().collect());
        assert!(partial.account.is_none());
        assert_eq!(partial.limits.len(), 1);
        assert_eq!(partial.limits[0].resets_at, Some(1800000000));
    }
}

#[cfg(test)]mod effective_default_tests{
 use super::*;
 #[test]fn project_config_overrides_catalog_defaults_and_preserves_per_model_efforts(){
  let records=std::collections::BTreeMap::from([
   (2,serde_json::json!({"result":{"data":[{"model":"catalog-default","isDefault":true,"defaultReasoningEffort":"low"},{"model":"project-model","defaultReasoningEffort":"medium"}]}})),
   (4,serde_json::json!({"result":{"config":{"model":"project-model","model_reasoning_effort":"high"}}}))]);
  let info=codex_snapshot(&records);assert_eq!(info.defaults.model.as_deref(),Some("project-model"));assert_eq!(info.defaults.effort.as_deref(),Some("high"));assert_eq!(info.defaults.efforts.get("catalog-default").map(String::as_str),Some("low"));
 }
}

/// Inspect a model's ACP selectors without sending a prompt, running a tool,
/// changing a workspace policy, or writing the provider's configuration.
pub fn inspect_model_project(kind:AgentKind,executable:&Path,home:&Path,project:&Path,model:Option<&str>)->ControlInfo{
 let mut info=inspect_project(kind,executable,home,project);
 if kind==AgentKind::OpenCode {
  info.defaults.capabilities_model=model.map(str::to_owned);info.defaults.capabilities_checked=true;
  if let Some(provider)=model.or(info.defaults.model.as_deref()).and_then(|id|id.split_once('/').map(|(provider,_)|provider)).filter(|provider|valid_model_id(provider)){
   if let Ok(output)=probe_with_limit(executable,&["models",provider,"--pure","--verbose"],project,PROBE_TIMEOUT,METADATA_BYTES as usize){
    if output.success{for model in crate::provider_defaults::opencode_verbose_models(&strip_ansi(&output.stdout)){if let Some(previous)=info.models.iter_mut().find(|m|m.id==model.id){*previous=model}else{info.models.push(model)}}}
   }
  }
 }
 if matches!(kind,AgentKind::Gemini|AgentKind::Cursor|AgentKind::Grok){
  info.defaults.capabilities_model=model.map(str::to_owned);info.defaults.capabilities_checked=true;
  match acp_model_metadata(kind,executable,project,model){
   Ok((models,defaults))=>{
    for model in models{if let Some(previous)=info.models.iter_mut().find(|m|m.id==model.id){*previous=model}else{info.models.push(model)}}
    if info.defaults.model.is_none(){info.defaults.model=defaults.model}
    info.defaults.efforts.extend(defaults.efforts);
   }
   Err(error)=>{info.note=Some(format!("Native model capabilities: {error}"));}
  }
 }
 if Some(home)==dirs::home_dir().as_deref() && matches!(kind,AgentKind::Claude|AgentKind::Gemini|AgentKind::OpenCode){if let Ok(output)=probe_with_limit(Path::new("/usr/bin/python3"),&["-c",include_str!("../native_helpers/provider_usage.py"),kind.command(),home.to_str().unwrap_or("")],home,Duration::from_secs(11),64*1024){if output.success{if let Ok(windows)=serde_json::from_slice::<Vec<LimitWindow>>(&output.stdout){info.limits=windows.into_iter().filter(LimitWindow::valid).take(32).collect();}}}}

 info
}
fn acp_model_metadata(kind:AgentKind,executable:&Path,project:&Path,selected:Option<&str>)->Result<(Vec<ModelChoice>,ModelDefaults)>{
 let mut command=Command::new(executable);crate::process_env::apply(&mut command);
 if kind==AgentKind::Cursor{command.arg("acp");}else if kind==AgentKind::Grok{command.env("GROK_NO_AUTO_UPDATE","1").args(["--permission-mode","plan","agent","--no-leader","stdio"]);}else{command.args(["--experimental-acp","--approval-mode","plan"]);}
 let mut child=ProbeChild(command.current_dir(project).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).process_group(0).spawn()?);
 let mut input=child.0.stdin.take().context("Native catalog input unavailable")?;let mut output=child.0.stdout.take().context("Native catalog output unavailable")?;
 let fd=output.as_raw_fd();let flags=unsafe{libc::fcntl(fd,libc::F_GETFL)};if flags<0 || unsafe{libc::fcntl(fd,libc::F_SETFL,flags|libc::O_NONBLOCK)}<0{bail!("Native catalog output unavailable")}
 let deadline=Instant::now()+Duration::from_secs(8);let mut pending=Vec::new();let mut total=0;let mut buffer=[0u8;8192];
 let mut rpc=|id:u64,method:&str,params:Value|->Result<Value>{
  writeln!(input,"{}",serde_json::json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;input.flush()?;
  while Instant::now()<deadline {
   while let Some(end)=pending.iter().position(|b|*b==b'\n'){
    let value:Value=serde_json::from_slice(&pending[..end])?;pending.drain(..=end);
    if value["id"]==id && value.get("method").is_none(){if let Some(error)=value.get("error"){bail!("{}",error["message"].as_str().unwrap_or("Native catalog rejected"))}return Ok(value["result"].clone())}
    if value.get("method").is_some() && !value["id"].is_null(){writeln!(input,"{}",serde_json::json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32601,"message":"Catalog inspection does not execute tools or approve permissions"}}))?;input.flush()?;}
   }
   match output.read(&mut buffer){Ok(0)=>bail!("Native catalog closed"),Ok(n)=>{total+=n;if total>METADATA_BYTES as usize{bail!("Native catalog exceeded bounds")}pending.extend_from_slice(&buffer[..n]);},Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>{let mut poll=libc::pollfd{fd,events:libc::POLLIN,revents:0};unsafe{libc::poll(&mut poll,1,20);}},Err(error)=>return Err(error.into())}
  }
  bail!("Native model catalog timed out")
 };
 let initialized=rpc(1,"initialize",serde_json::json!({"protocolVersion":1,"clientInfo":{"name":"code-model-controls","version":"1"},"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}))?;
 if initialized["protocolVersion"]!=1{bail!("Unsupported native catalog protocol")}
 let created=rpc(2,"session/new",serde_json::json!({"cwd":project,"mcpServers":[]}))?;
 let mut defaults=crate::provider_defaults::acp_selection(&created);let mut catalog=created.clone();
 if let Some(model)=selected {
  let available=crate::provider_defaults::acp_models(&created);let ids=available.iter().map(|m|m.id.clone()).collect();let resolved=crate::provider_defaults::acp_model_id(model,&ids).context("Selected model is absent from native catalog")?;let model=resolved.as_str();
  let config_id=created["configOptions"].as_array().into_iter().flatten().find(|option|option["category"]=="model" || option["id"]=="model").and_then(|option|option["id"].as_str());
  let updated=if let Some(config_id)=config_id{rpc(3,"session/set_config_option",serde_json::json!({"sessionId":created["sessionId"],"configId":config_id,"value":model}))?}else{rpc(3,"session/set_model",serde_json::json!({"sessionId":created["sessionId"],"modelId":model}))?};
  if updated["configOptions"].is_array(){catalog["configOptions"]=updated["configOptions"].clone();}else if defaults.model.as_deref()!=Some(model){catalog["configOptions"]=Value::Null;}
  catalog["models"]["currentModelId"]=Value::String(model.into());
 }
 let selection=crate::provider_defaults::acp_selection(&catalog);if let (Some(model),Some(effort))=(selection.model,selection.effort){defaults.efforts.insert(model,effort);}
 Ok((crate::provider_defaults::acp_models(&catalog),defaults))
}

#[cfg(test)]mod reasoning_catalog_tests{
 use super::*;use std::os::unix::fs::PermissionsExt;
 #[test]fn native_acp_catalog_uses_selected_model_without_a_prompt_or_global_write(){
  let root=std::env::temp_dir().join(format!("kai-model-catalog-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&root).unwrap();let path=root.join("agent");
  std::fs::write(&path,r#"#!/usr/bin/python3
import sys,json
model='model-a'
def config(): return [{'id':'native-model','category':'model','type':'select','currentValue':model,'options':[{'value':'model-a','name':'A'},{'value':'model-b','name':'B'}]},{'id':'native-thinking','category':'thought_level','type':'select','currentValue':'high' if model=='model-b' else 'medium','options':[{'value':'low'},{'value':'high'}] if model=='model-b' else [{'value':'medium'}]}]
for line in sys.stdin:
 v=json.loads(line);m=v['method']
 if m=='initialize':r={'protocolVersion':1}
 elif m=='session/new':r={'sessionId':'catalog-only','configOptions':config()}
 elif m=='session/set_config_option':
  assert v['params']=={'sessionId':'catalog-only','configId':'native-model','value':'model-b'}
  model='model-b';r={'configOptions':config()}
 else:raise AssertionError('Catalog must not send prompts, tools or policy changes: '+m)
 print(json.dumps({'jsonrpc':'2.0','id':v['id'],'result':r}),flush=True)
"#).unwrap();std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o700)).unwrap();
  let (models,defaults)=acp_model_metadata(AgentKind::Gemini,&path,&root,Some("model-b")).unwrap();assert_eq!(defaults.model.as_deref(),Some("model-a"));assert_eq!(defaults.efforts.get("model-b").map(String::as_str),Some("high"));assert_eq!(models.iter().find(|m|m.id=="model-b").unwrap().reasoning_efforts,vec!["low","high"]);assert!(models.iter().find(|m|m.id=="model-a").unwrap().reasoning_efforts.is_empty());std::fs::remove_dir_all(root).unwrap();
 }
}

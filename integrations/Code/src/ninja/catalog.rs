use super::*;
use serde_json::{Value, json};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Model {
    pub id: String,
    pub lab: String,
    pub label: String,
    pub installed: bool,
    pub size: u64,
    pub format: String,
    pub tools: bool,
    pub vision: bool,
    pub thinking: Vec<String>,
    pub thinking_default: Option<String>,
    pub context: usize,
}
pub fn lab(id: &str, family: &str) -> String {
    let s = format!("{} {}", id.to_lowercase(), family.to_lowercase());
    for (keys, name) in [
        (&["deepseek"][..], "DeepSeek"),
        (&["qwen", "qwq"][..], "Qwen"),
        (&["llama", "codellama"][..], "Meta"),
        (&["gemma"][..], "Google"),
        (&["gpt-oss"][..], "OpenAI"),
        (&["phi", "wizardlm"][..], "Microsoft"),
        (
            &[
                "mistral",
                "mixtral",
                "devstral",
                "codestral",
                "ministral",
                "magistral",
            ][..],
            "Mistral",
        ),
        (&["glm", "codegeex"][..], "Z.ai"),
        (&["kimi", "moonshot"][..], "Moonshot"),
        (&["minimax"][..], "MiniMax"),
        (&["granite"][..], "IBM"),
        (&["nemotron"][..], "NVIDIA"),
        (&["olmo"][..], "Ai2"),
        (&["smollm", "starcoder"][..], "Hugging Face"),
        (&["command-r", "command-a"][..], "Cohere"),
        (&["hermes"][..], "Nous Research"),
    ] {
        if keys.iter().any(|k| {
            if *k == "phi" {
                s.split(|c: char| !c.is_alphanumeric())
                    .any(|v| v.starts_with("phi"))
            } else {
                s.contains(k)
            }
        }) {
            return name.into();
        }
    }
    id.split_once('/')
        .map(|(org, _)| org)
        .filter(|v| *v != "library")
        .unwrap_or("Other")
        .into()
}
pub fn lab_glyph(id: &str) -> Option<&'static str> {
    match lab(id, "").as_str() {
        "DeepSeek" => Some("lab-deepseek"),
        "Qwen" => Some("lab-qwen"),
        "Meta" => Some("lab-meta"),
        "Google" => Some("lab-google"),
        "OpenAI" => Some("lab-openai"),
        "Microsoft" => Some("lab-microsoft"),
        "Mistral" => Some("lab-mistral"),
        "Z.ai" => Some("lab-zai"),
        "Moonshot" => Some("lab-moonshot"),
        "MiniMax" => Some("lab-minimax"),
        "IBM" => Some("lab-ibm"),
        "NVIDIA" => Some("lab-nvidia"),
        "Ai2" => Some("lab-ai2"),
        "Hugging Face" => Some("lab-huggingface"),
        "Cohere" => Some("lab-cohere"),
        "Nous Research" => Some("lab-nousresearch"),
        _ => None,
    }
}
pub fn installed(selection: &Selection) -> Result<Vec<Model>> {
    let url = if selection.engine == Engine::Ollama || selection.node {
        selection.api("/api/tags")
    } else {
        selection.api("/v1/models")
    };
    let value = http::json("GET", &url, None, Duration::from_secs(5))?;
    let mut models = Vec::new();
    for m in value["models"]
        .as_array()
        .or_else(|| value["data"].as_array())
        .into_iter()
        .flatten()
        .take(4096)
    {
        if let Some(id) = m["name"].as_str().or_else(|| m["id"].as_str()) {
            models.push(Model {
                id: id.into(),
                label: id.into(),
                lab: lab(id, m["details"]["family"].as_str().unwrap_or("")),
                installed: true,
                size: m["size"].as_u64().unwrap_or(0),
                format: m["details"]["format"].as_str().unwrap_or("gguf").into(),
                tools: false,
                vision: false,
                thinking: vec![],
                thinking_default: None,
                context: 0,
            });
        }
    }
    sort(&mut models);
    Ok(models)
}
pub fn inspect(selection: &Selection) -> Result<Model> {
    let mut model = Model {
        id: selection.model.clone(),
        label: selection.model.clone(),
        lab: lab(&selection.model, ""),
        installed: true,
        size: 0,
        format: "gguf".into(),
        tools: false,
        vision: false,
        thinking: vec![],
        thinking_default: None,
        context: 0,
    };
    let v = if selection.node {
        http::json(
            "POST",
            &selection.api("/model_info"),
            Some(
                &json!({"model":selection.model,"engine":selection.engine,"context":selection.context}),
            ),
            Duration::from_secs(if selection.engine == Engine::LlamaCpp {
                200
            } else {
                8
            }),
        )?
    } else if selection.engine == Engine::Ollama {
        http::json(
            "POST",
            &selection.api("/api/show"),
            Some(&json!({"model":selection.model})),
            Duration::from_secs(8),
        )?
    } else {
        http::json(
            "GET",
            &selection.api("/props"),
            None,
            Duration::from_secs(4),
        )?
    };
    anyhow::ensure!(
        v["remote_host"].as_str().is_none_or(str::is_empty)
            && v["remote_model"].as_str().is_none_or(str::is_empty),
        "Choose downloaded local weights; this Ollama model forwards inference to a cloud host"
    );
    let capabilities: Vec<_> = v["capabilities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    model.tools = capabilities.contains(&"tools")
        || (v["chat_template_caps"]["supports_tools"]
            .as_bool()
            .unwrap_or(false)
            && v["chat_template_caps"]["supports_tool_calls"]
                .as_bool()
                .unwrap_or(true));
    model.vision =
        capabilities.contains(&"vision") || v["modalities"]["vision"].as_bool().unwrap_or(false);
    model.lab = lab(
        &selection.model,
        v["details"]["family"].as_str().unwrap_or(""),
    );
    model.format = v["details"]["format"].as_str().unwrap_or("gguf").into();
    if let Some(levels) = v["thinking"]["values"]
        .as_array()
        .or_else(|| v["chat_template_caps"]["reasoning_effort_levels"].as_array())
    {
        model.thinking = levels
            .iter()
            .filter_map(|v| {
                v.as_str().map(str::to_owned).or_else(|| {
                    v.as_bool()
                        .map(|b| if b { "true".into() } else { "false".into() })
                })
            })
            .filter(|s| s.len() <= 32)
            .collect();
    } else if capabilities.contains(&"thinking") {
        model.thinking = if selection.model.starts_with("gpt-oss") {
            vec!["low".into(), "medium".into(), "high".into()]
        } else {
            vec!["false".into(), "true".into()]
        };
    }
    model.thinking_default = v["thinking"]["default"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| v["thinking"]["default"].as_bool().map(|v| v.to_string()))
        .or_else(|| {
            v["default_generation_settings"]["reasoning_effort"]
                .as_str()
                .map(str::to_owned)
        })
        .filter(|v| model.thinking.contains(v));
    if model.thinking == vec!["false".to_owned()] {
        model.thinking.clear();
        model.thinking_default = None;
    }
    model.context = v["default_generation_settings"]["n_ctx"]
        .as_u64()
        .or_else(|| {
            v["model_info"].as_object().and_then(|m| {
                m.iter()
                    .filter(|(k, _)| k.ends_with(".context_length"))
                    .filter_map(|(_, v)| v.as_u64())
                    .max()
            })
        })
        .unwrap_or(0) as usize;
    Ok(model)
}
pub fn sort(models: &mut [Model]) {
    models.sort_by(|a, b| {
        b.installed
            .cmp(&a.installed)
            .then(a.lab.to_lowercase().cmp(&b.lab.to_lowercase()))
            .then(a.id.cmp(&b.id))
    });
}
pub fn registry_page(page: usize, query: &str) -> Result<Vec<Model>> {
    anyhow::ensure!(
        (1..=1000).contains(&page) && query.len() <= 256,
        "Invalid registry query"
    );
    let q = query
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect::<String>();
    let html = String::from_utf8(http::public(&format!(
        "https://ollama.com/search?page={page}&q={q}"
    ))?)?;
    let re = regex::Regex::new(r#"(?s)<a\s[^>]*href="/library/([^"?#]+)"[^>]*>(.*?)</a>"#)?;
    let mut out = Vec::new();
    for c in re.captures_iter(&html) {
        let id = html_text(&c[1]);
        if id.contains(':') || out.iter().any(|m: &Model| m.id == id) {
            continue;
        }
        let text = html_text(&c[2]);
        out.push(Model {
            id: id.clone(),
            label: id.clone(),
            lab: lab(&id, ""),
            installed: false,
            size: 0,
            format: "gguf".into(),
            tools: text.split_whitespace().any(|s| s == "tools"),
            vision: text.split_whitespace().any(|s| s == "vision"),
            thinking: vec![],
            thinking_default: None,
            context: 0,
        });
    }
    Ok(out)
}
pub fn tags(family: &str) -> Result<Vec<String>> {
    anyhow::ensure!(
        family.len() <= 128
            && family
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-/ .".contains(c))
            && !family.contains([' ', '.']),
        "Invalid model family"
    );
    let html = String::from_utf8(http::public(&format!(
        "https://ollama.com/library/{family}/tags"
    ))?)?;
    let re = regex::Regex::new(r#"href="/library/([^"?#]+:[^"?#]+)""#)?;
    let mut tags = Vec::new();
    for c in re.captures_iter(&html) {
        let id = html_text(&c[1]);
        if !tags.contains(&id) {
            tags.push(id)
        }
    }
    Ok(tags)
}
fn html_text(s: &str) -> String {
    let re = regex::Regex::new("<[^>]*>").unwrap();
    re.replace_all(s, " ")
        .replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
}
pub fn bundled_registry() -> Vec<Model> {
    let mut models: Vec<Model> =
        serde_json::from_str(include_str!("../../assets/ninja-registry.json")).unwrap_or_default();
    for m in &mut models {
        m.lab = lab(&m.id, "");
    }
    sort(&mut models);
    models
}
/// Complete family catalog, cached off the render thread. Tags stay demand-driven.
pub fn registry() -> Result<Vec<Model>> {
    let cache = data_dir().join("registry.json");
    if let Ok(meta) = std::fs::metadata(&cache) {
        if meta.len() < 8 * 1024 * 1024
            && meta
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < Duration::from_secs(86400))
        {
            if let Ok(v) = serde_json::from_slice::<Vec<Model>>(&std::fs::read(&cache)?) {
                return Ok(v);
            }
        }
    }
    let mut models = Vec::new();
    for page in 1..=1000 {
        let rows = registry_page(page, "")?;
        let count = models.len();
        for row in rows {
            if !models.iter().any(|m: &Model| m.id == row.id) {
                models.push(row)
            }
        }
        if models.len() == count {
            break;
        }
        if models.len() > 10000 {
            anyhow::bail!("Model registry exceeds the catalog budget");
        }
    }
    anyhow::ensure!(
        !models.is_empty(),
        "The Ollama registry returned no model families"
    );
    sort(&mut models);
    std::fs::create_dir_all(data_dir())?;
    let temp = cache.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temp, serde_json::to_vec(&models)?)?;
    std::fs::rename(temp, cache)?;
    Ok(models)
}
pub fn pull(
    selection: &Selection,
    model: &str,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mut progress: impl FnMut(String),
) -> Result<()> {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        process::{Command, Stdio},
        sync::atomic::Ordering,
    };
    anyhow::ensure!(
        selection.engine == Engine::Ollama || selection.node,
        "Downloads for a direct llama.cpp server are managed on its host; connect the Ninja host helper to manage Ollama GGUF downloads"
    );
    anyhow::ensure!(
        !model.is_empty() && model.len() <= 256 && !model.chars().any(char::is_control),
        "Invalid model name"
    );
    let url = selection.api("/api/pull");
    http::validate_endpoint(&url)?;
    let mut child = Command::new("/usr/bin/curl")
        .args([
            "--noproxy",
            "*",
            "--silent",
            "--fail",
            "--no-buffer",
            "--connect-timeout",
            "3",
            "--max-time",
            "3600",
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
            &url,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&json!({"model":model,"stream":true}))?)?;
        let output = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(2);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(output);
            loop {
                let mut line = Vec::new();
                match reader.by_ref().take(65537).read_until(b'\n', &mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if tx.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        });
        let mut last = std::time::Instant::now() - Duration::from_secs(1);
        let until = std::time::Instant::now() + Duration::from_secs(3600);
        let mut success = false;
        loop {
            anyhow::ensure!(!cancel.load(Ordering::Acquire), "Model download stopped");
            anyhow::ensure!(
                std::time::Instant::now() < until,
                "Model download timed out"
            );
            let line = match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(v) => v?,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            anyhow::ensure!(
                line.len() <= 65536,
                "Download event exceeds metadata budget"
            );
            let v: Value = serde_json::from_slice(&line)?;
            if let Some(error) = v["error"].as_str() {
                anyhow::bail!("{error}")
            }
            let status = v["status"].as_str().unwrap_or("Downloading");
            success = status == "success";
            if last.elapsed() >= Duration::from_millis(150) || success {
                last = std::time::Instant::now();
                progress(
                    if let (Some(done), Some(total)) =
                        (v["completed"].as_u64(), v["total"].as_u64())
                    {
                        format!(
                            "{model} · {status} · {:.0}%",
                            100. * done as f64 / total.max(1) as f64
                        )
                    } else {
                        format!("{model} · {status}")
                    },
                );
            }
        }
        anyhow::ensure!(success, "Download ended without confirmation");
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    result?;
    anyhow::ensure!(status.success(), "Model download failed");
    Ok(())
}

/// Naming a Ninja chat stays on its selected model host, never a cloud title model.
pub fn generate_title(
    selection: &Selection,
    prompt: &str,
    cancel: &std::sync::atomic::AtomicBool,
) -> Option<String> {
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    selection.validate().ok()?;
    let message = crate::chat_titles::prompt(prompt);
    let metadata = inspect(selection).ok()?;
    let thinking = metadata.thinking.first().map(|v| match v.as_str() {
        "true" => json!(true),
        "false" => json!(false),
        _ => json!(v),
    });
    let (path, mut body) = if selection.engine == Engine::Ollama {
        (
            if selection.node {
                "/infer/ollama/api/chat"
            } else {
                "/api/chat"
            },
            json!({"model":selection.model,"messages":[{"role":"user","content":message}],"stream":false,"format":"json","options":{"num_ctx":8192,"num_predict":128}}),
        )
    } else {
        (
            if selection.node {
                "/infer/llama_cpp/v1/chat/completions"
            } else {
                "/v1/chat/completions"
            },
            json!({"model":selection.model,"messages":[{"role":"user","content":message}],"stream":false,"max_tokens":128,"response_format":{"type":"json_object"},"kai_context":selection.context}),
        )
    };
    if let Some(thinking) = thinking {
        body[if selection.engine == Engine::Ollama {
            "think"
        } else {
            "reasoning_effort"
        }] = thinking;
    }
    let value = http::json(
        "POST",
        &selection.api(path),
        Some(&body),
        Duration::from_secs(30),
    )
    .ok()?;
    if cancel.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    crate::chat_titles::parse(
        value["message"]["content"]
            .as_str()
            .or_else(|| value["choices"][0]["message"]["content"].as_str())?,
    )
}

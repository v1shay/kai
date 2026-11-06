//! Provider-reported usage only. No tokenizer, inferred quotas, or polling.
use crate::discovery::AgentKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A window explicitly supplied by the provider. Missing duration/reset stays
/// absent; it is never synthesized from a plan name or inferred token usage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitWindow {
    pub label: String,
    pub used_percent: f64,
    pub duration_minutes: Option<u64>,
    pub resets_at: Option<i64>,
    #[serde(default)]
    pub reported_at_ms: Option<i64>,
}
impl LimitWindow {
    /// Compact surfaces identify the provider with artwork and show one reset
    /// countdown. Internal bucket names stay in the detailed quota panel.
    pub fn compact_used(&self, now:i64)->String {
        let percent=if self.used_percent>0. && self.used_percent<0.1 {"<0.1".into()}else{format!("{:.0}",self.used_percent)};
        let reset=self.resets_at.map(|at|{
            let secs=(at-now).max(0);
            if secs>=86400{format!("{}d {}h",secs/86400,secs%86400/3600)}else if secs>=3600{format!("{}h {}m",secs/3600,secs%3600/60)}else{format!("{}m",secs/60)}
        });
        reset.map(|reset|format!("{percent}% {reset}")).unwrap_or_else(||format!("{percent}%"))
    }
    pub fn remaining_text(&self) -> String {
        let remaining = (100. - self.used_percent).clamp(0., 100.);
        let rounded = (remaining * 10.).round() / 10.;
        if remaining > 0. && rounded == 0. {
            "<0.1% left".into()
        } else if remaining < 100. && rounded == 100. {
            ">99.9% left".into()
        } else if rounded.fract() == 0. {
            format!("{rounded:.0}% left")
        } else {
            format!("{rounded:.1}% left")
        }
    }

    pub fn provider_title(&self, agent: AgentKind) -> String {
        let bucket = self
            .label
            .trim()
            .strip_suffix(" primary")
            .or_else(|| self.label.trim().strip_suffix(" secondary"))
            .unwrap_or(self.label.trim());
        let generic = bucket.eq_ignore_ascii_case(agent.command())
            || bucket.eq_ignore_ascii_case(agent.name())
            || ["default", "primary", "secondary"]
                .iter()
                .any(|name| bucket.eq_ignore_ascii_case(name))
            || agent == AgentKind::Codex
                && ["cod", "codex-cli"]
                    .iter()
                    .any(|name| bucket.eq_ignore_ascii_case(name));
        if generic {
            agent.name().into()
        } else {
            let bucket = bucket.replace(['_', '-'], " ");
            let mut chars = bucket.chars();
            let title = chars
                .next()
                .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default();
            format!("{} · {title}", agent.name())
        }
    }

    pub fn window_title(&self) -> Option<String> {
        match self.duration_minutes.filter(|minutes| *minutes > 0) {
            Some(minutes) if minutes % 1440 == 0 => Some(format!("{}-day limit", minutes / 1440)),
            Some(minutes) if minutes % 60 == 0 => Some(format!("{}-hour limit", minutes / 60)),
            Some(minutes) => Some(format!("{minutes}-minute limit")),
            None if self.label.ends_with(" primary") => Some("Primary limit".into()),
            None if self.label.ends_with(" secondary") => Some("Secondary limit".into()),
            None => None,
        }
    }

    pub fn valid(&self) -> bool {
        self.used_percent.is_finite()
            && (0. ..=100.).contains(&self.used_percent)
            && !self.label.is_empty()
            && self.label.len() <= 160
            && !self.label.chars().any(char::is_control)
    }
}

/// The current App Server schema has named buckets; legacy transcripts use the
/// same primary/secondary windows with snake_case fields. Bound both views.
pub fn codex_limit_windows(value: &Value, snake_case: bool) -> Vec<LimitWindow> {
    let (map_key, single_key, used_key, duration_key, reset_key) = if snake_case {
        (
            "rate_limits_by_limit_id",
            "rate_limits",
            "used_percent",
            "window_minutes",
            "resets_at",
        )
    } else {
        (
            "rateLimitsByLimitId",
            "rateLimits",
            "usedPercent",
            "windowDurationMins",
            "resetsAt",
        )
    };
    let mut out = Vec::new();
    let parse = |name: &str, bucket: &Value, out: &mut Vec<LimitWindow>| {
        let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
        let name = if name.trim().is_empty() {
            "codex"
        } else {
            name.trim()
        };
        for key in ["primary", "secondary"] {
            let window = &bucket[key];
            if let Some(used_percent) = window[used_key]
                .as_f64()
                .filter(|n| n.is_finite() && (0. ..=100.).contains(n))
            {
                out.push(LimitWindow {
                    label: format!("{name} {key}"),
                    used_percent,
                    duration_minutes: window[duration_key].as_u64().filter(|n| *n > 0),
                    resets_at: window[reset_key]
                        .as_i64()
                        .filter(|t| *t > 0 && chrono::DateTime::from_timestamp(*t, 0).is_some()),
                    reported_at_ms: Some(chrono::Utc::now().timestamp_millis()),
                });
            }
        }
    };
    if let Some(buckets) = value[map_key].as_object() {
        for (name, bucket) in buckets.iter().take(16) {
            parse(name, bucket, &mut out);
        }
    }
    if out.is_empty() {
        let bucket = &value[single_key];
        let name = bucket
            .get(if snake_case { "limit_id" } else { "limitId" })
            .and_then(Value::as_str)
            .unwrap_or("codex");
        parse(name, bucket, &mut out);
    }
    out
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cached: Option<u64>,
    pub context_used: Option<u64>,
    pub context_limit: Option<u64>,
    pub cost_usd: Option<f64>,
    pub scope: String,
    pub model: Option<String>,
    pub reasoning_effort:Option<String>,
    pub limit_used_percent: Option<f64>,
    pub limit_resets_at: Option<i64>,
    pub limit_windows: Vec<LimitWindow>,
}
impl Usage {
    pub fn merge(&mut self, mut next: Self) {
        if next.model.is_some() && next.model!=self.model && next.reasoning_effort.is_none(){self.reasoning_effort=None;}
        let stale_primary = next
            .limit_windows
            .iter()
            .find(|window| window.label.ends_with(" primary"))
            .and_then(|incoming| {
                self.limit_windows
                    .iter()
                    .find(|previous| previous.label == incoming.label)
                    .and_then(|previous| incoming.reported_at_ms.zip(previous.reported_at_ms))
            })
            .is_some_and(|(incoming, previous)| incoming < previous);
        if stale_primary {
            next.limit_used_percent = None;
            next.limit_resets_at = None;
        }
        macro_rules! take { ($($field:ident),*) => { $(if next.$field.is_some() { self.$field = next.$field; })* }; }
        take!(
            input,
            output,
            cached,
            context_used,
            context_limit,
            cost_usd,
            model,
            reasoning_effort,
            limit_used_percent,
            limit_resets_at
        );
        for mut window in next
            .limit_windows
            .into_iter()
            .filter(LimitWindow::valid)
            .take(32)
        {
            if let Some(existing) = self
                .limit_windows
                .iter_mut()
                .find(|previous| previous.label == window.label)
            {
                if window
                    .reported_at_ms
                    .zip(existing.reported_at_ms)
                    .is_some_and(|(next, previous)| next < previous)
                {
                    continue;
                }
                window.duration_minutes = window.duration_minutes.or(existing.duration_minutes);
                window.resets_at = window.resets_at.or(existing.resets_at);
                *existing = window;
            } else if self.limit_windows.len() < 32 {
                self.limit_windows.push(window);
            }
        }
        if !next.scope.is_empty() {
            self.scope = next.scope;
        }
    }
    pub fn codex_limits(value: &Value, snake_case: bool) -> Option<Self> {
        let windows = codex_limit_windows(value, snake_case);
        if windows.is_empty() {
            return None;
        }
        let primary = windows
            .iter()
            .find(|window| window.label.ends_with(" primary"));
        Some(Self {
            limit_used_percent: primary.map(|w| w.used_percent),
            limit_resets_at: primary.and_then(|w| w.resets_at),
            limit_windows: windows,
            ..Default::default()
        })
    }
    pub fn observed(agent: AgentKind, value: &Value) -> Option<Self> {
        if !value["parent_tool_use_id"].is_null() {
            return None;
        }
        let kind = value["type"]
            .as_str()
            .or_else(|| value["event"].as_str())
            .unwrap_or("");
        let mut out = Self::default();
        let count = |value: &Value| value.as_u64().filter(|n| *n <= 1_000_000_000_000);
        let amount = |value: &Value| {
            value
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0. && *n < 1_000_000_000.)
        };
        let model = |value: &Value| {
            value
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control))
                .map(str::to_owned)
        };
        match agent {
            AgentKind::Ninja => return Self::observed(if matches!(kind,"stream_event"|"result"|"assistant"|"system"){AgentKind::Claude}else{AgentKind::Codex},value),
            AgentKind::Codex => {
                let usage = if kind == "turn_context" {
                    out.model=model(&value["payload"]["model"]);
                    out.reasoning_effort=model(&value["payload"]["effort"]).or_else(||model(&value["payload"]["reasoning_effort"]));
                    out.scope="Native chat settings".into();&Value::Null
                } else if kind == "turn.completed" {
                    out.scope = "Last turn".into();
                    &value["usage"]
                } else if kind == "event_msg" && value["payload"]["type"] == "token_count" {
                    let info = &value["payload"]["info"];
                    out.context_used = count(&info["last_token_usage"]["total_tokens"]);
                    out.context_limit = count(&info["model_context_window"]);
                    if let Some(limits) = Self::codex_limits(&value["payload"], true) {
                        out.merge(limits);
                    }
                    out.scope = "Last model request".into();
                    &info["last_token_usage"]
                } else {
                    return None;
                };
                out.input = count(&usage["input_tokens"]);
                out.output = count(&usage["output_tokens"]);
                out.cached = count(&usage["cached_input_tokens"]);
            }
            AgentKind::Claude | AgentKind::Grok | AgentKind::Cursor => {
                let usage = if kind == "assistant" {
                    out.scope = "Last model request".into();
                    out.model = model(&value["message"]["model"]);
                    &value["message"]["usage"]
                } else if kind == "result" {
                    out.scope = "CLI report".into();
                    out.cost_usd =
                        amount(&value["total_cost_usd"]).or_else(|| amount(&value["cost_usd"]));
                    if let Some(models) = value["modelUsage"].as_object().filter(|m| m.len() == 1) {
                        let (name, report) = models.iter().next().unwrap();
                        out.model = model(&Value::String(name.clone()));
                        out.context_limit = count(&report["contextWindow"]);
                    }
                    &value["usage"]
                } else if kind == "system" && value["subtype"] == "init" {
                    out.model = model(&value["model"]);
                    out.reasoning_effort=model(&value["effortLevel"]).or_else(||model(&value["effort"]));
                    &Value::Null
                } else {
                    return None;
                };
                out.input = count(&usage["input_tokens"]);
                out.output = count(&usage["output_tokens"]);
                out.cached = count(&usage["cache_read_input_tokens"]);
                if kind == "assistant" {
                    out.context_used = out.input.map(|n| {
                        n.saturating_add(out.cached.unwrap_or(0)).saturating_add(
                            count(&usage["cache_creation_input_tokens"]).unwrap_or(0),
                        )
                    });
                }
            }
            AgentKind::Gemini => {
                if kind == "init" {
                    out.model = model(&value["model"]);
                } else if kind == "result" {
                    out.scope = "CLI report".into();
                    out.input = count(&value["stats"]["input_tokens"]);
                    out.output = count(&value["stats"]["output_tokens"]);
                    out.cached = count(&value["stats"]["cached"]);
                } else {
                    return None;
                }
            }
            AgentKind::OpenCode => {
                if kind != "step_finish" {
                    return None;
                }
                let part = &value["part"];
                out.scope = "Last model step".into();
                out.input = count(&part["tokens"]["input"]);
                out.output = count(&part["tokens"]["output"]);
                out.cached = count(&part["tokens"]["cache"]["read"]);
                out.context_used = out.input.map(|n| {
                    n.saturating_add(out.cached.unwrap_or(0))
                        .saturating_add(count(&part["tokens"]["cache"]["write"]).unwrap_or(0))
                });
                out.cost_usd = amount(&part["cost"]);
            }
            // This installed protocol does not document token/context counters.
            AgentKind::Antigravity => {out.model=model(&value["model"]).or_else(||model(&value["message"]["model"]));},
        }
        let has_data = out.input.is_some()
            || out.output.is_some()
            || out.cached.is_some()
            || out.context_used.is_some()
            || out.context_limit.is_some()
            || out.cost_usd.is_some()
            || out.model.is_some()
            || out.limit_used_percent.is_some()
            || !out.limit_windows.is_empty();
        has_data.then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quota_cards_use_remaining_values_and_readable_provider_names_for_every_adapter() {
        for agent in [
            AgentKind::Codex,
            AgentKind::Claude,
            AgentKind::Gemini,
            AgentKind::OpenCode,
            AgentKind::Antigravity,
            AgentKind::Cursor,
            AgentKind::Grok,
        ] {
            let mut window = LimitWindow {
                label: format!("{} secondary", agent.command()),
                used_percent: 26.,
                duration_minutes: Some(10080),
                resets_at: Some(1800000000),
                reported_at_ms: None,
            };
            assert_eq!(window.remaining_text(), "74% left");
            assert_eq!(window.provider_title(agent), agent.name());
            assert_eq!(window.window_title().as_deref(), Some("7-day limit"));
            for (used, expected) in [
                (0., "100% left"),
                (100., "0% left"),
                (12.5, "87.5% left"),
                (99.99, "<0.1% left"),
                (0.01, ">99.9% left"),
            ] {
                window.used_percent = used;
                assert_eq!(window.remaining_text(), expected);
            }
        }
        let mut window = LimitWindow {
            label: "cod primary".into(),
            used_percent: 26.,
            duration_minutes: Some(300),
            resets_at: None,
            reported_at_ms: None,
        };
        assert_eq!(window.provider_title(AgentKind::Codex), "Codex");
        assert_eq!(window.window_title().as_deref(), Some("5-hour limit"));
        window.label = "review secondary".into();
        assert_eq!(window.provider_title(AgentKind::Codex), "Codex · Review");
        window.duration_minutes = None;
        assert_eq!(window.window_title().as_deref(), Some("Secondary limit"));
        window.used_percent = f64::NAN;
        assert!(!window.valid());
    }

    use serde_json::json;
    #[test]
    fn usage_uses_real_fields_and_never_invents_context_or_quotas() {
        let u = Usage::observed(AgentKind::Codex, &json!({"type":"turn.completed","usage":{"input_tokens":800,"output_tokens":32,"cached_input_tokens":300}})).unwrap();
        assert_eq!(u.input, Some(800));
        assert_eq!(u.cached, Some(300));
        assert!(
            u.context_used.is_none() && u.context_limit.is_none() && u.limit_used_percent.is_none()
        );
        let c = Usage::observed(AgentKind::Claude, &json!({"type":"assistant","message":{"model":"real-model","usage":{"input_tokens":8,"cache_read_input_tokens":20,"cache_creation_input_tokens":30,"output_tokens":2}}})).unwrap();
        assert_eq!(c.context_used, Some(58));
        assert!(
            Usage::observed(
                AgentKind::Cursor,
                &json!({"type":"result","result":"hello"})
            )
            .is_none()
        );
        assert!(
            Usage::observed(
                AgentKind::Antigravity,
                &json!({"event":"result","tokens":100})
            )
            .is_none()
        );
    }
    #[test]
    fn all_reported_adapters_reject_subagents_bad_counts_and_keep_zero() {
        assert!(Usage::observed(AgentKind::Claude, &json!({"type":"assistant","parent_tool_use_id":"tool","message":{"usage":{"input_tokens":999}}})).is_none());
        let g = Usage::observed(
            AgentKind::Gemini,
            &json!({"type":"result","stats":{"input_tokens":0,"output_tokens":7,"cached":4}}),
        )
        .unwrap();
        assert_eq!(g.input, Some(0));
        let o = Usage::observed(AgentKind::OpenCode, &json!({"type":"step_finish","part":{"cost":0.03,"tokens":{"input":9,"output":2,"cache":{"read":4,"write":5}}}})).unwrap();
        assert_eq!(o.context_used, Some(18));
        assert!(
            Usage::observed(
                AgentKind::Grok,
                &json!({"type":"result","usage":{"input_tokens":-1,"output_tokens":"10"}})
            )
            .is_none()
        );
    }
    #[test]
    fn cached_old_quota_cannot_replace_a_newer_reported_window() {
        let mut current = Usage::codex_limits(
            &json!({"rateLimits":{"primary":{"usedPercent":35,"resetsAt":1800000000}}}),
            false,
        )
        .unwrap();
        current.limit_windows[0].reported_at_ms = Some(200);
        let mut old = Usage::codex_limits(
            &json!({"rateLimits":{"primary":{"usedPercent":5,"resetsAt":1700000000}}}),
            false,
        )
        .unwrap();
        old.limit_windows[0].reported_at_ms = Some(100);
        current.merge(old);
        assert_eq!(current.limit_used_percent, Some(35.));
        assert_eq!(current.limit_windows[0].used_percent, 35.);
        assert_eq!(current.limit_windows[0].resets_at, Some(1800000000));
    }

    #[test]
    fn merge_retains_explicit_context_when_terminal_report_has_only_usage() {
        let mut u = Usage::observed(AgentKind::Codex, &json!({"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"total_tokens":90},"model_context_window":1000},"rate_limits":{"primary":{"used_percent":25,"resets_at":1800000000}}}})).unwrap();
        u.merge(
            Usage::observed(
                AgentKind::Codex,
                &json!({"type":"turn.completed","usage":{"input_tokens":80,"output_tokens":10}}),
            )
            .unwrap(),
        );
        assert_eq!(u.context_used, Some(90));
        assert_eq!(u.context_limit, Some(1000));
        assert_eq!(u.limit_used_percent, Some(25.));
    }
    #[test]
    fn native_quota_maps_preserve_all_reported_windows_and_sparse_resets() {
        let value = json!({"rateLimits":{"primary":{"usedPercent":99}}, "rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":1800000000},"secondary":{"usedPercent":42,"windowDurationMins":10080,"resetsAt":1800600000}},"review":{"primary":{"usedPercent":7}}}});
        let mut usage = Usage::codex_limits(&value, false).unwrap();
        assert_eq!(usage.limit_windows.len(), 3);
        assert_eq!(usage.limit_windows[0].used_percent, 0.);
        usage.merge(Usage::codex_limits(&json!({"rateLimits":{"limitId":"codex","primary":{"usedPercent":10,"resetsAt":null},"secondary":null}}), false).unwrap());
        assert_eq!(usage.limit_windows.len(), 3);
        assert_eq!(usage.limit_windows[0].used_percent, 10.);
        assert_eq!(usage.limit_windows[0].resets_at, Some(1800000000));
        assert_eq!(usage.limit_windows[1].resets_at, Some(1800600000));
        let snake = Usage::observed(AgentKind::Codex, &json!({"type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":5,"window_minutes":300,"resets_at":1800000000},"secondary":{"used_percent":60,"window_minutes":10080,"resets_at":1800600000}}}})).unwrap();
        assert_eq!(snake.limit_windows.len(), 2);
        assert_eq!(snake.limit_windows[1].duration_minutes, Some(10080));
        assert!(Usage::codex_limits(&json!({"rateLimits":{"primary":{"usedPercent":null,"resetsAt":1800000000},"secondary":{"usedPercent":101}}}),false).is_none());
        let no_reset = codex_limit_windows(
            &json!({"rateLimits":{"primary":{"usedPercent":0,"resetsAt":-1,"windowDurationMins":0}}}),
            false,
        );
        assert_eq!(no_reset[0].resets_at, None);
        assert_eq!(no_reset[0].duration_minutes, None);
    }
}

/// Read the newest complete native turn-context record, without loading a large
/// transcript or mistaking the user's prose for model metadata.
pub fn native_chat_settings(path:&std::path::Path,agent:AgentKind)->Option<Usage>{
    use std::io::{Read,Seek,SeekFrom};
    if agent!=AgentKind::Codex{return None}
    let mut file=std::fs::File::open(path).ok()?;let len=file.metadata().ok()?.len();let start=len.saturating_sub(16*1024*1024);
    file.seek(SeekFrom::Start(start)).ok()?;let mut bytes=Vec::new();file.take(16*1024*1024).read_to_end(&mut bytes).ok()?;
    let begin=if start==0{0}else{bytes.iter().position(|b|*b==b'\n')?+1};
    for line in bytes[begin..].split(|b|*b==b'\n').rev(){
        if line.len()>64*1024 || !line.windows(14).any(|w|w==b"\"turn_context\""){continue}
        let Ok(value)=serde_json::from_slice::<Value>(line)else{continue};
        if value["type"]=="turn_context"{if let Some(usage)=Usage::observed(agent,&value){return Some(usage)}}
    }None
}

#[cfg(test)]mod native_model_tests{
 use super::*;
 #[test]fn reads_only_actual_native_turn_context_and_keeps_its_effort(){
  let path=std::env::temp_dir().join(format!("kai-model-history-{}.jsonl",uuid::Uuid::new_v4()));
  std::fs::write(&path,concat!("{\"type\":\"turn_context\",\"payload\":{\"model\":\"first-chat-model\",\"effort\":\"low\"}}\n","{\"type\":\"response_item\",\"payload\":{\"type\":\"turn_context\",\"model\":\"user-mentioned-model\"}}\n","{\"type\":\"turn_context\",\"payload\":{\"model\":\"actual-last-model\",\"effort\":\"high\"}}\n")).unwrap();
  let actual=native_chat_settings(&path,AgentKind::Codex).unwrap();assert_eq!(actual.model.as_deref(),Some("actual-last-model"));assert_eq!(actual.reasoning_effort.as_deref(),Some("high"));assert!(native_chat_settings(&path,AgentKind::Claude).is_none());std::fs::remove_file(path).unwrap();
 }
}

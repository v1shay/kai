//! Read only model-selection fields. Native protocol/config data stays separate
//! from explicit per-chat overrides and never chooses a replacement provider.
use crate::{agent_controls::{ControlInfo,ModelDefaults},discovery::AgentKind,runtime::valid_model_id};
use serde_json::Value;
use std::{path::{Path,PathBuf},fs,io::Read};
fn text(v:&Value)->Option<String>{v.as_str().filter(|s|valid_model_id(s)).map(str::to_owned)}
pub fn acp_selection(value:&Value)->ModelDefaults{
 let mut out=ModelDefaults::default();
 out.model=text(&value["models"]["currentModelId"]).or_else(||text(&value["currentModelId"]));
 for option in value["configOptions"].as_array().into_iter().flatten(){
  let id=option["category"].as_str().or_else(||option["id"].as_str()).unwrap_or("");
  match id{"model"=>{if let Some(model)=text(&option["currentValue"]){out.model=Some(model)}},"thought_level"|"reasoning_effort"|"reasoning"|"thinking"=>out.effort=text(&option["currentValue"]),_=>{}}
 }
 out
}
fn json(path:&Path)->Option<Value>{let f=fs::File::open(path).ok()?;if f.metadata().ok()?.len()>2*1024*1024{return None};let mut b=Vec::new();f.take(2*1024*1024).read_to_end(&mut b).ok()?;serde_json::from_slice(&b).ok()}
pub fn resolve_configured_defaults(kind:AgentKind,home:&Path,project:&Path,info:&mut ControlInfo){
 let files:Vec<(PathBuf,&str,&str)>=match kind{
  AgentKind::Claude=>vec![(home.join(".claude/settings.json"),"/model","/effortLevel"),(project.join(".claude/settings.json"),"/model","/effortLevel"),(project.join(".claude/settings.local.json"),"/model","/effortLevel")],
  AgentKind::Gemini=>vec![(home.join(".gemini/settings.json"),"/model/name","/model/thinkingLevel"),(project.join(".gemini/settings.json"),"/model/name","/model/thinkingLevel")],
  AgentKind::OpenCode=>vec![(home.join(".config/opencode/opencode.json"),"/model","/variant"),(project.join("opencode.json"),"/model","/variant")],
  AgentKind::Antigravity=>vec![(home.join(".gemini/antigravity-cli/settings.json"),"/model","/thinkingLevel")],
  AgentKind::Cursor=>vec![(home.join(".cursor/cli-config.json"),"/model","/reasoningEffort"),(project.join(".cursor/cli.json"),"/model","/reasoningEffort")],
  AgentKind::Grok=>vec![(home.join(".grok/settings.json"),"/model","/reasoning_effort"),(home.join(".grok/config.json"),"/model","/reasoning_effort")],
  _=>vec![]};
 for (path,model,effort) in files{if let Some(v)=json(&path){if let Some(model)=v.pointer(model).and_then(text){info.defaults.model=Some(model)};if let Some(effort)=v.pointer(effort).and_then(text){info.defaults.effort=Some(effort)}}}
 let env=match kind{AgentKind::Gemini=>Some("GEMINI_MODEL"),AgentKind::Claude=>Some("ANTHROPIC_MODEL"),_=>None};
 if let Some(model)=env.and_then(|k|std::env::var(k).ok()).filter(|id|valid_model_id(id)){info.defaults.model=Some(model)}
 if kind==AgentKind::Claude{if let Ok(effort)=std::env::var("CLAUDE_CODE_EFFORT_LEVEL"){if valid_model_id(&effort){info.defaults.effort=Some(effort)}}}
}
/// Gemini's native Auto selection is routing, not a guessed Flash/Pro model.
/// Read the installed build's own default symbol through its direct imports.
/// Only source files inside that installation are read; no source is executed.
pub fn gemini_build_default(executable:&Path)->Option<String>{
 let entry=fs::canonicalize(executable).ok()?;let root=entry.parent()?;
 fn source(path:&Path)->Option<String>{let file=fs::File::open(path).ok()?;if file.metadata().ok()?.len()>16*1024*1024{return None}let mut s=String::new();file.take(16*1024*1024).read_to_string(&mut s).ok()?;Some(s)}
 fn local(root:&Path,name:&str)->Option<PathBuf>{if !name.starts_with("./") || name.contains("..") || !name.ends_with(".js"){return None}let path=fs::canonicalize(root.join(name.strip_prefix("./")?)).ok()?;path.starts_with(root).then_some(path)}
 let entry_text=source(&entry)?;
 let import=regex::Regex::new(r#"import\(\"(\./gemini-[^\"]+\.js)\"\)"#).ok()?;
 let main=import.captures(&entry_text).and_then(|c|local(root,&c[1])).and_then(|p|source(&p)).unwrap_or(entry_text);
 // The selected default is declared by loadCliConfig in this installed build.
 let symbol=regex::Regex::new(r"const defaultModel = ([A-Z_]+);").ok()?.captures(&main)?.get(1)?.as_str();
 let imports=regex::Regex::new(r#"(?s)import\s*\{([^}]+)\}\s*from\s*\"([^\"]+)\""#).ok()?;
 for cap in imports.captures_iter(&main){if cap[1].split(',').any(|s|s.trim()==symbol){
  let content=source(&local(root,&cap[2])?)?;
  let pattern=format!(r#"(?:var|const|let)\s+{}\s*=\s*\"([^\"]+)\""#,regex::escape(symbol));
  let id=regex::Regex::new(&pattern).ok()?.captures(&content)?.get(1)?.as_str();return valid_model_id(id).then(||id.into())
 }}None
}

#[cfg(test)]mod tests{
 use super::*;
 #[test]fn acp_uses_the_current_selection_instead_of_the_first_catalog_item(){
  let v=serde_json::json!({"models":{"currentModelId":"gemini-3.5-flash","availableModels":[{"modelId":"some-other-model"}]},"configOptions":[{"category":"thought_level","currentValue":"high"}]});
  let d=acp_selection(&v);assert_eq!(d.model.as_deref(),Some("gemini-3.5-flash"));assert_eq!(d.effort.as_deref(),Some("high"));
  let update=acp_selection(&serde_json::json!({"configOptions":[{"category":"model","currentValue":"gemini-3.5-pro"}]}));assert_eq!(update.model.as_deref(),Some("gemini-3.5-pro"));
 }
 #[test]fn gemini_default_comes_from_the_installed_entry_imports_and_never_guesses_flash(){
  let root=std::env::temp_dir().join(format!("kai-gemini-default-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&root).unwrap();
  let entry=root.join("gemini.js");fs::write(&entry,"const { main } = await import(\"./gemini-version.js\");").unwrap();
  fs::write(root.join("gemini-version.js"),"import { PREVIEW_GEMINI_MODEL_AUTO } from \"./constants.js\"; const defaultModel = PREVIEW_GEMINI_MODEL_AUTO;").unwrap();
  fs::write(root.join("constants.js"),"var PREVIEW_GEMINI_MODEL_AUTO = \"auto-gemini-3\";").unwrap();assert_eq!(gemini_build_default(&entry).as_deref(),Some("auto-gemini-3"));
  fs::write(root.join("gemini-version.js"),"unsupported install format").unwrap();assert_eq!(gemini_build_default(&entry),None);let _=fs::remove_dir_all(root);
 }
 #[test]fn model_reroutes_never_keep_an_effort_from_the_previous_model(){
  let mut u=crate::usage::Usage{model:Some("old".into()),reasoning_effort:Some("high".into()),..Default::default()};u.merge(crate::usage::Usage{model:Some("new".into()),..Default::default()});assert_eq!(u.reasoning_effort,None);
 }
 #[test]#[ignore="Read-only installed provider metadata; no inference"]fn installed_defaults(){
  let home=dirs::home_dir().unwrap();
  for (kind,path) in [(AgentKind::Codex,"/opt/homebrew/bin/codex"),(AgentKind::Claude,"/opt/homebrew/bin/claude"),(AgentKind::Gemini,"/opt/homebrew/bin/gemini"),(AgentKind::Cursor,"/Users/vishayagarwal/.local/bin/cursor-agent"),(AgentKind::Grok,"/Users/vishayagarwal/.grok/bin/grok"),(AgentKind::OpenCode,"/opt/homebrew/bin/opencode"),(AgentKind::Antigravity,"/Users/vishayagarwal/.local/bin/agy")]{
   let info=crate::agent_controls::inspect_project(kind,Path::new(path),&home,&home);
   if kind==AgentKind::Cursor{assert_eq!(info.defaults.model.as_deref(),Some("auto"),"Native Cursor combined current/default marker must resolve");}
   println!("{}: {} · {} ({} choices)",kind.name(),info.defaults.model.unwrap_or_else(||"not reported".into()),info.defaults.effort.unwrap_or_else(||"provider-managed".into()),info.models.len());
  }
 }
}

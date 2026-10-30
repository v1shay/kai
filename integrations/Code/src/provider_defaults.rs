//! Read only model-selection fields. Native protocol/config data stays separate
//! from explicit per-chat overrides and never chooses a replacement provider.
use crate::{agent_controls::{ControlInfo,ModelDefaults},discovery::AgentKind,runtime::valid_model_id};
use serde_json::Value;
use std::{path::{Path,PathBuf},fs,io::Read};
/// CLI model menus use base IDs; ACP advertises their fully parameterized IDs.
/// Resolve only an unambiguous advertised variant; explicit overrides stay exact.
pub fn acp_model_id(requested:&str,models:&std::collections::HashSet<String>)->Option<String>{
 if models.contains(requested){return Some(requested.into())}
 if requested.contains('['){return None}
 let prefix=format!("{requested}[");let mut matches=models.iter().filter(|id|id.starts_with(&prefix));
 let value=matches.next()?.clone();matches.next().is_none().then_some(value)
}
fn text(v:&Value)->Option<String>{v.as_str().filter(|s|valid_model_id(s)).map(str::to_owned)}
pub fn acp_selection(value:&Value)->ModelDefaults{
 let mut out=ModelDefaults::default();
 out.model=text(&value["models"]["currentModelId"]).or_else(||text(&value["currentModelId"]));
 for option in value["configOptions"].as_array().into_iter().flatten(){
  let id=option["category"].as_str().or_else(||option["id"].as_str()).unwrap_or("");
  match id{"model"=>{if let Some(model)=text(&option["currentValue"]){out.model=Some(model)}},"thought_level"|"reasoning_effort"|"reasoning"|"thinking"|"thinking_level"|"effort"=>out.effort=text(&option["currentValue"]),_=>{}}
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

/// Exact native values, including grouped ACP options. Only reasoning selectors
/// qualify; unrelated model config (temperature, context size) cannot become effort.
pub fn acp_effort_option(value:&Value)->Option<(String,Vec<String>)>{
 for option in value["configOptions"].as_array().into_iter().flatten(){
  let Some(id)=option["id"].as_str()else{continue};let category=option["category"].as_str().unwrap_or(id);
  if category!="thought_level" && !["effort","reasoning_effort","thinking_level","thought_level","reasoning","thinking"].contains(&id){continue}
  if option["type"].as_str().is_some_and(|kind|kind!="select"){continue}
  let mut levels=Vec::new();
  for group in option["options"].as_array().into_iter().flatten(){
   let entries=if group["options"].is_array(){group["options"].as_array().unwrap().iter().collect::<Vec<_>>()}else{vec![group]};
   for entry in entries{if let Some(level)=entry["value"].as_str().filter(|v|!v.is_empty()&&v.len()<=128&&!v.chars().any(char::is_control)){if !levels.iter().any(|v|v==level)&&levels.len()<16{levels.push(level.to_owned())}}}
  }
  if !levels.is_empty(){return Some((id.into(),levels))}
 }
 None
}
/// OpenCode exposes provider/model variants, whose IDs must be sent unchanged.
pub fn opencode_models(value:&Value)->Vec<crate::agent_controls::ModelChoice>{
 let mut models=Vec::new();
 for provider in value["all"].as_array().or_else(||value["providers"].as_array()).into_iter().flatten(){
  let Some(provider_id)=provider["id"].as_str()else{continue};
  for (id,model) in provider["models"].as_object().into_iter().flatten(){
   let full=format!("{provider_id}/{id}");if !valid_model_id(&full){continue}
   let mut variants=model["variants"].as_object().into_iter().flatten().filter(|(_,v)|v["disabled"]!=true).filter_map(|(id,v)|{
    if !valid_model_id(id){return None}
    let effort=v["reasoningEffort"].as_str().or_else(||v["options"]["reasoningEffort"].as_str()).unwrap_or(id);
    let rank=["none","minimal","low","medium","high","xhigh","max","ultra"].iter().position(|v|*v==effort).unwrap_or(8);
    Some((rank,id.clone()))
   }).collect::<Vec<_>>();variants.sort_by(|a,b|a.0.cmp(&b.0));
   models.push(crate::agent_controls::ModelChoice{id:full,label:model["name"].as_str().unwrap_or(id).chars().take(160).collect(),reasoning_efforts:variants.into_iter().take(16).map(|(_,id)|id).collect()});
   if models.len()>=256{return models}
  }
 }
 models
}

pub fn acp_models(value:&Value)->Vec<crate::agent_controls::ModelChoice>{
 let mut models=Vec::new();
 for model in value["models"]["availableModels"].as_array().into_iter().flatten().take(256){
  if let Some(id)=model["modelId"].as_str().or_else(||model["id"].as_str()).filter(|id|valid_model_id(id)){
   models.push(crate::agent_controls::ModelChoice{id:id.into(),label:model["name"].as_str().unwrap_or(id).chars().take(160).collect(),reasoning_efforts:Vec::new()});
  }
 }
 for option in value["configOptions"].as_array().into_iter().flatten().filter(|v|v["category"]=="model" || v["id"]=="model"){
  for group in option["options"].as_array().into_iter().flatten(){
   let entries=if let Some(options)=group["options"].as_array(){options.iter().collect::<Vec<_>>()}else{vec![group]};
   for model in entries{if let Some(id)=model["value"].as_str().filter(|id|valid_model_id(id)){if models.len()<256 && !models.iter().any(|m|m.id==id){models.push(crate::agent_controls::ModelChoice{id:id.into(),label:model["name"].as_str().unwrap_or(id).chars().take(160).collect(),reasoning_efforts:Vec::new()})}}}
  }
 }
 let selected=acp_selection(value);
 if let Some((_,levels))=acp_effort_option(value){if let Some(model)=models.iter_mut().find(|m|Some(&m.id)==selected.model.as_ref()){model.reasoning_efforts=levels}}
 models
}

#[cfg(test)]mod capability_tests{
 use super::*;
 #[test]fn acp_reasoning_is_exact_grouped_and_applies_only_to_selected_model(){
  let value=serde_json::json!({"models":{"currentModelId":"gemini-one","availableModels":[{"modelId":"gemini-one"},{"modelId":"gemini-two"}]},"configOptions":[{"id":"temperature","category":"model_config","options":[{"value":"hot"}]},{"id":"budget-choice","category":"thought_level","type":"select","currentValue":"high","options":[{"group":"Thinking","options":[{"value":"low"},{"value":"high"}]}]}]});
  assert_eq!(acp_effort_option(&value),Some(("budget-choice".into(),vec!["low".into(),"high".into()])));
  let models=acp_models(&value);assert_eq!(models[0].reasoning_efforts,vec!["low","high"]);assert!(models[1].reasoning_efforts.is_empty());
  assert!(acp_effort_option(&serde_json::json!({"configOptions":[{"id":"temperature","category":"model_config","options":[{"value":"high"}]}]})).is_none());
 }
 #[test]fn config_only_acp_catalogs_do_not_require_legacy_model_metadata(){
  let value=serde_json::json!({"configOptions":[{"id":"selected","category":"model","currentValue":"model-b","options":[{"value":"model-a","name":"Model A"},{"value":"model-b","name":"Model B"}]},{"id":"effort","category":"thought_level","currentValue":"high","options":[{"value":"low"},{"value":"high"}]}]});
  let models=acp_models(&value);assert_eq!(models.len(),2);assert!(models[0].reasoning_efforts.is_empty());assert_eq!(models[1].reasoning_efforts,vec!["low","high"]);
 }
 #[test]fn opencode_variants_preserve_native_ids_and_order_by_real_effort(){
  let value=serde_json::json!({"all":[{"id":"openai","models":{"gpt-6.1-sol":{"name":"Sol","variants":{"deep":{"reasoningEffort":"high"},"fast":{"reasoningEffort":"low"},"disabled":{"disabled":true}}}}}]});
  let models=opencode_models(&value);assert_eq!(models[0].id,"openai/gpt-6.1-sol");assert_eq!(models[0].reasoning_efforts,vec!["fast","deep"]);
 }
}

/// The installed CLI prints an ID line followed by a complete JSON model.
/// Parse JSON framing, not brace counts or a guessed list of reasoning levels.
pub fn opencode_verbose_models(text:&str)->Vec<crate::agent_controls::ModelChoice>{
 let mut out=Vec::new();let mut remaining=text;
 while !remaining.trim().is_empty() && out.len()<256{
  remaining=remaining.trim_start();let Some(end)=remaining.find('\n')else{break};
  let id=remaining[..end].trim();let body=remaining[end+1..].trim_start();
  let Some((provider,model_id))=id.split_once('/').filter(|_|valid_model_id(id))else{remaining=body;continue};
  let mut stream=serde_json::Deserializer::from_str(body).into_iter::<Value>();
  if let Some(Ok(model))=stream.next(){
   let mut models=serde_json::Map::new();models.insert(model_id.into(),model);
   out.extend(opencode_models(&serde_json::json!({"all":[{"id":provider,"models":models}]})));
   remaining=&body[stream.byte_offset()..];
  }else{remaining=body;}
 }
 out
}
#[cfg(test)]mod verbose_catalog_tests{
 use super::*;
 #[test]fn cli_metadata_preserves_native_variants_before_a_first_message(){
  let text="openai/gpt-6.1-sol\n{\"name\":\"Sol\",\"variants\":{\"deep\":{\"reasoningEffort\":\"high\"},\"fast\":{\"reasoningEffort\":\"low\"}}}\nopenai/gpt-6-luna\n{\"name\":\"Luna\",\"variants\":{\"low\":{},\"high\":{}}}\n";
  let models=opencode_verbose_models(text);assert_eq!(models.len(),2);assert_eq!(models[0].reasoning_efforts,vec!["fast","deep"]);assert_eq!(models[1].id,"openai/gpt-6-luna");
 }
}

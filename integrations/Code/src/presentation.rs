//! Pure presentation work. Parse on a worker; render only visible bounded rows.
use crate::chat_interactions::{self, ChatInteraction, InteractionStatus, Speaker};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde_json::Value;
use std::{ops::Range, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineStyle {
    Bold,
    Italic,
    Code,
    Link(Arc<str>),
    Strike,
}

/// GPUI expects ordered, disjoint text runs. Split nested Markdown spans into
/// runs carrying every applicable style, rather than losing bold inside links.
pub fn inline_runs(
    text_len: usize,
    highlights: &[(Range<usize>, InlineStyle)],
) -> Vec<(Range<usize>, Vec<InlineStyle>)> {
    // Sweep ordered span boundaries instead of scanning every highlight for
    // every segment. Large outputs with thousands of links used to do O(n²)
    // work on the UI thread, even when no spans overlapped.
    let mut events = Vec::with_capacity(highlights.len() * 2);
    for (index, (range, _)) in highlights.iter().enumerate() {
        let start = range.start.min(text_len);
        let end = range.end.min(text_len);
        if start < end {
            events.push((start, true, index));
            events.push((end, false, index));
        }
    }
    events.sort_unstable();
    let mut active = std::collections::BTreeSet::new();
    let mut previous = 0;
    let mut result = Vec::new();
    for (at, starting, index) in events {
        if at > previous && !active.is_empty() {
            result.push((
                previous..at,
                active
                    .iter()
                    .map(|index: &usize| highlights[*index].1.clone())
                    .collect(),
            ));
        }
        if starting {
            active.insert(index);
        } else {
            active.remove(&index);
        }
        previous = at;
    }
    result
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MessageBlock {
    Voice {
        speaker: Speaker,
        text: String,
        highlights: Vec<(Range<usize>, InlineStyle)>,
    },
    Interaction(ChatInteraction),
    Attachment {
        name: String,
        target: String,
    },
    Paragraph(String),
    RichParagraph {
        text: String,
        highlights: Vec<(Range<usize>, InlineStyle)>,
    },
    Activity {
        label: String,
        details: String,
    },
    Heading(String),
    Code {
        language: String,
        text: String,
        highlights: Vec<(Range<usize>, u32)>,
    },
    Quote(String),
    ListItem {
        marker: String,
        text: String,
        highlights: Vec<(Range<usize>, InlineStyle)>,
    },
    TableRow {
        cells: Vec<String>,
        header: bool,
    },
}

pub fn ease_out(progress: f32) -> f32 {
    1. - (1. - progress.clamp(0., 1.)).powi(3)
}

/// Native harness context names. Unknown XML/HTML remains ordinary message text.
pub const HARNESS_TAGS: &[&str] = &[
    "environment_context",
    "user_instructions",
    "developer_instructions",
    "system-reminder",
    "system_reminder",
    "skills_instructions",
    "codex_apps_client_time_context",
    "app-context",
    "permissions instructions",
    "collaboration_mode",
    "multi_agent_role",
    "multi_agent_mode",
    "turn_aborted",
    "INSTRUCTIONS",
    "recommended_plugins",
    "code_handoff_context",
    "image_resize_notice",
    "codex_apps_open_page_instructions",
    "external_codex_apps_open_page",
    "environments_instructions",
    "apps_instructions",
    "plugins_instructions",
    "tools",
    "context_window",
    "context_window_guidance",
    "guardian_tool_descriptions",
    "realtime_conversation",
];

pub fn harness_tag(name: &str, assistant: bool) -> Option<(&str, bool, bool)> {
    let closing = name.starts_with('/');
    let body = name.trim_start_matches('/').trim();
    HARNESS_TAGS
        .iter()
        .copied()
        .chain(assistant.then_some("analysis"))
        .chain(assistant.then_some("thinking"))
        .find_map(|tag| {
            let suffix = body.strip_prefix(tag)?;
            if suffix.is_empty() || suffix.starts_with(char::is_whitespace) || suffix == "/" {
                Some((tag, closing, suffix.trim_end().ends_with('/')))
            } else {
                None
            }
        })
}

/// A constant-size envelope scanner that survives page boundaries. Its snapshots
/// live in the sparse disk index, so a middle page cannot expose context bodies.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct HarnessState {
    position: usize,
    active: Option<(String, u32, usize)>,
    candidate: Option<(usize, String)>,
    inline_ticks: usize,
    tick_run: usize,
    json: bool,
    json_quote: bool,
    json_escape: bool,
    started: bool,
}
impl HarnessState {
    pub fn hidden(&self) -> bool {
        self.active.is_some()
    }
    pub fn feed(&mut self, byte: u8, code: bool, assistant: bool) -> Option<Range<usize>> {
        let at = self.position;
        self.position += 1;
        if !self.started && !byte.is_ascii_whitespace() {
            self.started = true;
            self.json = matches!(byte, b'{' | b'[');
        }
        if self.json && self.active.is_none() {
            if self.json_escape {
                self.json_escape = false;
                return None;
            }
            if byte == b'\\' && self.json_quote {
                self.json_escape = true;
                return None;
            }
            if byte == b'"' {
                self.json_quote = !self.json_quote;
                return None;
            }
            if self.json_quote {
                return None;
            }
        }
        if !self.hidden() {
            if code {
                self.inline_ticks = 0;
                self.tick_run = 0;
                return None;
            }
            if byte == b'`' && !code {
                self.tick_run += 1;
                return None;
            }
            if self.tick_run > 0 {
                if self.inline_ticks == 0 {
                    self.inline_ticks = self.tick_run;
                } else if self.inline_ticks == self.tick_run {
                    self.inline_ticks = 0;
                }
                self.tick_run = 0;
            }
            if code || self.inline_ticks > 0 {
                return None;
            }
        }
        if byte == b'<' {
            self.candidate = Some((at, String::new()));
        } else if let Some((start, name)) = self.candidate.take() {
            if byte == b'>' {
                if let Some((tag, closing, single)) = harness_tag(&name, assistant) {
                    if let Some((open, depth, _)) = self.active.as_mut() {
                        if tag == open {
                            if closing {
                                *depth -= 1;
                                if *depth == 0 {
                                    let (_, _, begin) = self.active.take().unwrap();
                                    self.started = false;
                                    self.json = false;
                                    self.json_quote = false;
                                    self.json_escape = false;
                                    return Some(begin..self.position);
                                }
                            } else if !single {
                                *depth = depth.saturating_add(1);
                            }
                        }
                    } else if closing || single {
                        return Some(start..self.position);
                    } else {
                        self.active = Some((tag.to_owned(), 1, start));
                    }
                }
            } else if name.len() < 192 && byte.is_ascii() {
                let mut name = name;
                name.push(byte as char);
                self.candidate = Some((start, name));
            }
        }
        None
    }
    pub fn pending_range(&self) -> Option<Range<usize>> {
        self.active
            .as_ref()
            .map(|(_, _, start)| *start..self.position)
    }
}

/// Removes recognized native harness envelopes. Ordinary JSON and fenced source
/// examples survive. Call at every history boundary, including old caches.
pub fn sanitize_transcript_text(role: &str, text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if role == "tool" {
        return Some(text.to_owned());
    }
    if trimmed.starts_with("The following is the Codex agent history added since")
        || trimmed.starts_with("# AGENTS.md instructions for ")
        || trimmed.starts_with(
            "This session is being continued from a previous conversation that ran out of context.",
        )
    {
        return None;
    }
    if role == "assistant" && approval_decision(trimmed) {
        return None;
    }
    let mut lines = Vec::new();
    let mut offset = 0;
    let mut fence: Option<(u8, usize)> = None;
    for line in text.split_inclusive('\n') {
        let body = line.trim_start();
        let token = body.as_bytes().first().copied();
        let count = token
            .map(|b| body.bytes().take_while(|&v| v == b).count())
            .unwrap_or(0);
        let delimiter = matches!(token, Some(b'`' | b'~')) && count >= 3;
        let in_code = fence.is_some();
        if delimiter {
            if let Some((character, length)) = fence {
                if token == Some(character) && count >= length && body[count..].trim().is_empty() {
                    fence = None;
                }
            } else {
                fence = Some((token.unwrap(), count));
            }
        }
        if !in_code && !delimiter {
            lines.push((offset, offset + line.len(), line.trim()));
        }
        offset += line.len();
    }
    let planned = lines
        .iter()
        .find(|(_, _, line)| line.trim_matches('*').starts_with("Planned action JSON:"));
    if let Some((start, _, _)) = planned
        && lines
            .iter()
            .any(|(at, _, line)| at > start && line.contains("APPROVAL REQUEST END"))
    {
        return None;
    }
    let mut removed = Vec::<Range<usize>>::new();
    let mut state = HarnessState::default();
    let mut fence: Option<(u8, usize)> = None;
    for line in text.split_inclusive('\n') {
        let hidden = state.hidden();
        let body = line.trim_start();
        let token = body.as_bytes().first().copied().unwrap_or(0);
        let count = body.bytes().take_while(|&b| b == token).count();
        let delimiter = matches!(token, b'`' | b'~') && count >= 3;
        let protected = !hidden && (fence.is_some() || delimiter);
        for byte in line.bytes() {
            if let Some(range) = state.feed(byte, protected, role == "assistant") {
                removed.push(range);
            }
        }
        if delimiter && !hidden {
            if let Some((character, length)) = fence {
                if token == character && count >= length && body[count..].trim().is_empty() {
                    fence = None;
                }
            } else {
                fence = Some((token, count));
            }
        }
    }
    if let Some(range) = state.pending_range() {
        removed.push(range);
    }
    removed.sort_by_key(|range| range.start);
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    for range in removed {
        if range.start >= cursor {
            output.push_str(&text[cursor..range.start]);
        }
        cursor = cursor.max(range.end);
    }
    output.push_str(&text[cursor..]);
    let mut cleaned = output.trim();
    if role == "assistant"
        && let Some(body) = cleaned.strip_prefix("<final>")
    {
        cleaned = body.strip_suffix("</final>").unwrap_or(body).trim();
    }
    if cleaned.is_empty() || (role == "assistant" && approval_decision(cleaned)) {
        None
    } else {
        Some(cleaned.to_owned())
    }
}

fn approval_decision(text: &str) -> bool {
    if !text.starts_with('{') {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    matches!(
        value
            .get("outcome")
            .or_else(|| value.get("decision"))
            .and_then(Value::as_str),
        Some("allow" | "deny" | "ask_user")
    ) && value.get("risk_level").and_then(Value::as_str).is_some()
        && value
            .get("user_authorization")
            .and_then(Value::as_str)
            .is_some()
        && value.get("rationale").and_then(Value::as_str).is_some()
}

/// Bound transport previews at native record boundaries. Native tool payloads
/// remain typed cards; JSON escaping never becomes visible transcript prose.
/// Full details remain in the immutable source for history/review.
pub(crate) fn compact_transcript(text:&str,budget:usize)->String{
    let protected=code_protected_ranges(text);let literal=|at|protected.iter().any(|r|r.contains(&at));
    let mut out=String::new();let mut cursor=0;let mut search=0;
    // Older byte tails can begin inside an app-owned record. Its closing tag
    // identifies the end without interpreting arbitrary JSON or source code.
    if let Some(close)=text.find("</orbit_interaction>"){
        if !literal(close)&&text[..close].find("<orbit_interaction").is_none(){cursor=close+"</orbit_interaction>".len();search=cursor;}
    }
    while let Some(at)=text[search..].find("<orbit_interaction").map(|i|i+search){
        search=at+1;if literal(at){continue}
        let Some(("orbit_interaction",body))=opening_tag(text,at)else{continue};if !text[at..body].contains("version=\"1\""){continue}
        let Some(close)=text[body..].find("</orbit_interaction>").map(|i|i+body)else{break};
        let Some((mut card,context))=chat_interactions::decode_with_context(&text[body..close])else{continue};
        let limit=(budget/4).clamp(256,4096);
        match &mut card{ChatInteraction::Status{details,..}|ChatInteraction::Delegation{details,..}=>{if details.len()>limit{let mut end=limit;while !details.is_char_boundary(end){end-=1}details.truncate(end);details.push_str("\n…");}},_=>{}}
        out.push_str(&text[cursor..at]);
        out.push_str(&format!("<orbit_interaction version=\"1\">{}</orbit_interaction>",chat_interactions::encode_with_context(&card,context.as_deref()).replace('<',"\\u003c")));
        cursor=close+"</orbit_interaction>".len();search=cursor;
    }
    out.push_str(&text[cursor..]);
    if out.len()<=budget{return out}
    let mut start=out.len()-budget;while !out.is_char_boundary(start){start+=1}
    // Retain a whole native record, or start after it; never emit its suffix.
    if let Some(close)=out[start..].find("</orbit_interaction>"){
        if out[start..start+close].find("<orbit_interaction").is_none(){start+=close+"</orbit_interaction>".len();}
    }
    if let Some(range)=code_protected_ranges(&out).iter().find(|r|r.contains(&start)){
        if let Some(header)=out[range.start..].lines().next().filter(|s|s.trim_start().starts_with("```")||s.trim_start().starts_with("~~~")){
            return format!("{header}\n{}",out[start..].trim_start());
        }
    }
    out[start..].trim_start().to_owned()
}

pub fn message_blocks(role: &str, text: &str) -> Vec<MessageBlock> {
    let Some(text) = sanitize_transcript_text(role, text) else {
        return Vec::new();
    };
    if role == "tool"
        && let Some((card, context)) = chat_interactions::decode_with_context(&text)
    {
        return bounded_interaction_with_context(card, context.as_deref());
    }
    if role == "tool"
        && let Ok(value) = serde_json::from_str::<Value>(&text)
        && value["orbit_tool"] == true
    {
        return vec![MessageBlock::Activity {
            label: value["label"].as_str().unwrap_or("Used a tool").into(),
            details: value["details"].as_str().unwrap_or("").into(),
        }];
    }
    // App-owned association only: repository/session/root checks are performed
    // again on a worker before the preview and explicit restore action.
    if role == "user"
        && let Some(prefix) = text.strip_suffix("-->")
        && let Some((body, id)) = prefix.rsplit_once("\n\n<!--code-checkpoint:")
        && uuid::Uuid::parse_str(id).is_ok()
    {
        let mut result = blocks(body);
        result.push(MessageBlock::Activity {
            label: "Prompt changes".into(),
            details: id.into(),
        });
        return result;
    }
    blocks(&text)
}

struct MarkdownState {
    buffer: String,
    heading: bool,
    quotes: usize,
    bullets: usize,
    code: Option<String>,
    spans: Vec<(Range<usize>, InlineStyle)>,
    active: Vec<(InlineStyle, usize)>,
    lists: Vec<Option<u64>>,
    markers: Vec<String>,
    table: bool,
    table_header: bool,
    cells: Vec<String>,
}
impl MarkdownState {
    fn emit(&mut self, result: &mut Vec<MessageBlock>) {
        if self.table {
            return;
        }
        let body = if self.code.is_some() {
            self.buffer.as_str()
        } else {
            self.buffer.trim()
        };
        let mut row_start = if self.code.is_some() {
            0
        } else {
            self.buffer.len() - self.buffer.trim_start().len()
        };
        let mut lex = LexState::Normal;
        // Span closing order can differ from position order for nested styles.
        // Sweep starts once per block; preserve original style precedence within
        // each row instead of rescanning every block span for every bounded row.
        let mut starts: Vec<_> = (0..self.spans.len()).collect();
        starts.sort_unstable_by_key(|&ix| self.spans[ix].0.start);
        let mut next = 0;
        let mut active: Vec<usize> = Vec::new();
        for row in chunks(body) {
            let row_end = row_start + row.len();
            active.retain(|&ix| self.spans[ix].0.end > row_start);
            while next < starts.len() && self.spans[starts[next]].0.start < row_end {
                let ix = starts[next];
                if self.spans[ix].0.end > row_start {
                    active.push(ix);
                }
                next += 1;
            }
            active.sort_unstable();
            let spans = active
                .iter()
                .filter_map(|&ix| {
                    let (range, style) = &self.spans[ix];
                    let start = range.start.max(row_start);
                    let end = range.end.min(row_end);
                    (end > start).then(|| ((start - row_start)..(end - row_start), style.clone()))
                })
                .collect::<Vec<_>>();
            result.push(if let Some(language) = &self.code {
                MessageBlock::Code {
                    language: language.clone(),
                    text: row.to_owned(),
                    highlights: highlight(row, language, &mut lex),
                }
            } else if self.heading {
                MessageBlock::Heading(row.to_owned())
            } else if self.bullets > 0 {
                MessageBlock::ListItem {
                    marker: if row_start == self.buffer.len() - self.buffer.trim_start().len() {
                        self.markers.last().cloned().unwrap_or("•".into())
                    } else {
                        String::new()
                    },
                    text: row.to_owned(),
                    highlights: spans,
                }
            } else if self.quotes > 0 {
                MessageBlock::Quote(row.to_owned())
            } else if !spans.is_empty() {
                MessageBlock::RichParagraph {
                    text: row.to_owned(),
                    highlights: spans,
                }
            } else {
                MessageBlock::Paragraph(row.to_owned())
            });
            row_start = row_end;
        }
        self.buffer.clear();
        self.spans.clear();
    }
}

/// Bounded Markdown rows, parsed off the UI thread. HTML never executes.
pub fn blocks(text: &str) -> Vec<MessageBlock> {
    let coherent=coherent_voice_source(text);let text=coherent.as_ref();
    if let Some(blocks) = interaction_blocks(text) {
        return blocks;
    }
    markdown_blocks(text)
}

/// Parse only the bounded live tail on a worker. Withhold incomplete protocol
/// envelopes until their closing tag arrives, while retaining ordinary prose,
/// literal JSON/XML, and fenced examples. Completed cards use the same parser
/// as persisted history.
pub fn live_blocks(text: &str) -> Vec<MessageBlock> {
    const TAIL_CHARS: usize = 6000;
    let start = text
        .char_indices()
        .rev()
        .nth(TAIL_CHARS - 1)
        .map_or(0, |(at, _)| at);
    let text = if start > 0 {
        let boundary = text[start..].find('\n').map_or(start, |at| start + at + 1);
        &text[boundary..]
    } else {
        text
    };
    live_page_blocks(text)
}

/// Caller owns the byte budget; unlike the small tail helper this retains the
/// complete loaded page so scrolling never silently drops earlier work.
pub(crate) fn live_page_blocks(text:&str)->Vec<MessageBlock>{
    let coherent=coherent_voice_source(text);let text=coherent.as_ref();
    if matches!(text.trim_start().as_bytes().first(), Some(b'{' | b'['))
        && (!text.contains("</orbit_interaction>") || serde_json::from_str::<Value>(text).is_ok())
    {
        return markdown_blocks(text);
    }
    let protected = code_protected_ranges(text);
    let is_literal = |at: usize| protected.iter().any(|range| range.contains(&at));
    // A trimmed tail may start within a private single-line envelope. Its exact
    // closing marker gives us a safe boundary without guessing ordinary JSON.
    let mut begin = 0;
    if let Some(end) = text.find("</orbit_interaction>")
        && !is_literal(end)
        && !text[..end].contains("<orbit_interaction")
    {
        begin = end + "</orbit_interaction>".len();
    }
    let mut end = text.len();
    let mut search = begin;
    while let Some(relative) = text[search..].find('<') {
        let at = search + relative;
        search = at + 1;
        if is_literal(at) {
            continue;
        }
        let suffix = &text[at..];
        let complete = opening_tag(text, at);
        if let Some((name, body_start)) = complete
            && matches!(
                name,
                "orbit_interaction" | "send_user_message_question_reply"
            )
        {
            let (_, after) = closing_tag(text, body_start, name, &protected);
            if after == text.len() && !text[body_start..].ends_with(&format!("</{name}>")) {
                end = at;
                break;
            }
            search = after;
            continue;
        }
        if !suffix.contains('>') {
            let name = suffix.trim_start_matches('<').trim_start_matches('/');
            let native_partial = [
                "orbit_interaction",
                "realtime_delegation",
                "realtime_conversation",
                "send_user_message_question_reply",
                "transcript_delta",
                "input",
                "source",
            ]
            .iter()
            .chain(HARNESS_TAGS.iter())
            .any(|known| {
                name == *known
                    || (name.len() >= 7 && known.starts_with(name))
                    || name
                        .strip_prefix(known)
                        .is_some_and(|tail| tail.starts_with(char::is_whitespace))
            });
            if native_partial {
                end = at;
                break;
            }
        }
    }
    message_blocks("assistant", &text[begin..end])
}

/// A spoken relay of the backend answer is an alternate presentation of that
/// answer, not a second response. Scope this to each user utterance; independent
/// voice-only exchanges stay intact. Removing the owned separators also rejoins
/// Markdown links that a voice event arrived in the middle of.
fn coherent_voice_source(text:&str)->std::borrow::Cow<'_,str>{
    if !text.contains("<orbit_interaction")||!text.contains("\"speaker\""){return text.into()}
    let protected=code_protected_ranges(text);let mut markers=vec![];let mut written=vec![false];let mut group=0;let mut cursor=0;let mut search=0;
    while let Some(at)=text[search..].find("<orbit_interaction").map(|i|i+search){
        search=at+1;if protected.iter().any(|r|r.contains(&at)){continue}
        let Some(("orbit_interaction",body))=opening_tag(text,at)else{continue};
        if !text[at..body].contains("version=\"1\""){continue}
        let(end,after)=closing_tag(text,body,"orbit_interaction",&protected);
        let Some((card,_))=chat_interactions::decode_with_context(&text[body..end])else{continue};
        written[group]|=!text[cursor..at].trim().is_empty();
        if matches!(card,ChatInteraction::Speaker{speaker:Speaker::User,..}){group+=1;written.push(false)}
        if matches!(card,ChatInteraction::Speaker{speaker:Speaker::Assistant,..}){markers.push((at..after,group))}
        cursor=after;search=after;
    }
    written[group]|=!text[cursor..].trim().is_empty();let mut omit=vec![];
    for (mut range,g) in markers{if written[g]{
        if text[..range.start].ends_with("\n\n"){range.start-=2}if text[range.end..].starts_with("\n\n"){range.end+=2}omit.push(range)
    }}
    if omit.is_empty(){return text.into()}
    let mut result=String::with_capacity(text.len());let mut cursor=0;
    for range in omit{if range.start>cursor{result.push_str(&text[cursor..range.start])}cursor=cursor.max(range.end)}result.push_str(&text[cursor..]);result.into()
}

#[cfg(test)]mod coherent_voice_tests{
 use super::*;
 #[test]fn written_answer_rejoins_links_and_hides_its_spoken_relay(){
  let spoken=chat_interactions::serialize_interaction(&ChatInteraction::Speaker{speaker:Speaker::Assistant,text:"Here are three quick headlines".into()});
  let source=format!("Read [OpenAI](https://openai.com/ter\n\n{spoken}\n\nra/) for details.");
  let blocks=live_page_blocks(&source);assert!(!blocks.iter().any(|b|matches!(b,MessageBlock::Voice{..})));
  assert!(blocks.iter().any(|b|matches!(b,MessageBlock::RichParagraph{highlights,..} if highlights.iter().any(|(_,style)|matches!(style,InlineStyle::Link(url) if url.as_ref()=="https://openai.com/terra/")))));
 }
 #[test]fn a_separate_voice_exchange_and_literal_examples_are_preserved(){
  let user=chat_interactions::serialize_interaction(&ChatInteraction::Speaker{speaker:Speaker::User,text:"Thanks".into()});
  let spoken=chat_interactions::serialize_interaction(&ChatInteraction::Speaker{speaker:Speaker::Assistant,text:"You are welcome".into()});
  let source=format!("Written answer\n\n{user}\n\n{spoken}");assert!(blocks(&source).iter().any(|b|matches!(b,MessageBlock::Voice{speaker:Speaker::Assistant,text,..} if text=="You are welcome")));
  let literal=format!("Example:\n```xml\n{spoken}\n```");assert_eq!(coherent_voice_source(&literal).as_ref(),literal);
 }
}

fn markdown_blocks(text: &str) -> Vec<MessageBlock> {
    if let Some(blocks) = directive_blocks(text) {
        return blocks;
    }
    markdown_rows(text)
}

/// Desktop directives are presentation data, never instructions or executable
/// actions. Recognize only complete, standalone, bounded envelopes. Fenced and
/// inline examples, malformed directives, JSON, and unknown extensions survive.
fn directive_blocks(text: &str) -> Option<Vec<MessageBlock>> {
    if !(text.contains("::code-comment{")
        || text.contains(":::writing{")
        || text.contains("<proposed_plan>"))
        || matches!(text.trim_start().as_bytes().first(), Some(b'{' | b'['))
    {
        return None;
    }
    let protected = code_protected_ranges(text);
    let mut protected_ix = 0;
    let mut result = Vec::new();
    let mut cursor = 0;
    let mut offset = 0;
    let mut lines = text.split_inclusive('\n');
    while let Some(line) = lines.next() {
        let start = offset;
        offset += line.len();
        while protected_ix < protected.len() && protected[protected_ix].end <= start {
            protected_ix += 1;
        }
        if protected
            .get(protected_ix)
            .is_some_and(|range| range.contains(&start))
        {
            continue;
        }
        // Indented blocks are source examples; quoted/list directives remain
        // ordinary Markdown rather than acquiring unexpected native actions.
        let body = line.trim_end();
        if body.starts_with("::code-comment{") {
            let Some(fields) = directive_attributes(body, "::code-comment") else {
                continue;
            };
            let Some(title) = fields.get("title").filter(|text| !text.is_empty()) else {
                continue;
            };
            let Some(details) = fields.get("body") else {
                continue;
            };
            result.extend(markdown_rows(&text[cursor..start]));
            result.push(MessageBlock::Interaction(ChatInteraction::Status {
                title: format!("Review · {title}"),
                details: details.clone(),
                status: InteractionStatus::Completed,
            }));
            if let Some(file) = fields
                .get("file")
                .filter(|path| !path.is_empty() && !path.chars().any(char::is_control))
            {
                let number = |key: &str| {
                    fields
                        .get(key)
                        .and_then(|value| value.parse::<u64>().ok())
                        .filter(|line| *line > 0)
                };
                let start_line = number("start");
                let end_line =
                    number("end").filter(|end| start_line.is_some_and(|start| *end >= start));
                let location = match (start_line, end_line) {
                    (Some(start), Some(end)) => format!("#L{start}-L{end}"),
                    (Some(start), None) => format!("#L{start}"),
                    _ => String::new(),
                };
                let label = format!("{file}{location}");
                // Encode only path bytes that conflict with link location
                // syntax. Never let a filename turn into a scheme/action.
                let target = format!("{}{location}", file.replace('%', "%25").replace('#', "%23"));
                result.push(MessageBlock::RichParagraph {
                    highlights: vec![(0..label.len(), InlineStyle::Link(target.into()))],
                    text: label,
                });
            }
            cursor = offset;
        } else if body.starts_with(":::writing{") || body == "<proposed_plan>" {
            let writing = body != "<proposed_plan>";
            let title = if writing {
                let Some(fields) = directive_attributes(body, ":::writing") else {
                    continue;
                };
                let variant = match fields.get("variant").map(String::as_str) {
                    Some("email") => "Email draft",
                    Some("chat_message") => "Message draft",
                    Some("social_post") => "Social post",
                    Some("document") => "Document",
                    Some("standard") => "Draft",
                    _ => continue,
                };
                fields
                    .get("subject")
                    .filter(|value| !value.is_empty())
                    .map_or_else(
                        || variant.to_owned(),
                        |subject| format!("{variant} · {subject}"),
                    )
            } else {
                "Proposed plan".to_owned()
            };
            let content_start = offset;
            let mut close = None;
            // One forward scan. An unfinished envelope remains literal; never
            // repeatedly rescan a long incomplete response for each opener.
            for next in lines.by_ref() {
                let line_start = offset;
                offset += next.len();
                if next.trim_end() == if writing { ":::" } else { "</proposed_plan>" }
                    && !protected
                        .get(protected.partition_point(|range| range.end <= line_start))
                        .is_some_and(|range| range.contains(&line_start))
                {
                    close = Some(line_start);
                    break;
                }
            }
            let Some(content_end) = close else { break };
            if content_end - content_start > 64 * 1024 {
                continue;
            }
            result.extend(markdown_rows(&text[cursor..start]));
            result.push(MessageBlock::Interaction(ChatInteraction::Status {
                title,
                details: text[content_start..content_end].trim_end().to_owned(),
                status: InteractionStatus::Completed,
            }));
            cursor = offset;
        }
    }
    if cursor == 0 {
        return None;
    }
    result.extend(markdown_rows(&text[cursor..]));
    Some(result)
}

fn directive_attributes(
    text: &str,
    name: &str,
) -> Option<std::collections::BTreeMap<String, String>> {
    if text.len() > 16 * 1024 {
        return None;
    }
    let mut rest = text
        .strip_prefix(name)?
        .strip_prefix('{')?
        .strip_suffix('}')?;
    let mut fields = std::collections::BTreeMap::new();
    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();
        let key_end = rest
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            .count();
        if key_end == 0 || fields.len() >= 16 {
            return None;
        }
        let key = &rest[..key_end];
        rest = rest[key_end..].trim_start().strip_prefix('=')?.trim_start();
        let value;
        if rest.starts_with('"') {
            let mut escaped = false;
            let mut end = None;
            for (at, ch) in rest.char_indices().skip(1) {
                if escaped {
                    escaped = false;
                    continue;
                }
                if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    end = Some(at + 1);
                    break;
                }
            }
            let end = end?;
            value = serde_json::from_str::<String>(&rest[..end]).ok()?;
            rest = &rest[end..];
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let scalar = &rest[..end];
            if scalar.is_empty() || !scalar.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            value = scalar.to_owned();
            rest = &rest[end..];
        }
        if !rest.is_empty() && !rest.starts_with(char::is_whitespace) {
            return None;
        }
        if fields.insert(key.to_owned(), value).is_some() {
            return None;
        }
    }
    Some(fields)
}

/// Inline formatting for an expanded, visible status card. Skip parsing large
/// bodies; they still remain fully readable in the existing bounded scroller.
/// No HTML executes, no link opens until the user clicks it.
pub fn card_detail_text(title: &str, text: &str) -> (String, Vec<(Range<usize>, InlineStyle)>) {
    // Default to literal provider data. Tool names are open-ended (for example
    // "Read index.html"), so a blacklist of words such as "output" cannot
    // safely distinguish source, diffs, HTML and JSON from narrative prose.
    let prose = matches!(
        title,
        "Thinking"
            | "Plan"
            | "Agent plan"
            | "Agent task list"
            | "Proposed plan"
            | "Reviewing changes"
            | "Code review"
            | "Email draft"
            | "Message draft"
            | "Social post"
            | "Document"
            | "Draft"
    ) || ["Plan · ", "Review · ", "Email draft · "]
        .iter()
        .any(|prefix| title.starts_with(prefix));
    if prose {
        detail_text(text)
    } else {
        (text.to_owned(), Vec::new())
    }
}

pub fn detail_text(text: &str) -> (String, Vec<(Range<usize>, InlineStyle)>) {
    if text.len() > 16 * 1024 {
        return (text.to_owned(), Vec::new());
    }
    let mut plain = String::new();
    let mut highlights = Vec::new();
    for block in markdown_rows(text) {
        let (body, spans) = match block {
            MessageBlock::RichParagraph { text, highlights } => (text, highlights),
            MessageBlock::Paragraph(text) | MessageBlock::Quote(text) => (text, Vec::new()),
            MessageBlock::Heading(text) => {
                let len = text.len();
                (text, vec![(0..len, InlineStyle::Bold)])
            }
            MessageBlock::Code { text, .. } => {
                let len = text.len();
                (text, vec![(0..len, InlineStyle::Code)])
            }
            MessageBlock::ListItem {
                marker,
                text,
                highlights,
            } => {
                let shift = marker.len() + 1;
                (
                    format!("{marker} {text}"),
                    highlights
                        .into_iter()
                        .map(|(range, style)| (range.start + shift..range.end + shift, style))
                        .collect(),
                )
            }
            MessageBlock::TableRow { cells, .. } => (cells.join(" │ "), Vec::new()),
            MessageBlock::Attachment { name, target } => {
                let len = name.len();
                (name, vec![(0..len, InlineStyle::Link(target.into()))])
            }
            _ => continue,
        };
        if !plain.is_empty() {
            plain.push('\n');
        }
        let shift = plain.len();
        highlights.extend(
            spans
                .into_iter()
                .map(|(range, style)| (range.start + shift..range.end + shift, style)),
        );
        plain.push_str(&body);
    }
    (plain, highlights)
}

fn markdown_rows(text: &str) -> Vec<MessageBlock> {
    let mut result = vec![];
    let mut state = MarkdownState {
        buffer: String::new(),
        heading: false,
        quotes: 0,
        bullets: 0,
        code: None,
        spans: vec![],
        active: vec![],
        lists: vec![],
        markers: vec![],
        table: false,
        table_header: false,
        cells: vec![],
    };
    let mut image: Option<(String, String)> = None;
    for event in Parser::new_ext(
        text,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS | Options::ENABLE_TABLES,
    ) {
        match event {
            Event::Start(Tag::Image { dest_url, .. }) => {
                state.emit(&mut result);
                image = Some((String::new(), dest_url.into_string()));
            }
            Event::Text(value) | Event::Code(value) if image.is_some() => {
                image.as_mut().unwrap().0.push_str(&value);
            }
            Event::End(TagEnd::Image) => {
                if let Some((name, target)) = image.take() {
                    result.push(MessageBlock::Attachment {
                        name: if name.trim().is_empty() {
                            "Image".into()
                        } else {
                            name
                        },
                        target,
                    });
                }
            }
            Event::Start(Tag::Strong) => state.active.push((InlineStyle::Bold, state.buffer.len())),
            Event::Start(Tag::Emphasis) => {
                state.active.push((InlineStyle::Italic, state.buffer.len()))
            }
            Event::Start(Tag::Strikethrough) => {
                state.active.push((InlineStyle::Strike, state.buffer.len()))
            }
            Event::Start(Tag::Link { dest_url, .. }) => state.active.push((
                InlineStyle::Link(dest_url.into_string().into()),
                state.buffer.len(),
            )),
            Event::End(
                TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link,
            ) => {
                if let Some((style, start)) = state.active.pop()
                    && start < state.buffer.len()
                {
                    state.spans.push((start..state.buffer.len(), style));
                }
            }
            Event::Start(Tag::Heading { .. }) => state.heading = true,
            Event::Start(Tag::BlockQuote(_)) => state.quotes += 1,
            Event::Start(Tag::List(start)) => state.lists.push(start),
            Event::End(TagEnd::List(_)) => {
                state.lists.pop();
            }
            Event::Start(Tag::Item) => {
                let marker = if let Some(Some(number)) = state.lists.last_mut() {
                    let marker = format!("{number}.");
                    *number += 1;
                    marker
                } else {
                    "•".into()
                };
                state.markers.push(marker);
                state.bullets += 1;
            }
            Event::Start(Tag::Table(_)) => {
                state.emit(&mut result);
                state.table = true;
            }
            Event::Start(Tag::TableHead) => {
                state.table_header = true;
                state.cells.clear();
            }
            Event::Start(Tag::TableRow) => state.cells.clear(),
            Event::Start(Tag::TableCell) => {
                state.buffer.clear();
                state.spans.clear();
            }
            Event::End(TagEnd::TableCell) => {
                state.cells.push(state.buffer.trim().to_owned());
                state.buffer.clear();
                state.spans.clear();
            }
            Event::End(TagEnd::TableHead | TagEnd::TableRow) => {
                emit_table(&state.cells, state.table_header, &mut result);
                state.cells.clear();
                state.table_header = false;
            }
            Event::End(TagEnd::Table) => state.table = false,
            Event::Start(Tag::CodeBlock(kind)) => {
                state.emit(&mut result);
                state.code = Some(match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                });
            }
            Event::Text(value) => state.buffer.push_str(&value),
            Event::Code(value) => {
                let start = state.buffer.len();
                state.buffer.push_str(&value);
                state
                    .spans
                    .push((start..state.buffer.len(), InlineStyle::Code));
                if !state
                    .active
                    .iter()
                    .any(|(style, _)| matches!(style, InlineStyle::Link(_)))
                    && inline_file_candidate(&value)
                {
                    state.spans.push((
                        start..state.buffer.len(),
                        // Inline code names literal filesystem bytes, unlike
                        // explicit Markdown URLs. Escape '%' before the central
                        // citation handler performs its single URL decode.
                        InlineStyle::Link(value.replace('%', "%25").into()),
                    ));
                }
            }
            Event::Html(value) | Event::InlineHtml(value) => {
                state.buffer.push_str(&html_text(&value))
            }
            Event::SoftBreak | Event::HardBreak => state.buffer.push('\n'),
            Event::TaskListMarker(done) => {
                state.buffer.push_str(if done { "☑ " } else { "☐ " })
            }
            Event::Rule => {
                state.emit(&mut result);
                result.push(MessageBlock::Paragraph("—".into()));
            }
            Event::End(TagEnd::Paragraph) => state.emit(&mut result),
            Event::End(TagEnd::Heading(_)) => {
                state.emit(&mut result);
                state.heading = false;
            }
            Event::End(TagEnd::CodeBlock) => {
                state.emit(&mut result);
                state.code = None;
            }
            Event::End(TagEnd::Item) => {
                state.emit(&mut result);
                state.bullets = state.bullets.saturating_sub(1);
                state.markers.pop();
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                state.emit(&mut result);
                state.quotes = state.quotes.saturating_sub(1);
            }
            _ => {}
        }
    }
    state.emit(&mut result);
    result
}

/// File-shaped inline code is an explicit user-click target, not a filesystem
/// lookup. Keep commands, URLs, model versions, refs and arbitrary identifiers
/// literal. Paths with whitespace remain available through explicit Markdown.
fn inline_file_candidate(text: &str) -> bool {
    if text.is_empty()
        || text.len() > 4096
        || !text
            .chars()
            .all(|ch| ch.is_alphanumeric() || "/._-~:#%".contains(ch))
        || text.contains("://")
        || text.contains("::")
        || text.starts_with('-')
        || text.starts_with("~/")
    {
        return false;
    }
    let mut path = text.split_once('#').map_or(text, |(path, _)| path);
    let mut location = false;
    for _ in 0..2 {
        if let Some((before, line)) = path.rsplit_once(':') {
            if line.is_empty()
                || !line.bytes().all(|byte| byte.is_ascii_digit())
                || !line.parse::<u64>().is_ok_and(|line| line > 0)
            {
                return false;
            }
            path = before;
            location = true;
        }
    }
    if path.contains(':') || !(path.contains('/') || location) {
        return false;
    }
    let explicit = path.starts_with("./") || path.starts_with("../");
    let absolute = path.starts_with('/');
    let name = path.rsplit('/').next().unwrap_or_default();
    if name.is_empty() {
        return false;
    }
    let extension = name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && !extension.is_empty()
            && extension.chars().all(char::is_alphanumeric)
            && !extension.bytes().all(|byte| byte.is_ascii_digit())
    });
    let special = matches!(
        name,
        "Makefile" | "Dockerfile" | "Cargo.lock" | "LICENSE" | "README" | "AGENTS.md"
    );
    let native_root = [
        "/Users/",
        "/tmp/",
        "/private/",
        "/var/",
        "/home/",
        "/workspace/",
        "/opt/",
        "/etc/",
    ]
    .iter()
    .any(|root| path.starts_with(root));
    // A slash alone also matches slash commands and git refs. A bare model
    // version or hostname must never be presented as a workspace file.
    let first = path.split('/').next().unwrap_or_default();
    let hostname = [".com", ".org", ".net", ".io", ".ai", ".dev"]
        .iter()
        .any(|suffix| first.ends_with(suffix));
    (extension || special || explicit || native_root)
        && (!absolute || extension || native_root)
        && (explicit || absolute || !hostname)
}

fn bounded_interaction_with_context(
    mut card: ChatInteraction,
    context: Option<&str>,
) -> Vec<MessageBlock> {
    if let (ChatInteraction::Speaker { text, .. }, Some(context)) = (&mut card, context) {
        *text = if context.starts_with("~~~") {
            format!("{context}\n{text}")
        } else {
            format!("```{context}\n{text}")
        };
    }
    bounded_interaction(card)
}

fn bounded_interaction(card: ChatInteraction) -> Vec<MessageBlock> {
    match card {
        ChatInteraction::Speaker { speaker, text } => {
            let mut blocks = markdown_blocks(&text);
            if matches!(
                blocks.first(),
                Some(MessageBlock::Code { .. } | MessageBlock::Attachment { .. })
            ) {
                blocks.insert(
                    0,
                    MessageBlock::Voice {
                        speaker,
                        text: String::new(),
                        highlights: vec![],
                    },
                );
            }
            blocks
                .into_iter()
                .map(|block| {
                    let (text, highlights) = match block {
                        MessageBlock::Paragraph(text) | MessageBlock::Quote(text) => (text, vec![]),
                        MessageBlock::RichParagraph { text, highlights } => (text, highlights),
                        MessageBlock::Heading(text) => {
                            let end = text.len();
                            (text, vec![(0..end, InlineStyle::Bold)])
                        }
                        MessageBlock::ListItem {
                            marker,
                            text,
                            highlights,
                        } => {
                            let shift = marker.len() + 1;
                            (
                                format!("{marker} {text}"),
                                highlights
                                    .into_iter()
                                    .map(|(range, style)| {
                                        (range.start + shift..range.end + shift, style)
                                    })
                                    .collect(),
                            )
                        }
                        MessageBlock::TableRow { cells, .. } => (cells.join("   │   "), vec![]),
                        other => return other,
                    };
                    MessageBlock::Voice {
                        speaker,
                        text,
                        highlights,
                    }
                })
                .collect()
        }
        card => vec![MessageBlock::Interaction(card)],
    }
}

/// Recognize a small set of native wrappers once on the parsing worker. Code
/// examples and ordinary JSON never enter this path. Unknown XML is preserved.
fn interaction_blocks(text: &str) -> Option<Vec<MessageBlock>> {
    interaction_blocks_depth(text, 0)
}

fn code_protected_ranges(text: &str) -> Vec<Range<usize>> {
    let mut protected = Vec::<Range<usize>>::new();
    let mut fence: Option<(usize, u8, usize)> = None;
    let mut offset = 0;
    // HTML-block parsing can obscure Markdown fences inside native <input>.
    for line in text.split_inclusive('\n') {
        let body = line.trim_start();
        let marker = body.as_bytes().first().copied().unwrap_or_default();
        let count = body.bytes().take_while(|byte| *byte == marker).count();
        if matches!(marker, b'`' | b'~') && count >= 3 {
            if let Some((start, character, length)) = fence {
                if marker == character && count >= length && body[count..].trim().is_empty() {
                    protected.push(start..offset + line.len());
                    fence = None;
                }
            } else {
                fence = Some((offset, marker, count));
            }
        }
        offset += line.len();
    }
    if let Some((start, _, _)) = fence {
        protected.push(start..text.len());
    }
    // App-owned JSON payloads are opaque to Markdown. Tool output can contain
    // unmatched template/backticks; pairing them with ticks in the next record
    // would protect its delimiters and leak otherwise valid cards as raw JSON.
    // Skip only payload bytes: backticks surrounding a literal envelope example
    // are still indexed normally, as are all existing Markdown fences.
    let mut card_bodies = Vec::new();
    let mut card_search = 0;
    while let Some(relative) = text[card_search..].find("<orbit_interaction") {
        let start = card_search + relative;
        card_search = start + 1;
        let Some(("orbit_interaction", body_start)) = opening_tag(text, start) else {
            continue;
        };
        if !text[start..body_start].contains("version=\"1\"") {
            continue;
        }
        let body_end = text[body_start..]
            .find("</orbit_interaction>")
            .map_or(text.len(), |end| body_start + end);
        card_bodies.push(body_start..body_end);
        card_search = (body_end + "</orbit_interaction>".len()).min(text.len());
    }
    // Native wrappers form HTML blocks, in which Markdown does not report
    // inline code. Index matching backtick runs once rather than repeatedly
    // searching the remaining text for each unmatched opener.
    let mut runs = Vec::new();
    let mut search = 0;
    let mut fence_ix = 0;
    let mut card_ix = 0;
    while let Some(relative) = text[search..].find('`') {
        let at = search + relative;
        let count = text[at..].bytes().take_while(|byte| *byte == b'`').count();
        search = at + count;
        while fence_ix < protected.len() && protected[fence_ix].end <= at {
            fence_ix += 1;
        }
        while card_ix < card_bodies.len() && card_bodies[card_ix].end <= at {
            card_ix += 1;
        }
        if card_bodies
            .get(card_ix)
            .is_some_and(|body| body.contains(&at))
        {
            continue;
        }
        if !protected
            .get(fence_ix)
            .is_some_and(|range| range.contains(&at))
        {
            runs.push((at, search, fence_ix));
        }
    }
    let mut next = vec![None; runs.len()];
    let mut matching = std::collections::HashMap::new();
    for (ix, &(start, end, group)) in runs.iter().enumerate().rev() {
        next[ix] = matching.insert((group, end - start), ix);
    }
    let mut ix = 0;
    while ix < runs.len() {
        if let Some(close) = next[ix] {
            protected.push(runs[ix].0..runs[close].1);
            ix = close + 1;
        } else {
            ix += 1;
        }
    }
    protected.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = vec![];
    for range in protected {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn interaction_blocks_depth(text: &str, depth: usize) -> Option<Vec<MessageBlock>> {
    const TAGS: &[&str] = &[
        "realtime_delegation",
        "send_user_message_question_reply",
        "orbit_interaction",
    ];
    if depth >= 16
        || !TAGS.iter().any(|tag| text.contains(&format!("<{tag}")))
        || matches!(text.trim_start().as_bytes().first(), Some(b'{' | b'['))
    {
        return None;
    }
    let protected = code_protected_ranges(text);
    let mut result = vec![];
    let mut cursor = 0;
    let mut search = 0;
    let mut protected_ix = 0;
    while let Some(relative) = text[search..].find('<') {
        let start = search + relative;
        search = start + 1;
        while protected_ix < protected.len() && protected[protected_ix].end <= start {
            protected_ix += 1;
        }
        if protected
            .get(protected_ix)
            .is_some_and(|range| range.contains(&start))
        {
            continue;
        }
        let Some((name, body_start)) = opening_tag(text, start) else {
            continue;
        };
        if !TAGS.contains(&name) {
            continue;
        }
        let (body_end, end) = closing_tag(text, body_start, name, &protected);
        let body = &text[body_start..body_end];
        let cards = match name {
            "orbit_interaction" => {
                if !text[start..body_start].contains("version=\"1\"") {
                    continue;
                }
                let Some((card, context)) = chat_interactions::decode_with_context(body) else {
                    continue;
                };
                bounded_interaction_with_context(card, context.as_deref())
            }
            "send_user_message_question_reply" => {
                let answers = chat_interactions::question_answers(body);
                if answers.is_empty() {
                    continue;
                }
                answers.into_iter().flat_map(bounded_interaction).collect()
            }
            "realtime_delegation" => delegation_blocks(body, depth + 1),
            _ => unreachable!(),
        };
        result.extend(markdown_blocks(&text[cursor..start]));
        result.extend(cards);
        cursor = end;
        search = end;
    }
    if cursor == 0 {
        return None;
    }
    result.extend(markdown_blocks(&text[cursor..]));
    Some(result)
}

fn opening_tag(text: &str, start: usize) -> Option<(&str, usize)> {
    let end = text[start..].find('>')? + start;
    if end - start > 256 {
        return None;
    }
    let raw = &text[start + 1..end];
    let name = raw.split_whitespace().next()?;
    if name.starts_with('/') || name.ends_with('/') {
        return None;
    }
    Some((name, end + 1))
}

fn closing_tag(text: &str, start: usize, name: &str, protected: &[Range<usize>]) -> (usize, usize) {
    let mut depth = 1u32;
    let mut search = start;
    let mut protected_ix = protected.partition_point(|range| range.end <= start);
    while let Some(relative) = text[search..].find('<') {
        let at = search + relative;
        search = at + 1;
        while protected_ix < protected.len() && protected[protected_ix].end <= at {
            protected_ix += 1;
        }
        if protected
            .get(protected_ix)
            .is_some_and(|range| range.contains(&at))
        {
            continue;
        }
        if let Some(close) = text[at..]
            .strip_prefix("</")
            .and_then(|s| s.strip_prefix(name))
            .and_then(|s| s.strip_prefix('>'))
        {
            depth -= 1;
            if depth == 0 {
                return (at, text.len() - close.len());
            }
        } else if let Some((nested, _)) = opening_tag(text, at)
            && nested == name
        {
            depth = depth.saturating_add(1);
        }
    }
    (text.len(), text.len())
}

fn field_wrapper<'a>(
    text: &'a str,
    name: &str,
    protected: &[Range<usize>],
) -> Option<(&'a str, Range<usize>)> {
    let mut search = 0;
    let mut nested = 0usize;
    while let Some(relative) = text[search..].find('<') {
        let start = search + relative;
        search = start + 1;
        if protected.iter().any(|range| range.contains(&start)) {
            continue;
        }
        if text[start..].starts_with("</realtime_delegation>") {
            nested = nested.saturating_sub(1);
        }
        let Some((tag, body_start)) = opening_tag(text, start) else {
            continue;
        };
        if tag == "realtime_delegation" {
            nested = nested.saturating_add(1);
        } else if nested == 0 && tag == name {
            let (body_end, end) = closing_tag(text, body_start, name, protected);
            return Some((&text[body_start..body_end], start..end));
        }
    }
    None
}

fn delegation_blocks(body: &str, depth: usize) -> Vec<MessageBlock> {
    let protected = code_protected_ranges(body);
    let input = field_wrapper(body, "input", &protected);
    let transcript = field_wrapper(body, "transcript_delta", &protected);
    let source = field_wrapper(body, "source", &protected);
    let mut result = vec![];
    if let Some((text, _)) = &input {
        let text = sanitize_transcript_text("user", text).unwrap_or_default();
        result.extend(bounded_interaction(ChatInteraction::Delegation {
            title: "Voice task".into(),
            details: text,
            status: InteractionStatus::Started,
            agent: None,
        }));
    }
    if let Some((text, _)) = &transcript {
        let mut speaker = None;
        let mut spoken = String::new();
        for line in text.lines() {
            let trimmed = line.trim_start();
            let entry = trimmed
                .strip_prefix("user:")
                .map(|text| (Speaker::User, text))
                .or_else(|| {
                    trimmed
                        .strip_prefix("assistant:")
                        .map(|text| (Speaker::Assistant, text))
                });
            if let Some((next, text)) = entry {
                if let Some(speaker) = speaker.take() {
                    result.extend(bounded_interaction(ChatInteraction::Speaker {
                        speaker,
                        text: std::mem::take(&mut spoken).trim().into(),
                    }));
                }
                speaker = Some(next);
                spoken.push_str(text.trim_start());
            } else {
                if !spoken.is_empty() {
                    spoken.push('\n');
                }
                spoken.push_str(line);
            }
        }
        if let Some(speaker) = speaker {
            result.extend(bounded_interaction(ChatInteraction::Speaker {
                speaker,
                text: spoken.trim().into(),
            }));
        } else {
            result.extend(markdown_blocks(&spoken));
        }
    }
    let mut removed: Vec<_> = [input, transcript, source]
        .into_iter()
        .flatten()
        .map(|(_, range)| range)
        .collect();
    removed.sort_by_key(|range| range.start);
    let mut start = 0;
    for range in removed {
        if range.start >= start {
            result.extend(
                interaction_blocks_depth(&body[start..range.start], depth)
                    .unwrap_or_else(|| markdown_blocks(&body[start..range.start])),
            );
        }
        start = start.max(range.end);
    }
    result.extend(
        interaction_blocks_depth(&body[start..], depth)
            .unwrap_or_else(|| markdown_blocks(&body[start..])),
    );
    result
}

fn emit_table(cells: &[String], header: bool, result: &mut Vec<MessageBlock>) {
    // Wide/large tables remain virtualizable: split each logical row into
    // bounded panels, keeping every cell byte rather than truncating values.
    for columns in cells.chunks(12) {
        let budget = 1500 / columns.len().max(1);
        let split: Vec<Vec<String>> = columns
            .iter()
            .map(|cell| {
                let mut parts = Vec::new();
                let mut start = 0;
                for (at, ch) in cell.char_indices() {
                    if at + ch.len_utf8() - start > budget {
                        parts.push(cell[start..at].to_owned());
                        start = at;
                    }
                }
                parts.push(cell[start..].to_owned());
                parts
            })
            .collect();
        for row in 0..split.iter().map(Vec::len).max().unwrap_or(0) {
            result.push(MessageBlock::TableRow {
                cells: split
                    .iter()
                    .map(|parts| parts.get(row).cloned().unwrap_or_default())
                    .collect(),
                header,
            });
        }
    }
}

fn html_text(html: &str) -> String {
    let lower = html.trim_start().to_ascii_lowercase();
    if lower.starts_with("<script") || lower.starts_with("<style") || lower.starts_with("<!--") {
        return String::new();
    }
    let mut text = String::new();
    let mut in_tag = false;
    let mut quote = None;
    let mut tag = String::new();
    for ch in html.chars() {
        if in_tag {
            tag.push(ch);
            if let Some(active) = quote {
                if ch == active {
                    quote = None;
                }
            } else if matches!(ch, '\'' | '"') {
                quote = Some(ch);
            } else if ch == '>' {
                in_tag = false;
                let name = tag
                    .trim_start_matches(['<', '/'])
                    .split([' ', '\t', '\n', '/', '>'])
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if !matches!(
                    name.as_str(),
                    "p" | "div"
                        | "span"
                        | "em"
                        | "strong"
                        | "b"
                        | "i"
                        | "u"
                        | "s"
                        | "br"
                        | "hr"
                        | "a"
                        | "img"
                        | "ul"
                        | "ol"
                        | "li"
                        | "blockquote"
                        | "pre"
                        | "code"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "table"
                        | "thead"
                        | "tbody"
                        | "tr"
                        | "th"
                        | "td"
                        | "details"
                        | "summary"
                ) {
                    text.push_str(&tag);
                }
                tag.clear();
            }
        } else if ch == '<' {
            in_tag = true;
            tag.push(ch);
        } else {
            text.push(ch);
        }
    }
    text.push_str(&tag);
    text
}

#[derive(Clone, Copy, Default)]
enum LexState {
    #[default]
    Normal,
    String {
        quote: u8,
        triple: bool,
    },
    BlockComment,
    LineComment,
}
fn highlight(text: &str, language: &str, state: &mut LexState) -> Vec<(Range<usize>, u32)> {
    let python = matches!(language, "python" | "py");
    if !python
        && !matches!(
            language,
            "rust"
                | "rs"
                | "javascript"
                | "js"
                | "jsx"
                | "typescript"
                | "ts"
                | "tsx"
                | "json"
                | "jsonc"
        )
    {
        return vec![];
    }
    const KEYWORDS: &[&str] = &[
        "fn",
        "let",
        "mut",
        "pub",
        "use",
        "mod",
        "impl",
        "struct",
        "enum",
        "trait",
        "where",
        "self",
        "Self",
        "super",
        "crate",
        "move",
        "ref",
        "match",
        "loop",
        "in",
        "as",
        "const",
        "static",
        "return",
        "break",
        "continue",
        "if",
        "else",
        "for",
        "while",
        "async",
        "await",
        "unsafe",
        "dyn",
        "type",
        "function",
        "var",
        "class",
        "new",
        "this",
        "export",
        "import",
        "from",
        "default",
        "extends",
        "interface",
        "switch",
        "case",
        "throw",
        "try",
        "catch",
        "finally",
        "typeof",
        "void",
        "def",
        "lambda",
        "with",
        "yield",
        "pass",
        "raise",
        "except",
        "and",
        "or",
        "not",
        "is",
        "elif",
        "del",
        "global",
        "nonlocal",
        "assert",
    ];
    let mut spans = vec![];
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        if matches!(*state, LexState::Normal) {
            if bytes[index] == b'"'
                || (bytes[index] == b'\'' && !matches!(language, "json" | "jsonc"))
                || bytes[index] == b'`'
            {
                let quote = bytes[index];
                let triple = python
                    && bytes
                        .get(index..index + 3)
                        .is_some_and(|v| v == [quote, quote, quote]);
                index += if triple { 3 } else { 1 };
                *state = LexState::String { quote, triple };
            } else if (python && bytes[index] == b'#')
                || (!python && bytes.get(index..index + 2) == Some(b"//"))
            {
                *state = LexState::LineComment;
            } else if !python && bytes.get(index..index + 2) == Some(b"/*") {
                index += 2;
                *state = LexState::BlockComment;
            }
        }
        let color = match *state {
            LexState::String { quote, triple } => {
                while index < bytes.len() {
                    if bytes[index] == b'\\' {
                        index += 1;
                        if index < bytes.len() {
                            index += text[index..].chars().next().unwrap().len_utf8();
                        }
                    } else if bytes[index] == quote
                        && (!triple
                            || bytes
                                .get(index..index + 3)
                                .is_some_and(|v| v == [quote, quote, quote]))
                    {
                        index += if triple { 3 } else { 1 };
                        *state = LexState::Normal;
                        break;
                    } else {
                        index += text[index..].chars().next().unwrap().len_utf8();
                    }
                }
                Some(0xb5cba5)
            }
            LexState::LineComment => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += text[index..].chars().next().unwrap().len_utf8();
                }
                if index < bytes.len() {
                    *state = LexState::Normal;
                }
                Some(0x747d8b)
            }
            LexState::BlockComment => {
                while index < bytes.len() {
                    if bytes.get(index..index + 2) == Some(b"*/") {
                        index += 2;
                        *state = LexState::Normal;
                        break;
                    }
                    index += text[index..].chars().next().unwrap().len_utf8();
                }
                Some(0x747d8b)
            }
            LexState::Normal => {
                let ch = text[index..].chars().next().unwrap();
                if ch.is_ascii_digit() {
                    index += 1;
                    while index < bytes.len()
                        && (bytes[index].is_ascii_alphanumeric()
                            || matches!(bytes[index], b'.' | b'_'))
                    {
                        index += 1;
                    }
                    Some(0xe6b77e)
                } else if ch.is_alphabetic() || ch == '_' {
                    index += ch.len_utf8();
                    while index < bytes.len() {
                        let ch = text[index..].chars().next().unwrap();
                        if !ch.is_alphanumeric() && ch != '_' {
                            break;
                        }
                        index += ch.len_utf8();
                    }
                    let token = &text[start..index];
                    if KEYWORDS.contains(&token) {
                        Some(0xbdabf3)
                    } else if matches!(
                        token,
                        "true" | "false" | "null" | "None" | "True" | "False" | "undefined"
                    ) {
                        Some(0xe6b77e)
                    } else {
                        None
                    }
                } else {
                    index += ch.len_utf8();
                    None
                }
            }
        };
        if index == start {
            index += text[index..].chars().next().unwrap().len_utf8();
        }
        if let Some(color) = color
            && index > start
        {
            spans.push((start..index, color));
        }
    }
    spans
}

fn chunks(text: &str) -> Vec<&str> {
    let mut rows = vec![];
    let mut start = 0;
    let mut lines = 0;
    for (index, ch) in text.char_indices() {
        if index + ch.len_utf8() - start > 1500 || lines >= 20 {
            rows.push(&text[start..index]);
            start = index;
            lines = 0;
        }
        if ch == '\n' {
            lines += 1;
        }
    }
    if start < text.len() {
        rows.push(&text[start..]);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_transcripts_preserve_native_records_and_code_instead_of_json_fragments(){
        let details="terminal output \n".repeat(10000);
        let card=ChatInteraction::Delegation{title:"Terminal output".into(),details:details.clone(),status:InteractionStatus::Completed,agent:None};
        let native=chat_interactions::serialize_interaction(&card);
        assert_eq!(blocks(&native).iter().filter(|b|matches!(b,MessageBlock::Interaction(_))).count(),1,"One tool call must remain one expandable item");
        let raw=format!("## Starting\n\n{native}\n\n## Done\n\nThe final answer.");
        let compact=compact_transcript(&raw,8192);assert!(compact.len()<=8192);assert!(compact.ends_with("The final answer."));
        assert!(blocks(&compact).iter().any(|b|matches!(b,MessageBlock::Interaction(_))));assert!(!blocks(&compact).iter().any(|b|matches!(b,MessageBlock::Paragraph(s)if s.contains("orbit_interaction")||s.contains("\\n"))));
        let orphan=format!("escaped \\n \\t payload</orbit_interaction>\nThe real reply");assert_eq!(compact_transcript(&orphan,8192),"\nThe real reply");
        let code=format!("```rust\n{}\n```\nDone","let x = 1;\n".repeat(2000));let clipped=compact_transcript(&code,4096);assert!(matches!(blocks(&clipped).first(),Some(MessageBlock::Code{..})),"A clipped code block lost its fence");
        let literal=format!("```xml\n{native}\n```");assert_eq!(compact_transcript(&literal,literal.len()+1),literal);
    }
    #[test]
    fn inline_sweep_preserves_overlap_order_and_scales_to_many_links() {
        let spans = vec![
            (1..9, InlineStyle::Bold),
            (3..6, InlineStyle::Code),
            (3..8, InlineStyle::Link("src/main.rs:2".into())),
            (20..30, InlineStyle::Italic),
            (2..2, InlineStyle::Strike),
        ];
        let actual = inline_runs(10, &spans);
        // Compare every emitted byte to the same precedence as the source
        // highlight order, including same-position and clamped boundaries.
        for byte in 0..10 {
            let expected = spans
                .iter()
                .filter(|(span, _)| span.contains(&byte))
                .map(|(_, style)| style.clone())
                .collect::<Vec<_>>();
            let found = actual
                .iter()
                .find(|(span, _)| span.contains(&byte))
                .map(|(_, styles)| styles.clone())
                .unwrap_or_default();
            assert_eq!(found, expected);
        }
        let many = (0..10_000)
            .map(|index| {
                (
                    index * 2..index * 2 + 1,
                    InlineStyle::Link("src/main.rs".into()),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(inline_runs(20_000, &many).len(), 10_000);
    }

    #[test]
    fn review_directives_become_glass_cards_with_precise_file_links() {
        let text = "Before\n\n::code-comment{title=\"[P1] Missing check\" body=\"Handle **empty** input before indexing.\" file=\"/tmp/café #1.rs\" start=7 end=9 priority=1}\n\nAfter";
        let blocks = blocks(text);
        assert!(blocks.iter().any(|block| matches!(block,
            MessageBlock::Interaction(ChatInteraction::Status {title,details,..})
            if title == "Review · [P1] Missing check" && details.contains("**empty**"))));
        assert!(blocks.iter().any(|block| matches!(block,
            MessageBlock::RichParagraph { highlights, .. }
            if highlights.iter().any(|(_,style)| matches!(style,InlineStyle::Link(target)
                if target.as_ref() == "/tmp/café %231.rs#L7-L9")))));
        assert!(blocks.contains(&MessageBlock::Paragraph("Before".into())));
        assert!(blocks.contains(&MessageBlock::Paragraph("After".into())));
    }

    #[test]
    fn writing_and_proposed_plans_have_semantic_bounded_envelopes() {
        let source = ":::writing{variant=\"email\" id=\"12345\" subject=\"Release\"}\nHello **team**.\n\n```text\n::: \n```\nThanks.\n:::\n\n<proposed_plan>\n# Plan\n- Check first\n</proposed_plan>";
        let cards = blocks(source);
        assert!(cards.iter().any(|block| matches!(block,
            MessageBlock::Interaction(ChatInteraction::Status { title, details, .. })
                if title == "Email draft · Release" && details.contains("Thanks.") && details.contains(":::"))));
        assert!(cards.iter().any(|block| matches!(block,
            MessageBlock::Interaction(ChatInteraction::Status { title, details, .. })
                if title == "Proposed plan" && details.contains("Check first"))));
    }

    #[test]
    fn directive_examples_unknowns_and_incomplete_content_remain_literal() {
        for source in [
            "`::code-comment{title=\"x\" body=\"y\"}`",
            "```text\n::code-comment{title=\"x\" body=\"y\"}\n```",
            "    ::code-comment{title=\"x\" body=\"y\"}",
            "> ::code-comment{title=\"x\" body=\"y\"}",
            "::code-comment{title=\"x\" title=\"duplicate\" body=\"y\"}",
            "::code-comment{title=\"x\" body=\"unfinished}",
            "::unknown-directive{title=\"x\"}",
            ":::writing{variant=\"email\"}\nAn unfinished draft",
            "<proposed_plan>\nAn unfinished plan",
        ] {
            assert!(
                !blocks(source)
                    .iter()
                    .any(|block| matches!(block, MessageBlock::Interaction(_))),
                "{source}"
            );
        }
        let huge = format!(
            "::code-comment{{title=\"x\" body=\"{}\"}}",
            "a".repeat(32 * 1024)
        );
        assert!(
            !blocks(&huge)
                .iter()
                .any(|block| matches!(block, MessageBlock::Interaction(_)))
        );
    }

    #[test]
    fn expanded_details_preserve_unicode_links_code_and_style_boundaries() {
        let (text, spans) = detail_text(
            "**日本語** [source](<src/a b.rs#L2>)\n\n- `code`\n\n```rust\nlet x = 1;\n```",
        );
        assert!(text.contains("日本語"));
        assert!(text.contains("let x = 1;"));
        assert!(spans.iter().any(
            |(_, style)| matches!(style, InlineStyle::Link(url) if url.as_ref() == "src/a b.rs#L2")
        ));
        for (range, _) in spans {
            assert!(text.get(range).is_some());
        }
        let huge = "*literal*".repeat(3000);
        assert_eq!(detail_text(&huge), (huge, vec![]));
    }

    #[test]
    fn provider_tool_details_default_to_literal_source_including_html_and_diffs() {
        let html = "<!doctype html>\n<html><body><h1>Title</h1><script>alert('literal source')</script></body></html>\n";
        let diff = "diff --git a/index.html b/index.html\n--- a/index.html\n+++ b/index.html\n@@ -1 +1 @@\n-<h1>Old</h1>\n+<h1>**New**</h1>\n";
        for title in [
            "Read index.html",
            "Read",
            "read_file",
            "Connected tool",
            "File changes",
            "Ran npm test",
            "Terminal output",
            "Custom provider tool",
        ] {
            for source in [
                html,
                diff,
                "# literal heading\n[not a link](javascript:alert)\n",
                "{\"source\":\"<b>literal</b>\"}",
            ] {
                assert_eq!(
                    card_detail_text(title, source),
                    (source.to_owned(), vec![]),
                    "{title}"
                );
            }
        }
        let (text, spans) = card_detail_text(
            "Review · [P1] Missing guard",
            "Keep **bold** and [file](src/main.rs:2).",
        );
        assert_eq!(text, "Keep bold and file.");
        assert!(
            spans
                .iter()
                .any(|(_, style)| matches!(style, InlineStyle::Link(_)))
        );
        let (text, _) = card_detail_text("Plan · 1/2 complete", "**Verify** before shipping");
        assert_eq!(text, "Verify before shipping");
    }

    #[test]
    fn inferred_file_links_preserve_literal_percent_and_do_not_expand_home() {
        let rows = blocks(
            "Open `src/100%real.rs:8`, `src/literal%20name.rs#L2`. Keep `~/private/secrets.rs` literal.",
        );
        let links = rows
            .iter()
            .filter_map(|row| {
                if let MessageBlock::RichParagraph { highlights, .. } = row {
                    Some(highlights)
                } else {
                    None
                }
            })
            .flatten()
            .filter_map(|(_, style)| {
                if let InlineStyle::Link(target) = style {
                    Some(target.as_ref())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(links, ["src/100%25real.rs:8", "src/literal%2520name.rs#L2"]);
        assert!(!inline_file_candidate("~/private/secrets.rs"));
        assert!(inline_file_candidate("./src/local.rs"));
    }

    #[test]
    fn inline_file_tags_open_locations_without_linking_commands_or_versions() {
        let source = "Open `src/main.rs:12:3`, `./README.md` and `/Users/me/日本語/file.rs#L2`. Keep `cargo test`, `origin/main`, `https://example.com/a.rs`, `gpt-6.1`, `/status`, `Vec::new`, `example.com/foo.rs` literal.";
        let rows = blocks(source);
        let links = rows
            .iter()
            .filter_map(|row| {
                if let MessageBlock::RichParagraph { highlights, .. } = row {
                    Some(highlights)
                } else {
                    None
                }
            })
            .flatten()
            .filter_map(|(_, style)| {
                if let InlineStyle::Link(target) = style {
                    Some(target.as_ref())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(
            links,
            [
                "src/main.rs:12:3",
                "./README.md",
                "/Users/me/日本語/file.rs#L2"
            ]
        );
        let rows = blocks("[`src/main.rs`](https://example.com)");
        let MessageBlock::RichParagraph { highlights, .. } = &rows[0] else {
            panic!("link")
        };
        assert_eq!(
            highlights
                .iter()
                .filter(|(_, style)| matches!(style, InlineStyle::Link(_)))
                .count(),
            1
        );
    }

    #[test]
    fn unicode_chunks_preserve_bytes_and_bound_size() {
        let text = "🦀é日本語👨‍👩‍👧‍👦".repeat(3000);
        let rows = chunks(&text);
        assert_eq!(rows.concat(), text);
        assert!(rows.iter().all(|r| !r.is_empty() && r.len() <= 1500));
    }
    #[test]
    fn huge_code_blocks_have_bounded_rows() {
        let source = "    println!(\"🦀\");\n".repeat(1000);
        let rows = blocks(&format!("```rust\n{source}```"));
        let code: Vec<_> = rows
            .iter()
            .map(|r| match r {
                MessageBlock::Code { language, text, .. } => {
                    assert_eq!(language, "rust");
                    text.as_str()
                }
                _ => panic!("expected code"),
            })
            .collect();
        assert_eq!(code.concat(), source);
        assert!(
            code.iter()
                .all(|r| r.lines().count() <= 20 && r.len() <= 1500)
        );
    }
    #[test]
    fn long_nested_styles_preserve_cross_row_ranges_and_precedence() {
        let text = "α🦀".repeat(1000);
        let mut state = MarkdownState {
            buffer: text.clone(),
            heading: false,
            quotes: 0,
            bullets: 0,
            code: None,
            spans: vec![
                (600..2400, InlineStyle::Link("https://example.com".into())),
                (0..text.len(), InlineStyle::Bold),
                (1200..4200, InlineStyle::Italic),
            ],
            active: vec![],
            lists: vec![],
            markers: vec![],
            table: false,
            table_header: false,
            cells: vec![],
        };
        let mut start = 0;
        let expected: Vec<_> = chunks(&text)
            .into_iter()
            .map(|row| {
                let end = start + row.len();
                let highlights = state
                    .spans
                    .iter()
                    .filter_map(|(span, style)| {
                        let a = span.start.max(start);
                        let b = span.end.min(end);
                        (a < b).then(|| ((a - start)..(b - start), style.clone()))
                    })
                    .collect();
                start = end;
                MessageBlock::RichParagraph {
                    text: row.into(),
                    highlights,
                }
            })
            .collect();
        let mut actual = vec![];
        state.emit(&mut actual);
        assert_eq!(actual, expected);
        let urls: Vec<_> = actual
            .iter()
            .flat_map(|block| match block {
                MessageBlock::RichParagraph { highlights, .. } => highlights
                    .iter()
                    .filter_map(|(_, style)| {
                        if let InlineStyle::Link(url) = style {
                            Some(url)
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>(),
                _ => vec![],
            })
            .collect();
        assert!(urls.len() > 1);
        assert!(urls.windows(2).all(|pair| Arc::ptr_eq(pair[0], pair[1])));
    }
    #[test]
    fn recognizes_structure_and_keeps_inline_text() {
        assert_eq!(
            blocks("# Hello\n\nA **bold** [link](https://example.com).\n\n> Quoted\n\n- Item"),
            vec![
                MessageBlock::Heading("Hello".into()),
                MessageBlock::RichParagraph {
                    text: "A bold link.".into(),
                    highlights: vec![
                        (2..6, InlineStyle::Bold),
                        (7..11, InlineStyle::Link("https://example.com".into()))
                    ]
                },
                MessageBlock::Quote("Quoted".into()),
                MessageBlock::ListItem {
                    marker: "•".into(),
                    text: "Item".into(),
                    highlights: vec![]
                }
            ]
        );
    }
    #[test]
    fn code_tokens_are_colored_with_valid_unicode_ranges() {
        let text = "fn main() { let count = 42; println!(\"🦀日本語\"); // comment\n}";
        let spans = highlight(text, "rust", &mut LexState::Normal);
        let tokens: Vec<_> = spans.iter().map(|(r, _)| &text[r.clone()]).collect();
        assert!(tokens.contains(&"fn"));
        assert!(tokens.contains(&"42"));
        assert!(tokens.contains(&"\"🦀日本語\""));
        assert!(tokens.contains(&"// comment"));
        assert!(spans.windows(2).all(|p| p[0].0.end <= p[1].0.start));
    }
    #[test]
    fn comments_and_strings_keep_state_across_rows() {
        let mut state = LexState::Normal;
        let first = highlight("/* a long", "typescript", &mut state);
        let second = highlight(" comment */ const x = 2;", "typescript", &mut state);
        assert_eq!(first[0].1, second[0].1);
        assert_eq!(second[0].0, 0..11);
        let mut state = LexState::Normal;
        highlight("\"long 🦀", "json", &mut state);
        assert_eq!(highlight("string\": 42", "json", &mut state)[0].0, 0..7);
    }
    #[test]
    fn interaction_easing_is_bounded_and_monotonic() {
        assert_eq!(ease_out(-1.), 0.);
        assert_eq!(ease_out(0.), 0.);
        assert_eq!(ease_out(0.5), 0.875);
        assert_eq!(ease_out(1.), 1.);
        assert_eq!(ease_out(2.), 1.);
        assert!((0..100).all(|i| ease_out(i as f32 / 100.) <= ease_out((i + 1) as f32 / 100.)));
    }
    #[test]
    fn known_approval_envelopes_are_removed_without_touching_user_json() {
        let request = "Planned action JSON:\n{\"command\":[\"/bin/zsh\",\"-lc\",\"cargo test\"],\"sandbox_permissions\":\"require_escalated\"}\nAPPROVAL REQUEST END";
        assert!(sanitize_transcript_text("user", request).is_none());
        let decision = r#"{"risk_level":"low","user_authorization":"medium","outcome":"allow","rationale":"Requested test"}"#;
        assert!(sanitize_transcript_text("assistant", decision).is_none());
        assert_eq!(
            sanitize_transcript_text("user", decision).as_deref(),
            Some(decision)
        );
        let ordinary = r#"{"outcome":"allow","command":"cargo test"}"#;
        assert_eq!(
            sanitize_transcript_text("user", ordinary).as_deref(),
            Some(ordinary)
        );
        let code = format!(
            "Here is the protocol example:\n```text\n{request}\n```\n```json\n{decision}\n```"
        );
        assert_eq!(
            sanitize_transcript_text("user", &code).as_deref(),
            Some(code.as_str())
        );
    }
    #[test]
    fn injected_context_is_stripped_but_the_real_message_is_retained() {
        let input = "<environment_context>\n<cwd>/private/project</cwd>\n</environment_context>\n\nFix the scrolling.\n\n<user_instructions>machine context</user_instructions>";
        assert_eq!(
            sanitize_transcript_text("user", input).as_deref(),
            Some("Fix the scrolling.")
        );
        assert!(sanitize_transcript_text("user","The following is the Codex agent history added since your last turn: machine events").is_none());
        let markdown = "Show `<environment_context>` as code.";
        assert_eq!(
            sanitize_transcript_text("user", markdown).as_deref(),
            Some(markdown)
        );
        let fenced = "```xml\n<environment_context>example</environment_context>\n```";
        assert_eq!(
            sanitize_transcript_text("user", fenced).as_deref(),
            Some(fenced)
        );
    }
    #[test]
    fn plugin_context_is_hidden_without_hiding_literal_examples_or_real_text() {
        let context = "<recommended_plugins>\nHere is a list of plugins that are available but not installed.\n\n- Dropbox (app-id@openai-curated-remote)\n- Figma (figma@openai-curated-remote)\n</recommended_plugins>";
        assert!(sanitize_transcript_text("user", context).is_none());
        assert_eq!(
            sanitize_transcript_text("user", &format!("Before {context} after.")).as_deref(),
            Some("Before  after.")
        );
        assert_eq!(sanitize_transcript_text("user", "<recommended_plugins source=\"native\">hidden<recommended_plugins>nested</recommended_plugins>hidden</recommended_plugins>Real request").as_deref(), Some("Real request"));
        assert_eq!(
            sanitize_transcript_text(
                "user",
                "<image_resize_notice>pixels</image_resize_notice>Image question"
            )
            .as_deref(),
            Some("Image question")
        );
        assert_eq!(sanitize_transcript_text("user", &format!("{context}\n{{\"example\":\"<recommended_plugins>literal</recommended_plugins>\"}}")).as_deref(), Some("{\"example\":\"<recommended_plugins>literal</recommended_plugins>\"}"));
        for text in [
            "Show `<recommended_plugins>` literally.",
            "Show ``<recommended_plugins>literal</recommended_plugins>``.",
            "```xml\n<recommended_plugins>example</recommended_plugins>\n```\nThen explain.",
            "<div>ordinary HTML</div><widget>unknown XML</widget>",
            "{\"example\":\"<recommended_plugins>literal</recommended_plugins>\"}",
        ] {
            assert_eq!(
                sanitize_transcript_text("user", text).as_deref(),
                Some(text)
            );
        }
    }
    #[test]
    fn assistant_reasoning_is_hidden_and_final_text_is_unwrapped() {
        assert_eq!(
            sanitize_transcript_text(
                "assistant",
                "<analysis>private draft</analysis>\n<final>**Here is the answer.**</final>"
            )
            .as_deref(),
            Some("**Here is the answer.**")
        );
        assert_eq!(
            sanitize_transcript_text("user", "<analysis>user example</analysis>").as_deref(),
            Some("<analysis>user example</analysis>")
        );
    }
    #[test]
    fn inline_code_emphasis_and_links_keep_unicode_safe_ranges() {
        let rows = blocks("é **bold** *italic* `🦀.rs` [open](https://example.com) ~~old~~");
        let MessageBlock::RichParagraph { text, highlights } = &rows[0] else {
            panic!("rich text")
        };
        assert_eq!(text, "é bold italic 🦀.rs open old");
        assert!(
            highlights
                .iter()
                .any(|(r, s)| &text[r.clone()] == "🦀.rs" && *s == InlineStyle::Code)
        );
        assert!(
            highlights
                .iter()
                .all(|(r, _)| text.is_char_boundary(r.start) && text.is_char_boundary(r.end))
        );
    }
    #[test]
    fn tool_envelopes_become_compact_semantic_activity() {
        let text =
            serde_json::json!({"orbit_tool":true,"label":"Ran a command","details":"cargo test"})
                .to_string();
        assert_eq!(
            message_blocks("tool", &text),
            vec![MessageBlock::Activity {
                label: "Ran a command".into(),
                details: "cargo test".into()
            }]
        );
    }
    #[test]
    fn ordinary_html_retains_readable_text_without_executing() {
        assert_eq!(
            blocks("<p>Hello <em>world</em></p>"),
            vec![MessageBlock::Paragraph("Hello world".into())]
        );
    }
    #[test]
    fn native_voice_delegation_and_speakers_render_without_internal_tags() {
        let text = "Before\n<realtime_delegation><source>internal routing</source><input>Inspect the patch</input><transcript_delta>user: Open [main.rs](/project/main.rs:12)\nassistant: **Ready**\ncontinuation</transcript_delta></realtime_delegation>\nAfter";
        let rows = message_blocks("user", text);
        assert!(rows.iter().any(|row| matches!(row,MessageBlock::Interaction(ChatInteraction::Delegation {details,..}) if details=="Inspect the patch")));
        assert!(rows.iter().any(|row| matches!(row,MessageBlock::Voice {speaker:Speaker::User,text,highlights} if text=="Open main.rs" && highlights.iter().any(|(_,style)| matches!(style,InlineStyle::Link(url) if url.as_ref()=="/project/main.rs:12")))));
        assert!(rows.iter().any(|row| matches!(row,MessageBlock::Voice {speaker:Speaker::Assistant,text,highlights} if text=="Ready\ncontinuation" && highlights[0].1==InlineStyle::Bold)));
        assert!(rows.contains(&MessageBlock::Paragraph("Before".into())));
        assert!(rows.contains(&MessageBlock::Paragraph("After".into())));
        assert!(!format!("{rows:?}").contains("internal routing"));
    }
    #[test]
    fn nested_and_unclosed_delegations_preserve_actual_content() {
        let nested = "<realtime_delegation><realtime_delegation><input>Nested task</input></realtime_delegation><input>Outer task</input></realtime_delegation>Tail";
        let rows = message_blocks("user", nested);
        for text in ["Nested task", "Outer task"] {
            assert_eq!(rows.iter().filter(|row| matches!(row,MessageBlock::Interaction(ChatInteraction::Delegation {details,..}) if details==text)).count(),1);
        }
        assert!(rows.contains(&MessageBlock::Paragraph("Tail".into())));
        let rows = message_blocks(
            "user",
            "<realtime_delegation><input>Unfinished but real request",
        );
        assert!(rows.iter().any(|row| matches!(row,MessageBlock::Interaction(ChatInteraction::Delegation {details,..}) if details=="Unfinished but real request")));
    }
    #[test]
    fn historical_question_reply_becomes_an_answer_card() {
        let text = "<send_user_message_question_reply>[{\"questionItemId\":\"opaque\",\"question\":\"Which appearance?\",\"answer\":\"Glass\"}]</send_user_message_question_reply> Real follow-up";
        assert_eq!(
            message_blocks("user", text),
            vec![
                MessageBlock::Interaction(ChatInteraction::Answer {
                    question: "Which appearance?".into(),
                    answer: "Glass".into()
                }),
                MessageBlock::Paragraph("Real follow-up".into())
            ]
        );
    }
    #[test]
    fn literal_native_tags_json_unknown_xml_and_fenced_examples_stay_literal() {
        let wrapper = "<realtime_delegation><input>literal</input></realtime_delegation>";
        for text in [
            format!("```xml\n{wrapper}\n```"),
            format!("Example `{wrapper}`."),
            serde_json::json!({"example":wrapper}).to_string(),
        ] {
            let rows = message_blocks("user", &text);
            assert!(!rows.iter().any(|row| matches!(
                row,
                MessageBlock::Interaction(_) | MessageBlock::Voice { .. }
            )));
            assert!(format!("{rows:?}").contains("realtime_delegation"));
        }
        let rows = message_blocks("user", "<Widget mode=\"custom\">Keep this XML</Widget>");
        assert!(format!("{rows:?}").contains("Widget"));
        assert!(format!("{rows:?}").contains("Keep this XML"));
    }
    #[test]
    fn native_context_wrappers_hide_only_context_and_preserve_adjacent_cards() {
        let text = "Before <realtime_conversation>instructions<tools>internal schemas</tools></realtime_conversation><realtime_delegation><input>Real task</input></realtime_delegation> After";
        let rows = message_blocks("user", text);
        assert!(!format!("{rows:?}").contains("instructions"));
        assert!(rows.iter().any(|row| matches!(row,MessageBlock::Interaction(ChatInteraction::Delegation {details,..}) if details=="Real task")));
        assert_eq!(
            sanitize_transcript_text(
                "user",
                "Before <tools>nested<tools>hidden</tools>still hidden"
            )
            .as_deref(),
            Some("Before")
        );
    }
    #[test]
    fn image_markdown_becomes_a_named_attachment_without_decoding_media() {
        assert_eq!(
            blocks("See this:\n\n![diagram.png](file:///project/diagram.png)\n\nThen continue."),
            vec![
                MessageBlock::Paragraph("See this:".into()),
                MessageBlock::Attachment {
                    name: "diagram.png".into(),
                    target: "file:///project/diagram.png".into()
                },
                MessageBlock::Paragraph("Then continue.".into())
            ]
        );
        assert!(
            matches!(&blocks("![site](https://example.com/image.svg)")[0],MessageBlock::Attachment {target,..} if target=="https://example.com/image.svg")
        );
    }
    #[test]
    fn private_cards_survive_interleaved_final_text_and_speaker_links() {
        let card = ChatInteraction::Status {
            title: "Completed".into(),
            details: "2.4 s".into(),
            status: InteractionStatus::Completed,
        };
        let text = format!(
            "Actual reply\n\n{}\n\nNext note",
            chat_interactions::serialize_interaction(&card)
        );
        assert_eq!(
            blocks(&text),
            vec![
                MessageBlock::Paragraph("Actual reply".into()),
                MessageBlock::Interaction(card),
                MessageBlock::Paragraph("Next note".into())
            ]
        );
        let voice = ChatInteraction::Speaker {
            speaker: Speaker::Assistant,
            text: "Read [the file](/project/a.rs#L8).".into(),
        };
        assert!(
            matches!(&message_blocks("tool",&chat_interactions::encode(&voice))[0],MessageBlock::Voice {text,highlights,..} if text=="Read the file." && highlights.iter().any(|(_,style)| matches!(style,InlineStyle::Link(_))))
        );
        assert!(
            !message_blocks("user", &chat_interactions::encode(&voice))
                .iter()
                .any(|row| matches!(row, MessageBlock::Voice { .. }))
        );
    }

    #[test]
    fn nested_markdown_styles_form_disjoint_gpui_runs() {
        let rows = blocks("**bold [link](https://example.com)**");
        let MessageBlock::RichParagraph { text, highlights } = &rows[0] else {
            panic!("rich text")
        };
        let runs = inline_runs(text.len(), highlights);
        assert!(runs.windows(2).all(|pair| pair[0].0.end <= pair[1].0.start));
        let (_, styles) = runs
            .iter()
            .find(|(r, _)| &text[r.clone()] == "link")
            .unwrap();
        assert!(styles.contains(&InlineStyle::Bold));
        assert!(styles.contains(&InlineStyle::Link("https://example.com".into())));
    }

    #[test]
    fn numbered_lists_preserve_start_number_and_inline_formatting() {
        let rows = blocks("3. **First** step\n4. `Second` step\n\n- Bullet");
        assert!(
            matches!(&rows[0], MessageBlock::ListItem {marker,text,highlights} if marker=="3." && text=="First step" && highlights[0].1==InlineStyle::Bold)
        );
        assert!(
            matches!(&rows[1], MessageBlock::ListItem {marker,text,highlights} if marker=="4." && text=="Second step" && highlights[0].1==InlineStyle::Code)
        );
        assert!(
            matches!(&rows[2], MessageBlock::ListItem {marker,text,..} if marker=="•" && text=="Bullet")
        );
    }

    #[test]
    fn markdown_tables_keep_header_rows_and_all_large_cell_bytes() {
        let rows = blocks("| Name | Status |\n| --- | --- |\n| Orbit | Ready |");
        assert_eq!(
            rows,
            vec![
                MessageBlock::TableRow {
                    cells: vec!["Name".into(), "Status".into()],
                    header: true
                },
                MessageBlock::TableRow {
                    cells: vec!["Orbit".into(), "Ready".into()],
                    header: false
                }
            ]
        );
        let original = "🦀".repeat(3000);
        let mut rows = Vec::new();
        emit_table(&[original.clone(), "Small".into()], false, &mut rows);
        let joined: String = rows
            .iter()
            .map(|r| {
                if let MessageBlock::TableRow { cells, .. } = r {
                    cells[0].as_str()
                } else {
                    panic!("table")
                }
            })
            .collect();
        assert_eq!(joined, original);
        assert!(
            rows.iter()
                .all(|r| if let MessageBlock::TableRow { cells, .. } = r {
                    cells.iter().map(String::len).sum::<usize>() <= 1500
                } else {
                    false
                })
        );
    }
    #[test]
    fn terminal_backticks_cannot_swallow_adjacent_internal_cards() {
        let cards = [
            ChatInteraction::Status { title: "Terminal output".into(), details: "const source = `unfinished template;\nUnicode 日本語 and <literal> remain exact".into(), status: InteractionStatus::Completed },
            ChatInteraction::Status { title: "Terminal command".into(), details: "python3 -c 'assert `value` == `expected`'".into(), status: InteractionStatus::Working },
            ChatInteraction::Status { title: "Terminal output".into(), details: "Verified output\n".into(), status: InteractionStatus::Completed },
        ];
        let text = cards
            .iter()
            .map(chat_interactions::serialize_interaction)
            .collect::<Vec<_>>()
            .join("\n\n");
        let expected = cards
            .iter()
            .cloned()
            .map(MessageBlock::Interaction)
            .collect::<Vec<_>>();
        assert_eq!(blocks(&text), expected);
        assert_eq!(message_blocks("assistant", &text), expected);
        assert_eq!(live_blocks(&text), expected);
        // Opposite odd tick counts in separate payloads must not form Markdown
        // around real prose or the next record's opening/closing marker.
        let trailing = ChatInteraction::Status {
            title: "Terminal output".into(),
            details: "another unmatched `".into(),
            status: InteractionStatus::Completed,
        };
        let mixed = format!(
            "{}\n\nReal prose with `literal code`.\n\n{}",
            chat_interactions::serialize_interaction(&cards[0]),
            chat_interactions::serialize_interaction(&trailing)
        );
        let parsed = blocks(&mixed);
        assert_eq!(
            parsed
                .iter()
                .filter(|r| matches!(r, MessageBlock::Interaction(_)))
                .count(),
            2
        );
        assert!(!parsed.iter().any(
            |r| matches!(r,MessageBlock::Paragraph(text) if text.contains("orbit_interaction"))
        ));
    }

    #[test]
    fn internal_card_literals_remain_literal_outside_real_records() {
        let card = ChatInteraction::Status {
            title: "Terminal output".into(),
            details: "literal ` backtick".into(),
            status: InteractionStatus::Completed,
        };
        let encoded = chat_interactions::serialize_interaction(&card);
        for example in [
            format!("```xml\n{encoded}\n```"),
            format!("Example ``{encoded}``."),
            serde_json::json!({"example":encoded}).to_string(),
        ] {
            assert!(
                !blocks(&example)
                    .iter()
                    .any(|r| matches!(r, MessageBlock::Interaction(_)))
            );
            assert!(
                !live_blocks(&example)
                    .iter()
                    .any(|r| matches!(r, MessageBlock::Interaction(_)))
            );
        }
        let literal = format!(
            "An example ``{encoded}``.\n\n{}",
            chat_interactions::serialize_interaction(&card)
        );
        assert_eq!(
            blocks(&literal)
                .iter()
                .filter(|r| matches!(r, MessageBlock::Interaction(_)))
                .count(),
            1
        );
    }

    #[test]
    #[ignore = "Read-only local transcript audit; set CODE_TRANSCRIPT_AUDIT_FILE"]
    fn recorded_internal_interaction_transcript_audit() {
        let path = std::env::var("CODE_TRANSCRIPT_AUDIT_FILE")
            .expect("Set a local exported transcript path");
        assert!(std::fs::metadata(&path).unwrap().len() <= 2 * 1024 * 1024);
        let text = std::fs::read_to_string(path).unwrap();
        let mut expected = vec![];
        for part in text.split("<orbit_interaction version=\"1\">").skip(1) {
            let (body, _) = part
                .split_once("</orbit_interaction>")
                .expect("Complete exported record");
            let (card, context) =
                chat_interactions::decode_with_context(body).expect("Valid exported record");
            expected.extend(bounded_interaction_with_context(card, context.as_deref()));
        }
        assert!(!expected.is_empty());
        let actual = message_blocks("assistant", &text)
            .into_iter()
            .filter(|b| matches!(b, MessageBlock::Interaction(_) | MessageBlock::Voice { .. }))
            .collect::<Vec<_>>();
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
            assert!(actual == expected, "Recorded interaction {index} changed");
        }
        eprintln!("Verified {} recorded interaction blocks", actual.len());
    }

    #[test]
    fn live_tail_withholds_partial_protocol_and_preserves_complete_cards() {
        let card = ChatInteraction::Status {
            title: "Completed".into(),
            details: "Done".into(),
            status: InteractionStatus::Completed,
        };
        let envelope = chat_interactions::serialize_interaction(&card);
        for end in 8..envelope.len() {
            let text = format!("Visible reply\n{}", &envelope[..end]);
            assert_eq!(
                live_blocks(&text),
                vec![MessageBlock::Paragraph("Visible reply".into())],
                "partial marker at {end}"
            );
        }
        assert_eq!(
            live_blocks(&format!("Visible reply\n{envelope}")),
            vec![
                MessageBlock::Paragraph("Visible reply".into()),
                MessageBlock::Interaction(card),
            ]
        );
        let orphan = format!("{}\nNext paragraph", &envelope[45..]);
        assert_eq!(
            live_blocks(&orphan),
            vec![MessageBlock::Paragraph("Next paragraph".into())]
        );
        for partial in [
            "<realtime_deleg",
            "<recommended_plugins",
            "<send_user_message_question_reply>[{\"answer\":",
            "<orbit_interaction version=\"1\">{\"orbit_interaction\":1",
        ] {
            assert_eq!(
                live_blocks(&format!("Visible\n{partial}")),
                vec![MessageBlock::Paragraph("Visible".into())]
            );
        }
        let voice = live_blocks(
            "<realtime_delegation><input>Real request</input><transcript_delta>assistant: Reply <transcript_d",
        );
        assert!(!format!("{voice:?}").contains("transcript_d"));
        assert!(
            voice
                .iter()
                .any(|row| matches!(row, MessageBlock::Voice { text,.. } if text=="Reply"))
        );
    }
    #[test]
    fn live_tail_preserves_literal_xml_json_code_and_bounds_unicode_work() {
        for text in [
            "<Widget>ordinary XML</Widget>",
            "```xml\n<orbit_interaction version=\"1\">unfinished\n```",
            "Example `<realtime_delegation>`.",
            "{\"example\":\"<orbit_interaction version=\\\"1\\\">literal</orbit_interaction>\"}",
        ] {
            assert_eq!(live_blocks(text), blocks(text));
        }
        let long = format!("{}\nCurrent reply", "🦀".repeat(100_000));
        assert_eq!(
            live_blocks(&long),
            vec![MessageBlock::Paragraph("Current reply".into())]
        );
    }
    #[test]
    fn voice_fields_do_not_close_inside_literal_code_examples() {
        let text = "<realtime_delegation><input>Explain this:\n```xml\n</input>\n</realtime_delegation>\n```\nThen continue `</input>` please.</input></realtime_delegation>Outside";
        let rows = message_blocks("user", text);
        let details: String = rows
            .iter()
            .filter_map(|row| match row {
                MessageBlock::Interaction(ChatInteraction::Delegation { details, .. }) => {
                    Some(details.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            details,
            "Explain this:\n```xml\n</input>\n</realtime_delegation>\n```\nThen continue `</input>` please."
        );
        assert!(rows.contains(&MessageBlock::Paragraph("Outside".into())));
    }
}

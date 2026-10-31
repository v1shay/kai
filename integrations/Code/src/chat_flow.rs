//! Canonical transcript presentation. Folding keeps prompts and final answers.
use super::{Row,MessageBlock,chat_interactions::{ChatInteraction,Speaker,InteractionStatus}};
use std::ops::Range;
#[derive(Default,Clone,Debug,PartialEq,Eq)]
pub(super) struct Index {pub message_ends:Vec<Option<Range<usize>>>,pub completion_ends:Vec<Option<usize>>,pub work:Vec<Option<Range<usize>>>,pub commands:Vec<Option<Range<usize>>>,pub hidden:Vec<bool>,pub has_subagents:bool}
impl Index {
 pub fn new(rows:&[Row])->Self{Self::for_transcript(rows,false,None)}
 pub fn for_transcript(rows:&[Row],settled:bool,live_start:Option<usize>)->Self{
  let mut out=Self{message_ends:vec![None;rows.len()],completion_ends:vec![None;rows.len()],work:vec![None;rows.len()],commands:vec![None;rows.len()],hidden:vec![false;rows.len()],has_subagents:false};
  let mut at=0;while at<rows.len(){if let Some(span)=command_span(rows,at){for i in span.clone(){out.commands[i]=Some(span.clone())}at=span.end}else{at+=1}}
  let mut starts=vec![0];for i in 0..rows.len(){if matches!(&rows[i].block,MessageBlock::Interaction(ChatInteraction::Delegation{agent:Some(_),..})){out.has_subagents=true;}if starts_turn(&rows[i])&&i>0{starts.push(i)}}starts.push(rows.len());
  for bounds in starts.windows(2){
   let begin=bounds[0];let end=bounds[1];if begin==end{continue}
   let body=(begin..end).find(|i|!user(&rows[*i])&&!matches!(rows[*i].block,MessageBlock::Attachment{..})).unwrap_or(end);if body==end{continue}
   let terminal=(body..end).rev().find(|i|is_terminal(&rows[*i].block));
   let succeeded=terminal.is_some_and(|i|is_success(&rows[i].block));let failed=terminal.is_some_and(|i|!is_success(&rows[i].block));
   let boundary=(body..end).rev().find(|i|is_boundary(&rows[*i].block));
   let anchored=starts_turn(&rows[begin])||boundary.is_some();
   let complete=!failed&&(succeeded||end<rows.len()||live_start.is_some_and(|start|end<=start)||settled&&end==rows.len()&&anchored);
   for i in body..end{if is_boundary(&rows[i].block)||is_checkpoint(&rows[i].block){out.hidden[i]=true}}
   if !complete{if terminal.is_some(){if let Some(last)=(body..end).rev().find(|i|body_content(&rows[*i].block)){out.completion_ends[last]=terminal;}}continue}
   let final_start=boundary.map(|i|i+1).or_else(||(body..end).rev().find(|i|body_content(&rows[*i].block)).map(|last|{
    let message=(body..=last).rev().find(|i|assistant_start(&rows[*i])).unwrap_or(body);
    (message..last).rev().find(|i|work_item(&rows[*i].block)).map_or(message,|i|i+1)
   }));
   let Some(final_start)=final_start.filter(|start|(*start..end).any(|i|body_content(&rows[i].block)))else{continue};
   if let Some(last)=(final_start..end).rev().find(|i|body_content(&rows[*i].block)){out.completion_ends[last]=Some(terminal.unwrap_or(last));}
   for i in body..end{if is_success(&rows[i].block){out.hidden[i]=true}}
   if let Some(first)=(body..final_start).find(|i|!out.hidden[*i]&&!user(&rows[*i])&&!attention(&rows[*i].block)){for i in first..final_start{if !user(&rows[i])&&!attention(&rows[i].block){out.work[i]=Some(first..final_start)}}}
   for i in final_start..end{if work_item(&rows[i].block)&&!attention(&rows[i].block){out.hidden[i]=true}}
  }
  // A footer belongs to a complete authored message, never a Markdown paragraph
  // or a tool result. Store ranges only; build clipboard text on click.
  let mut message:Option<usize>=None;
  for i in 0..=rows.len(){
   let boundary=i==rows.len() || rows.get(i).is_some_and(|r| {
    let role=r.role.as_deref();
    role.is_some_and(|r|r!="user:continuation"&&!r.starts_with("continuation:")) || !body_content(&r.block)&&!user(r)
   });
   if boundary {
    if let Some(start)=message.take(){
     if let Some(end)=(start..i).rev().find(|j|!out.hidden[*j] && (user(&rows[*j])||body_content(&rows[*j].block))) {out.message_ends[end]=Some(start..i);}
    }
   }
   if i<rows.len() && message.is_none() && (user(&rows[i])||body_content(&rows[i].block)) && !matches!(rows[i].block,MessageBlock::Interaction(_)) {message=Some(i);}
  }
  out
 }
 pub fn visible(&self,ix:usize,expanded:&std::collections::HashSet<usize>)->bool{if self.hidden.get(ix)==Some(&true){return false}if self.commands.get(ix).and_then(|s|s.as_ref()).is_some_and(|r|r.start!=ix){return false}self.work.get(ix).and_then(|r|r.as_ref()).is_none_or(|r|r.start==ix||expanded.contains(&r.start))}
 pub fn anchor(&self,ix:usize,expanded:&std::collections::HashSet<usize>)->usize{if self.visible(ix,expanded){return ix}if let Some(span)=self.work.get(ix).and_then(|s|s.as_ref()){if !expanded.contains(&span.start){return span.start}}if let Some(span)=self.commands.get(ix).and_then(|s|s.as_ref()){if self.visible(span.start,expanded){return span.start}}(ix..self.work.len()).find(|i|self.visible(*i,expanded)).or_else(||(0..ix.min(self.work.len())).rev().find(|i|self.visible(*i,expanded))).unwrap_or(self.work.len())}
 pub fn tail_start(&self,end:usize,budget:usize,expanded:&std::collections::HashSet<usize>)->usize{let mut left=budget;for i in (0..end.min(self.work.len())).rev(){if self.visible(i,expanded){left=left.saturating_sub(1);if left==0{return i}}}0}
}
pub(super) fn user(row:&Row)->bool{row.role.as_deref().is_some_and(|r|r=="user"||r.starts_with("user:"))||matches!(&row.block,MessageBlock::Voice{speaker:Speaker::User,..}|MessageBlock::Interaction(ChatInteraction::Speaker{speaker:Speaker::User,..}))}
fn starts_turn(row:&Row)->bool{row.role.as_deref().is_some_and(|r|r.split(':').next()==Some("user")&&!r.starts_with("user:continuation"))||matches!(&row.block,MessageBlock::Voice{speaker:Speaker::User,..}|MessageBlock::Interaction(ChatInteraction::Speaker{speaker:Speaker::User,..}))}
fn assistant_start(row:&Row)->bool{row.role.as_deref().is_some_and(|r|r=="assistant"||r.starts_with("assistant:"))}
fn is_checkpoint(block:&MessageBlock)->bool{matches!(block,MessageBlock::Activity{label,..}if label=="Prompt changes")}
fn is_boundary(block:&MessageBlock)->bool{matches!(block,MessageBlock::Interaction(ChatInteraction::Status{title,..})if title=="Final answer")}
fn is_success(block:&MessageBlock)->bool{matches!(block,MessageBlock::Interaction(ChatInteraction::Status{title,status:InteractionStatus::Completed,..})if title=="Completed")}
fn is_terminal(block:&MessageBlock)->bool{matches!(block,MessageBlock::Interaction(ChatInteraction::Status{title,..})if matches!(title.as_str(),"Completed"|"Stopped"|"Agent failed"))}
fn body_content(block:&MessageBlock)->bool{matches!(block,MessageBlock::Paragraph(_)|MessageBlock::RichParagraph{..}|MessageBlock::Heading(_)|MessageBlock::Quote(_)|MessageBlock::ListItem{..}|MessageBlock::Code{..}|MessageBlock::TableRow{..}|MessageBlock::Voice{speaker:Speaker::Assistant,..}|MessageBlock::Attachment{..}|MessageBlock::Interaction(ChatInteraction::Speaker{speaker:Speaker::Assistant,..}))}
fn work_item(block:&MessageBlock)->bool{matches!(block,MessageBlock::Activity{..}|MessageBlock::Interaction(ChatInteraction::Status{..}|ChatInteraction::Delegation{..}|ChatInteraction::Answer{..}|ChatInteraction::Question(_)|ChatInteraction::NativeRequest(_)))}
fn attention(block:&MessageBlock)->bool{matches!(block,MessageBlock::Interaction(ChatInteraction::Status{status:InteractionStatus::Failed|InteractionStatus::Cancelled,..}|ChatInteraction::Delegation{status:InteractionStatus::Failed|InteractionStatus::Cancelled,..}))}
pub(super) fn command(block:&MessageBlock)->Option<(&str,&str,InteractionStatus)>{match block{MessageBlock::Activity{label,details}if matches!(label.as_str(),"Terminal command"|"Terminal output"|"Ran a command"|"Ran commands")=>Some((label,details,InteractionStatus::Completed)),MessageBlock::Interaction(ChatInteraction::Status{title,details,status})if matches!(title.as_str(),"Terminal command"|"Terminal output"|"Ran a command"|"Ran commands")=>Some((title,details,*status)),_=>None}}
pub(super) fn command_span(rows:&[Row],ix:usize)->Option<Range<usize>>{command(&rows.get(ix)?.block)?;let mut start=ix;while start>0&&command(&rows[start-1].block).is_some(){start-=1}let mut end=ix+1;while end<rows.len()&&command(&rows[end].block).is_some(){end+=1}Some(start..end)}
pub(super) fn grouped_command(rows:&[Row],span:Range<usize>)->(String,InteractionStatus){let mut text=String::new();let mut status=InteractionStatus::Completed;for row in &rows[span]{if let Some((title,details,next))=command(&row.block){if !text.is_empty(){text.push_str("\n\n")};text.push_str(title);text.push('\n');text.push_str(details);if title!="Terminal output"||next==InteractionStatus::Failed{status=next}}}(text,status)}
#[cfg(test)]pub(super) fn completed_work(rows:&[Row],ix:usize)->Option<Range<usize>>{Index::new(rows).work.get(ix).cloned().flatten()}
/// Keep turn/phase identity when a decoded page leaves memory. Its text remains
/// on disk; losing this metadata would merge unrelated turns on the next paint.
pub(super) fn placeholder(row:&Row)->Row{
 let block=match &row.block{
  MessageBlock::Interaction(ChatInteraction::Status{title,status,..})=>MessageBlock::Interaction(ChatInteraction::Status{title:title.clone(),details:String::new(),status:*status}),
  MessageBlock::Activity{label,..}=>MessageBlock::Activity{label:label.clone(),details:String::new()},
  MessageBlock::Voice{speaker,..}|MessageBlock::Interaction(ChatInteraction::Speaker{speaker,..})=>MessageBlock::Voice{speaker:*speaker,text:String::new(),highlights:vec![]},
  MessageBlock::Interaction(_)=>MessageBlock::Activity{label:"Work".into(),details:String::new()},
  MessageBlock::Attachment{..}=>MessageBlock::Attachment{name:String::new(),target:String::new()},
  _=>MessageBlock::Paragraph(String::new()),
 };Row{role:row.role.clone(),block}
}
#[cfg(test)]mod tests{
 use super::*;
 fn row(block:MessageBlock)->Row{Row{role:Some("assistant:codex".into()),block}}
 #[test]fn commands_group_without_merging_across_an_agent_update(){
  let tool=||row(MessageBlock::Activity{label:"Terminal command".into(),details:"pwd".into()});
  let rows=vec![tool(),tool(),row(MessageBlock::Paragraph("Checking files".into())),tool()];
  assert_eq!(command_span(&rows,1),Some(0..2));assert_eq!(command_span(&rows,3),Some(3..4));assert_eq!(grouped_command(&rows,0..2).0.matches("pwd").count(),2);
 }
 #[test]fn only_finished_work_collapses_and_the_final_answer_stays_outside(){
  let mut rows=vec![Row{role:Some("user".into()),block:MessageBlock::Paragraph("Build it".into())},row(MessageBlock::Paragraph("Checking".into())),row(MessageBlock::Activity{label:"Terminal command".into(),details:"pwd".into()}),row(MessageBlock::Paragraph("Done, here is the result".into()))];
  assert_eq!(completed_work(&rows,1),None);rows.push(row(MessageBlock::Interaction(ChatInteraction::Status{title:"Completed".into(),details:String::new(),status:InteractionStatus::Completed})));
  assert_eq!(completed_work(&rows,1),Some(1..3));assert_eq!(completed_work(&rows,3),None);
  assert!(user(&row(MessageBlock::Voice{speaker:Speaker::User,text:"Hello".into(),highlights:vec![]})));
 }
 #[test]fn explicit_final_phase_folds_commentary_even_without_any_tool_call(){
  let status=|title:&str|row(MessageBlock::Interaction(ChatInteraction::Status{title:title.into(),details:String::new(),status:InteractionStatus::Completed}));
  let mut rows=vec![row(MessageBlock::Paragraph("Here is my plan".into())),status("Final answer"),row(MessageBlock::Paragraph("The actual final result".into()))];
  assert_eq!(completed_work(&rows,0),None);rows.push(status("Completed"));assert_eq!(completed_work(&rows,0),Some(0..2));assert_eq!(completed_work(&rows,2),None);
 }
 #[test]fn completed_turns_keep_every_prompt_and_final_and_eviction_keeps_the_same_projection(){
  let mut rows=vec![];
  for n in 0..3{rows.push(Row{role:Some("user".into()),block:MessageBlock::Paragraph(format!("Prompt {n}"))});rows.push(Row{role:Some("user:continuation".into()),block:MessageBlock::Attachment{name:"image".into(),target:"/image".into()}});rows.push(row(MessageBlock::Paragraph("Commentary".into())));rows.push(row(MessageBlock::Interaction(ChatInteraction::Status{title:"Final answer".into(),details:String::new(),status:InteractionStatus::Completed})));rows.push(Row{role:Some("continuation:codex".into()),block:MessageBlock::Paragraph(format!("Final {n}"))});rows.push(row(MessageBlock::Interaction(ChatInteraction::Status{title:"Completed".into(),details:String::new(),status:InteractionStatus::Completed})));}
  let flow=Index::new(&rows);let open=std::collections::HashSet::new();
  for n in 0..3{assert!(flow.visible(n*6,&open));assert!(flow.visible(n*6+1,&open));assert!(flow.visible(n*6+4,&open));assert!(!flow.visible(n*6+3,&open));assert!(!flow.visible(n*6+5,&open));assert_eq!(flow.work[n*6+2],Some(n*6+2..n*6+4));}
  let evicted:Vec<_>=rows.iter().map(placeholder).collect();assert_eq!(flow,Index::new(&evicted));assert_eq!(flow.tail_start(rows.len(),100,&open),0);
 }
 #[test]fn final_history_message_is_complete_including_all_its_paragraphs_without_a_provider_phase(){
  let rows=vec![Row{role:Some("user".into()),block:MessageBlock::Paragraph("Prompt".into())},row(MessageBlock::Paragraph("Checking".into())),row(MessageBlock::Paragraph("Final first paragraph".into())),Row{role:Some("continuation:codex".into()),block:MessageBlock::Paragraph("Final second paragraph".into())}];
  let flow=Index::for_transcript(&rows,true,None);assert_eq!(flow.work[1],Some(1..2));let open=std::collections::HashSet::new();assert!(flow.visible(0,&open)&&flow.visible(2,&open)&&flow.visible(3,&open));assert!(Index::for_transcript(&rows,false,Some(1)).work.iter().all(Option::is_none));
 }
}

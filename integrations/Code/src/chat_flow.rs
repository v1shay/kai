//! Transcript presentation policy. Source messages and native requests are kept.
use super::{Row,MessageBlock,chat_interactions::{ChatInteraction,Speaker,InteractionStatus}};
#[derive(Default)]pub(super) struct Index{pub work:Vec<Option<std::ops::Range<usize>>>,pub commands:Vec<Option<std::ops::Range<usize>>>}
impl Index{
 pub fn new(rows:&[Row])->Self{
  let mut index=Self{work:vec![None;rows.len()],commands:vec![None;rows.len()]};let mut at=0;
  while at<rows.len(){if let Some(span)=command_span(rows,at){for i in span.clone(){index.commands[i]=Some(span.clone())}at=span.end}else{at+=1}}
  let mut at=0;while at<rows.len(){if user(&rows[at]){at+=1;continue}let end=(at..rows.len()).find(|i|user(&rows[*i])).unwrap_or(rows.len());let candidate=(at..end).find(|i|!matches!(&rows[*i].block,MessageBlock::Activity{label,..} if label=="Prompt changes"));if let Some(span)=candidate.and_then(|i|completed_work(rows,i)){for i in span.clone(){index.work[i]=Some(span.clone())}}at=end;}
  index
 }
}
pub(super) fn user(row:&Row)->bool{
    row.role.as_deref().is_some_and(|r|r=="user"||r.starts_with("user:")) || matches!(&row.block,MessageBlock::Voice{speaker:Speaker::User,..}|MessageBlock::Interaction(ChatInteraction::Speaker{speaker:Speaker::User,..}))
}
pub(super) fn command(block:&MessageBlock)->Option<(&str,&str,InteractionStatus)>{
    match block{
        MessageBlock::Activity{label,details} if matches!(label.as_str(),"Terminal command"|"Terminal output"|"Ran a command"|"Ran commands")=>Some((label,details,InteractionStatus::Completed)),
        MessageBlock::Interaction(ChatInteraction::Status{title,details,status}) if matches!(title.as_str(),"Terminal command"|"Terminal output"|"Ran a command"|"Ran commands")=>Some((title,details,*status)),_=>None
    }
}
pub(super) fn command_span(rows:&[Row],ix:usize)->Option<std::ops::Range<usize>>{
    command(&rows.get(ix)?.block)?;let mut start=ix;while start>0&&command(&rows[start-1].block).is_some(){start-=1}
    let mut end=ix+1;while end<rows.len()&&command(&rows[end].block).is_some(){end+=1}Some(start..end)
}
pub(super) fn grouped_command(rows:&[Row],span:std::ops::Range<usize>)->(String,InteractionStatus){
    let mut text=String::new();let mut status=InteractionStatus::Completed;
    for row in &rows[span]{if let Some((title,details,next))=command(&row.block){
        if !text.is_empty(){text.push_str("\n\n")};text.push_str(title);text.push('\n');text.push_str(details);
        if title!="Terminal output"||next==InteractionStatus::Failed{status=next}
    }}(text,status)
}
pub(super) fn completed_work(rows:&[Row],ix:usize)->Option<std::ops::Range<usize>>{
    if user(rows.get(ix)?){return None}
    let turn=(0..=ix).rev().find(|i|user(&rows[*i])).map_or(0,|i|i+1);
    let end=(ix+1..rows.len()).find(|i|user(&rows[*i])).unwrap_or(rows.len());
    let completion=(turn..end).rev().find(|i|matches!(&rows[*i].block,MessageBlock::Interaction(ChatInteraction::Status{title,status:InteractionStatus::Completed,..}) if title=="Completed"))?;
    if let Some(final_start)=(turn..completion).rev().find(|i|matches!(&rows[*i].block,MessageBlock::Interaction(ChatInteraction::Status{title,..}) if title=="Final answer")){
        let first=(turn..=final_start).find(|i|!matches!(&rows[*i].block,MessageBlock::Activity{label,..} if label=="Prompt changes"))?;
        if rows[first..final_start].iter().any(|r|matches!(&r.block,MessageBlock::Interaction(ChatInteraction::NativeRequest(_)|ChatInteraction::Question(_)|ChatInteraction::Status{status:InteractionStatus::Failed,..}))){return None}
        return (ix>=first&&ix<=final_start).then_some(first..final_start+1)
    }
    let last_action=(turn..completion).rev().find(|i|matches!(&rows[*i].block,MessageBlock::Activity{label,..} if label!="Prompt changes")||matches!(&rows[*i].block,MessageBlock::Interaction(ChatInteraction::Status{title,..}) if title!="Completed"))?;
    if !(last_action+1..completion).any(|i|matches!(&rows[i].block,MessageBlock::Paragraph(_)|MessageBlock::RichParagraph{..}|MessageBlock::Heading(_)|MessageBlock::ListItem{..}|MessageBlock::Code{..})){return None}
    let first=(turn..=last_action).find(|i|!matches!(&rows[*i].block,MessageBlock::Activity{label,..} if label=="Prompt changes"))?;
    // Requests and failures remain visible and actionable, never folded away.
    if rows[first..=last_action].iter().any(|r|matches!(&r.block,MessageBlock::Interaction(ChatInteraction::NativeRequest(_)|ChatInteraction::Question(_)|ChatInteraction::Status{status:InteractionStatus::Failed,..}))){return None}
    (ix>=first&&ix<=last_action).then_some(first..last_action+1)
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
}

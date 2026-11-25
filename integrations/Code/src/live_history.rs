//! On-demand current-turn history. Parsing and immutable snapshots stay off UI.
use super::*;
use crate::response_buffer::ResponseSource;
pub(super) struct View{pub owner:String,pub epoch:uuid::Uuid,pub base:usize,provider:AgentKind,source:ResponseSource,start:usize,end:usize,text:String}
impl View{
 fn new(owner:String,epoch:uuid::Uuid,base:usize,provider:AgentKind,source:ResponseSource)->Self{let end=source.len();Self{owner,epoch,base,provider,source,start:end,end,text:String::new()}}
 fn older(mut self)->anyhow::Result<(Self,Vec<MessageBlock>)>{
  let prefix=match &self.source{ResponseSource::Memory(s)=>ResponseSource::Memory(s[..self.start].into()),ResponseSource::File{artifact,..}=>ResponseSource::File{artifact:artifact.clone(),len:self.start}};
  let mut budget=64*1024;let mut earlier=prefix.tail(budget)?;
  while earlier.find("</orbit_interaction>").is_some_and(|close|earlier.find("<orbit_interaction").is_none_or(|open|close<open))&&earlier.len()<self.start&&budget<2*1024*1024+512{
   budget=(budget*2).min(2*1024*1024+512);earlier=prefix.tail(budget)?;
  }
  self.start-=earlier.len();earlier.push_str(&self.text);self.text=earlier;
  if self.text.len()>256*1024{let mut end=256*1024;while !self.text.is_char_boundary(end){end-=1}self.text.truncate(end);self.end=self.start+self.text.len();}
  let blocks=presentation::live_page_blocks(&self.text);Ok((self,blocks))
 }
}
impl Orbit{
 pub(super) fn load_live_history(&mut self,cx:&mut Context<Self>)->bool{
  let Some(owner)=self.active.clone()else{return false};if self.loading||self.loading_earlier{return true}
  if self.live_history.as_ref().is_some_and(|v|v.owner!=owner){self.live_history=None}
  if self.live_history.as_ref().is_some_and(|v|v.start==0){return false}
  let existing=self.live_history.take();
  let (epoch,base,provider,receiver)=if let Some(v)=&existing{(v.epoch,v.base,v.provider,None)}else{
   let Some(run)=self.runs.get(&owner).filter(|run|run.live_earlier)else{return false};
   let Ok(receiver)=run.handle.live_snapshot()else{return false};(run.parse_epoch,run.stream_start.unwrap_or(self.rows.len()),run.agent,Some(receiver))
  };
  let generation=self.generation;self.loading_earlier=true;let anchor=self.list.logical_scroll_top();
  let anchor_row=self.rows.get(anchor.item_ix).cloned();let worker=cx.background_executor().clone();
  cx.spawn(async move|this,cx|{
   let result:Result<(View,Vec<MessageBlock>),String>=async{
    let view=if let Some(v)=existing{v}else{
     let receiver=receiver.unwrap();let deadline=Instant::now()+Duration::from_secs(10);
     let received=loop{match receiver.try_recv(){Ok(result)=>break result?,Err(async_channel::TryRecvError::Closed)=>return Err("History snapshot closed".into()),Err(async_channel::TryRecvError::Empty)=>{if Instant::now()>=deadline{return Err("History snapshot timed out".into())}Timer::after(Duration::from_millis(50)).await;}}};
     View::new(owner.clone(),epoch,base,provider,received)
    };
    worker.spawn(async move{view.older().map_err(|e|e.to_string())}).await
   }.await;
   let _=this.update(cx,|this,cx|{
    if this.generation!=generation||this.active.as_deref()!=Some(&owner){return}
    if this.runs.get(&owner).is_some_and(|r|r.parse_epoch!=epoch){this.loading_earlier=false;return}
    this.loading_earlier=false;
    match result{
     Ok((view,blocks))=>{
      let base=view.base.min(this.rows.len());this.rows.truncate(base);
      this.rows.extend(blocks.into_iter().enumerate().map(|(i,block)|Row{role:Some(format!("{}:{}",if i==0{"assistant"}else{"continuation"},provider.command())),block}));
      this.navigation_history=true;this.following_bottom=false;this.has_older=view.start>0||this.has_older;this.has_newer=true;this.transcript_reveal=None;
      let position=anchor_row.and_then(|old|this.rows.iter().enumerate().skip(if anchor.item_ix<base{0}else{base}).find(|(_,r)|r.role==old.role&&r.block==old.block).map(|(i,_)|i)).unwrap_or(base);
      this.live_history=Some(view);this.list.reset(this.rows.len());this.list.scroll_to(ListOffset{item_ix:position,offset_in_item:anchor.offset_in_item});
     },Err(error)=>this.notice=Some(error),
    }cx.notify();
   });
  }).detach();true
 }
}
#[cfg(test)]mod tests{
 use super::*;use std::prelude::v1::test;
 #[test]fn paging_retains_earlier_unicode_work_in_a_bounded_window(){
  let text=(0..9000).map(|n|format!("Step {n} 🐈\n")).collect::<String>();let source=ResponseSource::Memory(text);
  let mut view=View::new("chat".into(),uuid::Uuid::new_v4(),0,AgentKind::Codex,source);let mut saw_first=false;
  for _ in 0..8{let(next,blocks)=view.older().unwrap();view=next;assert!(!blocks.is_empty());assert!(view.text.len()<=256*1024);saw_first|=view.text.contains("Step 0 🐈");if view.start==0{break}}
  assert!(saw_first,"Earlier work disappeared instead of paging from its retained source");
 }
}

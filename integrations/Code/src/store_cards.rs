//! App-owned card boundaries inside saved SQLite replies. Physical paging may
//! intersect a card; each slice must carry a complete typed envelope of its own.
use crate::chat_interactions::{self,ChatInteraction};
use anyhow::{Result,ensure};
use rusqlite::blob::Blob;
use std::{fs::File,io::{Read,Seek,SeekFrom,Write},ops::Range};

const OPEN:&[u8]=b"<orbit_interaction version=\"1\">";
const CLOSE:&[u8]=b"</orbit_interaction>";
pub(super) const MAGIC:&[u8;8]=b"KAICARD1";

#[derive(serde::Serialize,serde::Deserialize)]
pub(super) struct Scanner {
    position:usize,
    #[serde(default="yes")] line_white:bool,
    candidate:usize,
    begin:usize,
    active:bool,
    closing:usize,
    #[serde(skip)] pub completed:Option<Range<usize>>,
}
fn yes()->bool{true}
impl Default for Scanner{fn default()->Self{Self{position:0,line_white:true,candidate:0,begin:0,active:false,closing:0,completed:None}}}
impl Scanner {
    pub fn feed(&mut self,byte:u8,allowed:bool){
        let at=self.position;self.position+=1;
        if self.active {
            self.closing=if CLOSE.get(self.closing)==Some(&byte){self.closing+1}else{usize::from(byte==CLOSE[0])};
            if self.closing==CLOSE.len(){self.completed=Some(self.begin..self.position);self.active=false;self.closing=0;self.line_white=false;}
            return
        }
        if self.candidate>0 {
            if allowed && OPEN.get(self.candidate)==Some(&byte){
                self.candidate+=1;
                if self.candidate==OPEN.len(){self.active=true;self.candidate=0;}
                return
            }
            self.candidate=0;self.line_white=false;
        }
        if allowed && self.line_white && byte==OPEN[0]{self.begin=at;self.candidate=1;return}
        if byte==b'\n'{self.line_white=true}else if !byte.is_ascii_whitespace(){self.line_white=false}
    }
}

struct BlobRange<'a,'b>{blob:&'a Blob<'b>,position:usize,end:usize}
impl Read for BlobRange<'_,'_>{
    fn read(&mut self,buffer:&mut [u8])->std::io::Result<usize>{
        let count=buffer.len().min(self.end-self.position);
        self.blob.read_at_exact(&mut buffer[..count],self.position).map_err(std::io::Error::other)?;
        self.position+=count;Ok(count)
    }
}

#[derive(Default)]
pub(super) struct Cache{last:Option<(usize,Option<(ChatInteraction,Option<String>)>)>}
impl Cache {
    fn fragment(&mut self,blob:&Blob<'_>,card:Range<usize>,slice:Range<usize>)->Result<Option<String>>{
        if self.last.as_ref().is_none_or(|(at,_)|*at!=card.start){
            let source=BlobRange{blob,position:card.start+OPEN.len(),end:card.end-CLOSE.len()};
            // Read one indexed card through the incremental BLOB, never the
            // complete saved reply. Only this card is retained during paging.
            let decoded=serde_json::from_reader(source).ok().and_then(chat_interactions::decode_value_with_context);
            self.last=Some((card.start,decoded));
        }
        let Some((value,context))=self.last.as_ref().and_then(|(_,v)|v.as_ref())else{return Ok(None)};
        let part=|text:&str|{
            let at=|offset:usize|{
                let mut boundary=((text.len() as u128*(offset-card.start) as u128)/(card.len() as u128)) as usize;
                while !text.is_char_boundary(boundary){boundary-=1}boundary
            };
            text[at(slice.start)..at(slice.end)].to_owned()
        };
        let value=match value {
            ChatInteraction::Status{title,details,status}=>ChatInteraction::Status{title:title.clone(),details:part(details),status:*status},
            ChatInteraction::Delegation{title,details,status,agent}=>ChatInteraction::Delegation{title:title.clone(),details:part(details),status:*status,agent:agent.clone()},
            // Submitted questions and voice messages remain atomic. Their
            // terminating slice owns them; partial prefixes are never exposed.
            value if slice.end==card.end=>value.clone(),
            _=>return Ok(Some(String::new())),
        };
        Ok(Some(format!("\n<orbit_interaction version=\"1\">{}</orbit_interaction>\n",chat_interactions::encode_with_context(&value,context.as_deref()).replace('<',"\\u003c"))))
    }
}

pub(super) fn count(file:&mut File)->Result<Option<(u64,u64)>>{
    let length=file.metadata()?.len();
    if length<16{return Ok(None)}
    file.seek(SeekFrom::End(-16))?;let mut footer=[0;16];file.read_exact(&mut footer)?;
    if &footer[..8]!=MAGIC{return Ok(None)}
    let count=u64::from_le_bytes(footer[8..].try_into().unwrap());
    ensure!(count<=(length-16)/16,"Invalid saved-card index");
    Ok(Some((length-16-count*16,count)))
}
pub(super) fn write(file:&mut File,range:Range<usize>)->Result<()>{file.write_all(&(range.start as u64).to_le_bytes())?;file.write_all(&(range.end as u64).to_le_bytes())?;Ok(())}
fn range(file:&mut File,base:u64,entry:u64)->Result<Range<usize>>{
    file.seek(SeekFrom::Start(base+entry*16))?;let mut raw=[0;16];file.read_exact(&mut raw)?;
    Ok(u64::from_le_bytes(raw[..8].try_into().unwrap()) as usize..u64::from_le_bytes(raw[8..].try_into().unwrap()) as usize)
}
pub(super) fn project(file:&mut File,cache:&mut Cache,blob:&Blob<'_>,offset:usize,end:usize,body:&str,hidden:Vec<Range<usize>>)->Result<String>{
    let (base,count)=count(file)?.expect("Saved-card index initialized");
    let mut low=0;let mut high=count;
    while low<high{let mid=(low+high)/2;if range(file,base,mid)?.end<=offset{low=mid+1}else{high=mid}}
    let mut edits=hidden.into_iter().map(|r|(r,String::new())).collect::<Vec<_>>();
    for entry in low..count{
        let card=range(file,base,entry)?;if card.start>=end{break}
        let slice=card.start.max(offset)..card.end.min(end);
        if let Some(replacement)=cache.fragment(blob,card,slice.clone())?{edits.push((slice.start-offset..slice.end-offset,replacement));}
    }
    edits.sort_by_key(|(r,_)|r.start);
    let mut output=String::with_capacity(body.len());let mut at=0;
    for (range,replacement) in edits {if range.start<at{continue}output.push_str(&body[at..range.start]);output.push_str(&replacement);at=range.end;}
    output.push_str(&body[at..]);Ok(output)
}

//! Bounded paint-only decode over the original immutable transcript. Existing
//! ListReveal supplies the top-down entrance; this supplies Kai's glyph dissolve.
use std::{collections::HashMap,hash::{Hash,Hasher},time::{Duration,Instant}};
struct Entry{source:String,fingerprint:u64,prefix:usize,started:Instant}
#[derive(Default)]
pub(super) struct TextDecode{rows:HashMap<usize,Entry>}
impl TextDecode {
    pub fn frame(&mut self,row:usize,source:&str,now:Instant)->(String,bool){
        self.frame_mode(row,source,now,false)
    }
    pub fn voice_frame(&mut self,row:usize,source:&str,now:Instant)->(String,bool){
        self.frame_mode(row,source,now,true)
    }
    fn frame_mode(&mut self,row:usize,source:&str,now:Instant,voice:bool)->(String,bool){
        if source.len()>16*1024{return(source.into(),false)}
        let mut hash=std::collections::hash_map::DefaultHasher::new();source.hash(&mut hash);let fingerprint=hash.finish();
        if self.rows.len()>=128&&!self.rows.contains_key(&row){if let Some(old)=self.rows.iter().min_by_key(|(_,entry)|entry.started).map(|(id,_)|*id){self.rows.remove(&old);}}
        let entry=self.rows.entry(row).or_insert_with(||Entry{source:source.into(),fingerprint,prefix:0,started:now});
        if entry.fingerprint!=fingerprint{
            let prefix=entry.source.bytes().zip(source.bytes()).take_while(|(a,b)|a==b).count();
            if voice || now.duration_since(entry.started)>=Duration::from_millis(300){entry.started=now;entry.prefix=prefix;}else{entry.prefix=entry.prefix.min(prefix);}
            entry.source=source.into();entry.fingerprint=fingerprint;
        }
        let progress=(now.duration_since(entry.started).as_secs_f32()/if voice{0.16}else{0.30}).clamp(0.,1.);
        (paint(source,entry.prefix,progress,fingerprint),progress<1.)
    }
}
fn paint(source:&str,prefix:usize,progress:f32,seed:u64)->String{
    if progress>=1.{return source.into()}
    let mut bytes=source.as_bytes().to_vec();let phase=(progress*18.) as u64;
    let alphabet=b"abcdefghijklmnoprstuvxyz0123456789";
    for(index,byte)in bytes.iter_mut().enumerate().skip(prefix).take(256){
        if byte.is_ascii_alphanumeric(){
            let noise=seed.wrapping_add((index as u64).wrapping_mul(0x9e3779b9)).rotate_left(13);
            if (noise%100) as f32>progress*100.{*byte=alphabet[(noise.wrapping_add(phase*7)%alphabet.len() as u64)as usize];}
        }
    }
    // Replacements are single-byte ASCII; all UTF-8 boundaries and link offsets survive.
    String::from_utf8(bytes).expect("ASCII-only replacement preserves UTF-8")
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn decode_preserves_unicode_ranges_and_finishes_exactly(){let text="Hello 🐈 café /tmp/文書";let ghost=paint(text,0,0.1,42);assert_eq!(ghost.len(),text.len());assert!(ghost.contains("🐈"));assert!(ghost.contains("文書"));assert_eq!(paint(text,0,1.,42),text);}
    #[test]fn streaming_keeps_old_prefix_and_has_a_finite_settle(){let mut cache=TextDecode::default();let start=Instant::now();cache.frame(0,"Already here",start);let later=start+Duration::from_millis(400);let (value,active)=cache.frame(0,"Already here new words",later);assert!(active);assert!(value.starts_with("Already here"));assert_eq!(cache.frame(0,"Already here new words",later+Duration::from_millis(310)),("Already here new words".into(),false));}
    #[test]fn voice_deltas_only_decode_new_text_and_settle_quickly(){
        let mut cache=TextDecode::default();let start=Instant::now();
        cache.voice_frame(0,"Hello 🐈",start);
        let next=start+Duration::from_millis(20);
        let (text,active)=cache.voice_frame(0,"Hello 🐈 there",next);assert!(active);assert!(text.starts_with("Hello 🐈"));assert_eq!(text.len(),"Hello 🐈 there".len());
        let corrected=next+Duration::from_millis(20);
        let (text,_)=cache.voice_frame(0,"Hello 🐈 friend",corrected);assert!(text.starts_with("Hello 🐈 "));
        assert_eq!(cache.voice_frame(0,"Hello 🐈 friend",corrected+Duration::from_millis(161)),("Hello 🐈 friend".into(),false));
    }
}

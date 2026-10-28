//! Bounded in-memory interaction metadata with a private sequential spill log.
use crate::history_artifact::HistoryArtifact;
use anyhow::{Result, ensure};
use std::{
    fs::{File, OpenOptions},
    io::{BufReader, Read, Write},
    os::unix::fs::OpenOptionsExt,
    sync::Arc,
};

const MEMORY_BYTES: usize = 128 * 1024;
const RECORD_BYTES: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub(super) struct InteractionLog {
    memory: Vec<(usize, String)>,
    memory_bytes: usize,
    disk: Option<(File, Arc<HistoryArtifact>)>,
    count: usize,
    updates: std::collections::BTreeMap<usize,String>,
}
impl InteractionLog {
    pub(super) fn insert_at(&mut self,offset:usize,marker:String)->Result<usize>{
        let mut next=Self::default();let mut marker=Some(marker);let mut index=None;
        self.for_each(|at,text|{if at>offset{if let Some(text)=marker.take(){index=Some(next.len());next.push((offset,text))?;}}next.push((at,text.into()))})?;
        if let Some(text)=marker{index=Some(next.len());next.push((offset,text))?;}*self=next;Ok(index.unwrap())
    }
    pub(super) fn is_empty(&self) -> bool {
        self.count == 0
    }
    pub(super) fn len(&self)->usize{self.count}
    pub(super) fn update(&mut self,index:usize,marker:String)->Result<()>{
        ensure!(index<self.count && marker.len()<=RECORD_BYTES,"Invalid interaction update");
        self.updates.insert(index,marker);Ok(())
    }
    pub(super) fn push(&mut self, entry: (usize, String)) -> Result<()> {
        ensure!(
            entry.1.len() <= RECORD_BYTES,
            "Interaction record exceeds its presentation budget"
        );
        if self.disk.is_none()
            && self.memory_bytes.saturating_add(entry.1.len() + 16) > MEMORY_BYTES
        {
            let mut disk = Self::file()?;
            for (offset, marker) in &self.memory {
                Self::write(&mut disk.0, *offset, marker)?;
            }
            self.disk = Some(disk);
            self.memory = Vec::new();
            self.memory_bytes = 0;
        }
        if let Some((file, _)) = &mut self.disk {
            Self::write(file, entry.0, &entry.1)?;
        } else {
            self.memory_bytes += entry.1.len() + 16;
            self.memory.push(entry);
        }
        self.count += 1;
        Ok(())
    }
    fn file() -> Result<(File, Arc<HistoryArtifact>)> {
        let path =
            std::env::temp_dir().join(format!("code-response-cards-{}", uuid::Uuid::new_v4()));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)?;
        Ok((file, HistoryArtifact::own(path)))
    }
    fn write(file: &mut File, offset: usize, marker: &str) -> Result<()> {
        file.write_all(&(offset as u64).to_le_bytes())?;
        file.write_all(&(marker.len() as u32).to_le_bytes())?;
        for chunk in marker.as_bytes().chunks(64 * 1024) {
            file.write_all(chunk)?;
        }
        Ok(())
    }
    pub(super) fn for_each(&self, mut visit: impl FnMut(usize, &str) -> Result<()>) -> Result<()> {
        if let Some((_, artifact)) = &self.disk {
            let mut reader = BufReader::with_capacity(64 * 1024, File::open(&artifact.path)?);
            let mut header = [0_u8; 12];
            let mut marker = Vec::new();
            for index in 0..self.count {
                reader.read_exact(&mut header)?;
                let offset = usize::try_from(u64::from_le_bytes(header[..8].try_into()?))?;
                let len = u32::from_le_bytes(header[8..].try_into()?) as usize;
                ensure!(len <= RECORD_BYTES, "Invalid interaction spill record");
                marker.resize(len, 0);
                reader.read_exact(&mut marker)?;
                visit(offset, self.updates.get(&index).map(String::as_str).unwrap_or(std::str::from_utf8(&marker)?))?;
            }
        } else {
            for (index,(offset, marker)) in self.memory.iter().enumerate() {
                visit(*offset, self.updates.get(&index).unwrap_or(marker))?;
            }
        }
        Ok(())
    }
    pub(super) fn commit_updates(&mut self)->Result<()>{
        if self.updates.is_empty(){return Ok(())}
        let mut replacement=Self::default();
        self.for_each(|offset,marker|replacement.push((offset,marker.into())))?;
        *self=replacement;Ok(())
    }
    pub(super) fn reanchor(&mut self, start: usize, end: usize) -> Result<()> {
        if self.disk.is_some() {
            let mut replacement = Self::default();
            self.for_each(|offset, marker| {
                replacement.push((if offset > start { end } else { offset }, marker.into()))
            })?;
            *self = replacement;
        } else {
            for (offset, _) in &mut self.memory {
                if *offset > start {
                    *offset = end;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn updating_a_spilled_card_preserves_order_offsets_and_latest_text(){
        let mut log=InteractionLog::default();log.push((0,"first".into())).unwrap();log.push((4,"middle".repeat(30_000))).unwrap();log.push((8,"last".into())).unwrap();
        log.update(1,"new middle".into()).unwrap();log.reanchor(0,12).unwrap();let mut rows=vec![];log.for_each(|at,text|{rows.push((at,text.to_owned()));Ok(())}).unwrap();assert_eq!(rows,vec![(0,"first".into()),(12,"new middle".into()),(12,"last".into())]);assert_eq!(log.len(),3);
    }
    #[test]
    fn thousands_of_cards_spill_and_correct_without_retaining_all_markers() {
        let mut log = InteractionLog::default();
        let marker = "🦀status".repeat(70);
        for index in 0..10000 {
            log.push((index, marker.clone())).unwrap();
        }
        let path = log.disk.as_ref().unwrap().1.path.clone();
        assert!(log.memory.is_empty());
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        log.reanchor(500, 700).unwrap();
        let mut count = 0;
        log.for_each(|offset, text| {
            assert_eq!(offset, if count > 500 { 700 } else { count });
            assert_eq!(text, marker);
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 10000);
        drop(log);
        assert!(!path.exists());
    }
}

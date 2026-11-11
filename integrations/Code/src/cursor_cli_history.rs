//! Cursor's local CLI store is read-only here. The root lists message blobs in
//! conversation order. Tool artifacts and injected system context stay in Cursor.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;
use std::{collections::{HashMap, HashSet}, path::{Path, PathBuf}, sync::{Arc, Mutex}};

#[derive(Clone)]
pub(super) struct Sync {
    pub id: String,
    root: PathBuf,
    directory: PathBuf,
    seen: Arc<Mutex<HashSet<(String, usize)>>>,
}

impl Sync {
    pub fn start(home: &Path, directory: &Path, id: String) -> Result<Self> {
        ensure!(uuid::Uuid::parse_str(&id).is_ok(), "Invalid Cursor chat ID");
        let mut history = Self {
            id, root: home.join(".cursor/chats"), directory: directory.to_owned(),
            seen: Arc::new(Mutex::new(HashSet::new())),
        };
        history.seen = Arc::new(Mutex::new(history.read()?.into_iter().map(|(id, _, _)| id).collect()));
        Ok(history)
    }

    fn database(&self) -> Result<Option<PathBuf>> {
        let projects = match std::fs::read_dir(&self.root) {
            Ok(projects) => projects,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let mut matches = Vec::new();
        for project in projects.take(8192).flatten() {
            let chat = project.path().join(&self.id);
            let db = chat.join("store.db");
            if !db.is_file() { continue; }
            let cwd = std::fs::File::open(chat.join("meta.json")).ok()
                .and_then(|file| serde_json::from_reader::<_, Value>(std::io::Read::take(file, 64 * 1024)).ok())
                .and_then(|meta| meta["cwd"].as_str().map(PathBuf::from));
            if cwd.as_ref() == Some(&self.directory) { return Ok(Some(db)); }
            matches.push(db);
        }
        ensure!(matches.len() <= 1, "Cursor chat exists in multiple workspaces");
        Ok(matches.pop())
    }

    fn read(&self) -> Result<Vec<((String, usize), &'static str, String)>> {
        let Some(db) = self.database()? else { return Ok(Vec::new()); };
        let db = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.busy_timeout(std::time::Duration::from_millis(200))?;
        let mut statement = db.prepare("SELECT data FROM blobs WHERE id=?1 AND length(data)<=?2")?;
        let meta: Option<String> = db.query_row(
            "SELECT value FROM meta WHERE key='0' AND length(value)<=65536", [], |row| row.get(0)
        ).optional()?;
        let Some(meta) = meta else { return Ok(Vec::new()); };
        let meta = meta.as_bytes().chunks_exact(2).map(|pair| {
            let pair = std::str::from_utf8(pair)?;
            Ok(u8::from_str_radix(pair, 16)?)
        }).collect::<Result<Vec<_>>>()?;
        let meta: Value = serde_json::from_slice(&meta)?;
        let Some(root) = meta["latestRootBlobId"].as_str().filter(|id| !id.is_empty()) else { return Ok(Vec::new()); };
        let root: Vec<u8> = statement.query_row((root, 2_000_000), |row| row.get(0))?;
        let ids = message_ids(&root)?;
        let seen = self.seen.lock().unwrap();
        let mut output = Vec::new();
        let mut occurrences = HashMap::<&str, usize>::new();
        let mut total_bytes = 0;
        for id in &ids {
            let count = occurrences.entry(id).or_default();
            *count += 1;
            let key = (id.clone(), *count);
            if seen.contains(&key) { continue; }
            let data: Option<Vec<u8>> = statement.query_row((id, 1_000_000), |row| row.get(0)).optional()?;
            let Some(data) = data else { continue; };
            let Ok(message) = serde_json::from_slice::<Value>(&data) else { continue; };
            let role = match message["role"].as_str() {
                Some("user") if message["content"].is_array() => "user",
                Some("assistant") => "assistant:cursor",
                _ => continue,
            };
            let text = crate::runtime::content_text(&message["content"]);
            if !text.trim().is_empty() {
                total_bytes += text.len();
                ensure!(total_bytes <= 16_000_000, "Cursor transcript exceeds the import size limit");
                output.push((key, role, text));
            }
        }
        Ok(output)
    }

    pub fn import(&self, store: &crate::store::Store, session: &str) -> Result<()> {
        for (id, role, text) in self.read()? {
            if let Some(text) = crate::presentation::sanitize_transcript_text(role, &text) {
                store.append_cursor_cli_message(session, role, &text)?;
            }
            self.seen.lock().unwrap().insert(id);
        }
        Ok(())
    }
}

fn varint(bytes: &mut &[u8]) -> Result<usize> {
    let mut value = 0usize;
    for shift in (0..usize::BITS).step_by(7) {
        let (&byte, rest) = bytes.split_first().context("Truncated Cursor history")?;
        *bytes = rest;
        value |= ((byte & 127) as usize).checked_shl(shift).context("Invalid Cursor history")?;
        if byte < 128 { return Ok(value); }
    }
    anyhow::bail!("Invalid Cursor history");
}

fn message_ids(mut bytes: &[u8]) -> Result<Vec<String>> {
    use std::fmt::Write;
    let mut ids = Vec::new();
    while !bytes.is_empty() {
        let key = varint(&mut bytes)?;
        let length = match key & 7 {
            0 => { varint(&mut bytes)?; continue; }
            1 => 8,
            2 => varint(&mut bytes)?,
            5 => 4,
            _ => anyhow::bail!("Unsupported Cursor history field"),
        };
        let (field, rest) = bytes.split_at_checked(length).context("Truncated Cursor history field")?;
        bytes = rest;
        if key == 10 && length == 32 {
            let mut id = String::with_capacity(64);
            for byte in field { write!(id, "{byte:02x}")?; }
            ids.push(id);
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_new_messages_from_the_exact_cursor_chat_in_root_order() {
        struct Temporary(PathBuf);
        impl Drop for Temporary {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
        }
        let temporary = Temporary(std::env::temp_dir().join(format!("kai-cursor-test-{}", uuid::Uuid::new_v4())));
        let home = &temporary.0;
        let id = uuid::Uuid::new_v4().to_string();
        let directory = home.join(".cursor/chats/project").join(&id);
        std::fs::create_dir_all(&directory).unwrap();
        let db = Connection::open(directory.join("store.db")).unwrap();
        db.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT);
                          CREATE TABLE blobs(id TEXT PRIMARY KEY, data BLOB);").unwrap();
        let encode = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        db.execute("INSERT INTO meta VALUES('0',?1)", [encode(br#"{"latestRootBlobId":"root"}"#)]).unwrap();
        let write_message = |byte, message: &[u8]| {
            db.execute("INSERT INTO blobs VALUES(?1,?2)", (encode(&[byte; 32]), message)).unwrap();
        };
        let write_root = |messages: &[u8]| {
            let root: Vec<_> = messages.iter().flat_map(|byte| [10, 32].into_iter().chain([*byte; 32])).collect();
            db.execute("INSERT OR REPLACE INTO blobs VALUES('root',?1)", [root]).unwrap();
        };
        write_message(1, br#"{"role":"assistant","content":"Previously in the GUI"}"#);
        write_root(&[1]);
        let sync = Sync::start(home, home, id.clone()).unwrap();
        write_message(9, br#"{"role":"user","content":[{"type":"text","text":"CLI question"}]}"#);
        write_message(2, br#"{"role":"assistant","content":[{"type":"text","text":"CLI answer"}]}"#);
        write_root(&[1, 9, 2]);
        let rows: Vec<_> = sync.read().unwrap().into_iter().map(|(_, role, text)| (role, text)).collect();
        assert_eq!(rows, vec![("user", "CLI question".into()), ("assistant:cursor", "CLI answer".into())]);
        let store = crate::store::Store::open(Path::new(":memory:")).unwrap();
        let gui = crate::discovery::SessionSummary {
            id: "kai-tab".into(), title: "Cursor fixture".into(),
            agent: crate::discovery::AgentKind::Cursor, project_path: home.clone(),
            execution_path: None, updated_at: 0, external_id: Some("gui-acp-id".into()),
            transcript_path: None,
        };
        store.upsert_session(&gui).unwrap();
        store.bind_cursor_cli(&gui.id, &id).unwrap();
        sync.import(&store, &gui.id).unwrap();
        sync.import(&store, &gui.id).unwrap();
        assert_eq!(store.message_count(&gui.id).unwrap(), 2);
        assert_eq!(store.cursor_cli_id(&gui.id).unwrap().as_deref(), Some(id.as_str()));
        assert_eq!(store.session(&gui.id).unwrap().unwrap().external_id.as_deref(), Some("gui-acp-id"));
        let context = store.cursor_cli_context(&gui.id).unwrap().unwrap();
        assert_eq!(context, "User: CLI question\nAssistant: CLI answer\n");
        store.clear_cursor_cli_context(&gui.id).unwrap();
        assert_eq!(store.cursor_cli_context(&gui.id).unwrap(), None);
        assert!(message_ids(&[10, 32, 1]).is_err());
    }
}

//! Metadata-only indexes for other native CLIs. Never opens CLI credentials.
use super::*;

pub(super) fn sessions(home: &Path, project: Option<&Path>) -> Vec<SessionSummary> {
    let mut sessions = gemini_sessions(home, project);
    sessions.extend(opencode_sessions(home, project));
    sessions
}

fn small_json(path: &Path) -> Option<Value> {
    let file = File::open(path).ok()?;
    if file.metadata().ok()?.len() > 2 * 1024 * 1024 {
        return None;
    }
    // Keep the read bounded even if a native process grows this file after
    // the metadata check.
    serde_json::from_reader(file.take(2 * 1024 * 1024)).ok()
}

fn gemini_sessions(home: &Path, selected: Option<&Path>) -> Vec<SessionSummary> {
    let Some(index) = small_json(&home.join(".gemini/projects.json")) else {
        return Vec::new();
    };
    let Some(projects) = index["projects"].as_object() else {
        return Vec::new();
    };
    let mut result = Vec::new();
    let mut inspected = 0;
    let mut sampled = 0;
    for (root, slug) in projects.iter().take(MAX_PROJECT_ENTRIES) {
        let project = PathBuf::from(root);
        if !usable_project(&project) || selected.is_some_and(|path| !project.starts_with(path)) {
            continue;
        }
        let Some(slug) = slug.as_str() else {
            continue;
        };
        // Registry values name a directory, never an arbitrary file outside tmp.
        if slug.is_empty()
            || slug == "."
            || slug == ".."
            || slug.contains('/')
            || slug.contains('\\')
        {
            continue;
        }
        if inspected >= MAX_SESSION_ENTRIES
            || sampled >= MAX_LOG_SAMPLES
            || result.len() >= MAX_SESSIONS
        {
            break;
        }
        let entry_budget = (MAX_SESSION_ENTRIES - inspected).min(2_000);
        let mut paths: Vec<_> = WalkDir::new(home.join(".gemini/tmp").join(slug).join("chats"))
            .follow_links(false)
            .max_depth(1)
            .into_iter()
            .take(entry_budget)
            .inspect(|_| inspected += 1)
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_file()
                    && entry.path().extension().is_some_and(|ext| ext == "jsonl")
            })
            .map(|entry| {
                (
                    entry
                        .metadata()
                        .ok()
                        .and_then(|meta| meta.modified().ok())
                        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0),
                    entry.into_path(),
                )
            })
            .collect();
        paths.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        let project_limit = if selected.is_some() {
            1_000
        } else {
            MAX_SESSIONS_PER_PROJECT
        };
        let sample_budget = project_limit
            .min(MAX_LOG_SAMPLES - sampled)
            .min(MAX_SESSIONS - result.len());
        for (modified, path) in paths.into_iter().take(sample_budget) {
            sampled += 1;
            // Gemini's ID and first user prompt are at the beginning; unlike
            // Claude, it has no custom-title tail record to sample again.
            let Some(records) = sample_log_head(&path) else {
                continue;
            };
            let Some(id) = records
                .iter()
                .find_map(|record| record["sessionId"].as_str())
            else {
                continue;
            };
            if records.iter().any(|record| {
                record["kind"]
                    .as_str()
                    .is_some_and(|kind| kind == "subagent")
            }) {
                continue;
            }
            let title = records
                .iter()
                .filter(|record| record["type"] == "user")
                .find_map(|record| message_title(record.get("content")))
                .unwrap_or_else(|| "Gemini session".into());
            result.push(SessionSummary {
                id: format!("gemini:{id}"),
                title,
                agent: AgentKind::Gemini,
                project_path: project.clone(),
                execution_path: Some(project.clone()),
                updated_at: modified,
                external_id: Some(id.into()),
                transcript_path: Some(path),
            });
        }
        if result.len() >= MAX_SESSIONS {
            break;
        }
    }
    result
}

fn opencode_sessions(home: &Path, selected: Option<&Path>) -> Vec<SessionSummary> {
    let root = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));
    let path = root.join("opencode/opencode.db");
    if !path.is_file() {
        return Vec::new();
    }
    let Ok(connection) = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let _ = connection.busy_timeout(std::time::Duration::from_millis(100));
    // No joins or message bodies: native provider/session metadata remains bounded.
    let sql = if selected.is_some() {
        "SELECT id,directory,substr(title,1,4096),time_updated FROM session WHERE parent_id IS NULL AND time_archived IS NULL AND (directory = ?1 OR substr(directory,1,length(?1)+1) = ?1 || '/') ORDER BY time_updated DESC LIMIT 1000"
    } else {
        "SELECT id,directory,substr(title,1,4096),time_updated FROM session WHERE parent_id IS NULL AND time_archived IS NULL ORDER BY time_updated DESC LIMIT 2400"
    };
    let Ok(mut statement) = connection.prepare(sql) else {
        return Vec::new();
    };
    let mapper = |row: &rusqlite::Row<'_>| -> rusqlite::Result<SessionSummary> {
        let id: String = row.get(0)?;
        let project: PathBuf = PathBuf::from(row.get::<_, String>(1)?);
        let title: String = row.get(2)?;
        Ok(SessionSummary {
            id: format!("opencode:{id}"),
            title: clean_title(&title).unwrap_or_else(|| "OpenCode session".into()),
            agent: AgentKind::OpenCode,
            project_path: project.clone(),
            execution_path: Some(project),
            updated_at: row.get::<_, i64>(3)? / 1000,
            external_id: Some(id),
            transcript_path: Some(path.clone()),
        })
    };
    let rows = if let Some(project) = selected {
        statement.query_map([project.to_string_lossy().as_ref()], mapper)
    } else {
        statement.query_map([], mapper)
    };
    rows.map(|rows| {
        rows.filter_map(Result::ok)
            .filter(|session| usable_project(&session.project_path))
            .collect()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gemini_reads_bounded_metadata_and_keeps_resumable_source_path() {
        let root =
            std::env::temp_dir().join(format!("orbit-gemini-discovery-{}", uuid::Uuid::new_v4()));
        let project = root.join("project");
        let chats = root.join(".gemini/tmp/project/chats");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&chats).unwrap();
        fs::write(
            root.join(".gemini/projects.json"),
            serde_json::json!({"projects":{project.to_string_lossy().to_string():"project"}})
                .to_string(),
        )
        .unwrap();
        let path = chats.join("session-test.jsonl");
        fs::write(&path,"{\"sessionId\":\"gemini-1\",\"projectHash\":\"hash\",\"kind\":\"main\"}\n{\"type\":\"user\",\"content\":[{\"text\":\"Explain the code\"}]}\n").unwrap();
        let sessions = gemini_sessions(&root, Some(&project));
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title, "Explain the code");
        assert_eq!(sessions[0].external_id.as_deref(), Some("gemini-1"));
        assert_eq!(sessions[0].transcript_path.as_ref(), Some(&path));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opencode_metadata_query_skips_archives_subagents_and_other_projects() {
        let root =
            std::env::temp_dir().join(format!("orbit-opencode-discovery-{}", uuid::Uuid::new_v4()));
        let project = root.join("project");
        let other = root.join("other");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::create_dir_all(root.join(".local/share/opencode")).unwrap();
        let conn =
            rusqlite::Connection::open(root.join(".local/share/opencode/opencode.db")).unwrap();
        conn.execute_batch("CREATE TABLE session (id TEXT,directory TEXT,title TEXT,time_updated INTEGER,parent_id TEXT,time_archived INTEGER);").unwrap();
        for (id, dir, parent, archived) in [
            ("visible", &project, None, None),
            ("child", &project, Some("parent"), None),
            ("archived", &project, None, Some(1)),
            ("other", &other, None, None),
        ] {
            conn.execute(
                "INSERT INTO session VALUES (?1,?2,'Real title',100000,?3,?4)",
                rusqlite::params![id, dir.to_string_lossy(), parent, archived],
            )
            .unwrap();
        }
        let sessions = opencode_sessions(&root, Some(&project));
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].external_id.as_deref(), Some("visible"));
        assert_eq!(sessions[0].updated_at, 100);
        fs::remove_dir_all(root).unwrap();
    }
}

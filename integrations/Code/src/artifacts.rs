//! Persistent, bounded artifacts. Disk/SQLite and file decoding stay off UI.
use super::*;
use anyhow::{Context as AnyhowContext, ensure};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::io::Read;

fn database(directory: &std::path::Path) -> anyhow::Result<Connection> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(directory)?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    let path = directory.join("artifacts.sqlite");
    let db = Connection::open(&path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS artifacts(id TEXT PRIMARY KEY,project TEXT,chat TEXT,title TEXT,kind TEXT,body TEXT,revision INTEGER,updated INTEGER); CREATE INDEX IF NOT EXISTS artifact_scope ON artifacts(project,updated DESC);")?;
    Ok(db)
}
pub(super) fn command(
    directory: &std::path::Path,
    args: &Value,
    project: &std::path::Path,
) -> anyhow::Result<Value> {
    let mut db = database(directory)?;
    match args["action"].as_str().unwrap_or("") {
        "list_artifacts" => {
            let offset = args["offset"].as_u64().unwrap_or(0).min(100000) as i64;
            let mut stmt=db.prepare("SELECT id,chat,title,kind,revision,updated FROM artifacts WHERE project=?1 ORDER BY updated DESC,id LIMIT 51 OFFSET ?2")?;
            let mut rows=stmt.query_map(params![project.to_string_lossy(),offset],|r|Ok(json!({"id":r.get::<_,String>(0)?,"chat_id":r.get::<_,String>(1)?,"title":r.get::<_,String>(2)?,"kind":r.get::<_,String>(3)?,"revision":r.get::<_,i64>(4)?,"updated_at":r.get::<_,i64>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let next = (rows.len() > 50).then_some(offset + 50);
            rows.truncate(50);
            Ok(json!({"artifacts":rows,"next_offset":next}))
        }
        "read_artifact" => {
            let id = args["id"].as_str().context("Missing artifact ID")?;
            db.query_row("SELECT title,kind,body,revision,chat FROM artifacts WHERE id=?1 AND project=?2",params![id,project.to_string_lossy()],|r|Ok(json!({"id":id,"title":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"content":r.get::<_,String>(2)?,"revision":r.get::<_,i64>(3)?,"chat_id":r.get::<_,String>(4)?}))).context("Artifact is absent from this project")
        }
        "write_artifact" => {
            let body = args["content"].as_str().context("Missing content")?;
            ensure!(body.len() <= 512 * 1024, "Artifact exceeds 512 KiB");
            let kind = args["kind"].as_str().unwrap_or("markdown");
            ensure!(
                ["markdown", "html", "text", "mermaid"].contains(&kind),
                "Unsupported artifact kind"
            );
            let title = args["name"].as_str().unwrap_or("Document");
            ensure!(preferences::valid_name(title), "Invalid title");
            let id = args["id"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            ensure!(uuid::Uuid::parse_str(&id).is_ok(), "Use a UUID artifact ID");
            let tx=db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let existing: Option<String> = tx
                .query_row("SELECT project FROM artifacts WHERE id=?1", [&id], |r| {
                    r.get(0)
                })
                .optional()?;
            ensure!(
                existing
                    .as_deref()
                    .is_none_or(|p| p == project.to_string_lossy()),
                "Artifact belongs to another project"
            );
            let count: i64 = tx.query_row("SELECT count(*) FROM artifacts", [], |r| r.get(0))?;
            ensure!(
                existing.is_some() || count < 4096,
                "Artifact capacity reached"
            );
            tx.execute("INSERT INTO artifacts VALUES(?1,?2,?3,?4,?5,?6,1,?7) ON CONFLICT(id) DO UPDATE SET title=excluded.title,kind=excluded.kind,body=excluded.body,revision=revision+1,updated=excluded.updated",params![id,project.to_string_lossy(),args["chat_id"].as_str().unwrap_or(""),title,kind,body,chrono::Utc::now().timestamp()])?;
            tx.commit()?;
            Ok(
                json!({"id":id,"title":title,"kind":kind,"saved":true,"open_url":format!("kai://artifact/{id}")}),
            )
        }
        _ => anyhow::bail!("Unknown artifact action"),
    }
}
use rusqlite::OptionalExtension;
fn execution_workspace(project:&std::path::Path,session:Option<&SessionSummary>)->Result<PathBuf,String>{
    if let Some(session)=session{
        if session.project_path!=project{return Err("The chat belongs to a different project".into())}
        return Ok(session.execution_path.clone().unwrap_or_else(||project.to_path_buf()));
    }
    Ok(project.to_path_buf())
}
pub(super) fn scoped_file(
    project: &std::path::Path,
    path: &std::path::Path,
    offset: u64,
) -> anyhow::Result<Value> {
    let root = project.canonicalize()?;
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
    .canonicalize()?;
    ensure!(
        path.starts_with(&root),
        "File is outside the granted workspace"
    );
    ensure!(path.is_file(), "Choose a regular file");
    let mut file = file_context::open_file(&root, &path)?;
    let total = file.metadata()?.len();
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset.min(total)))?;
    let mut bytes = vec![];
    file.take(256 * 1024).read_to_end(&mut bytes)?;
    let next = offset.saturating_add(bytes.len() as u64);
    let encoded = base64_bytes(&bytes);
    Ok(
        json!({"path":path,"offset":offset,"total_bytes":total,"base64":encoded,"next_offset":(next<total).then_some(next)}),
    )
}
pub(super) fn base64_bytes(bytes: &[u8]) -> String {
    const C: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(C[((n >> 18) & 63) as usize] as char);
        out.push(C[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            C[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            C[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
impl Orbit {
    pub(super) fn artifact_command(
        &mut self,
        args: &Value,
        grant: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Option<Value>, String> {
        let project = args["project"]
            .as_str()
            .map(PathBuf::from)
            .ok_or("Select the artifact's project")?;
        if !self.projects.iter().any(|p| p.path == project) {
            return Err("Unknown project".into());
        }
        if grant["projects"].as_array().is_some_and(|roots| {
            !roots.is_empty() && !roots.iter().any(|p| p.as_str() == project.to_str())
        }) {
            return Err("Project is outside this grant".into());
        }
        if args["action"] == "write_artifact" && grant["scope"] != "control" {
            return Err("Read-only client".into());
        }
        let directory = self.workflow_dir.join("artifacts");
        let args = args.clone();
        let token = args["_remote_id"].as_str().unwrap_or("").to_string();
        let owner = args["_owner"].as_str().unwrap_or("").to_string();
        cx.spawn(async move |this, cx| {
            let value = cx
                .background_executor()
                .spawn(async move {
                    command(&directory, &args, &project)
                        .unwrap_or_else(|e| json!({"error":e.to_string()}))
                })
                .await;
            let _ = this.update(cx, |app, _| {
                if owner.is_empty() {
                    app.remote_direct_reply(&token, value)
                } else {
                    app.workspace_artifact_reply(&owner, &token, value)
                }
            });
        })
        .detach();
        Ok(None)
    }
    pub(super) fn remote_file(
        &mut self,
        args: &Value,
        grant: &Value,
        session:Option<&SessionSummary>,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Result<Option<Value>, String> {
        let project = PathBuf::from(args["project"].as_str().ok_or("Select a project")?);
        let path = PathBuf::from(args["path"].as_str().ok_or("Select a path")?);
        if !self.projects.iter().any(|p| p.path == project)
            || grant["projects"].as_array().is_some_and(|roots| {
                !roots.is_empty() && !roots.iter().any(|p| p.as_str() == project.to_str())
            })
        {
            return Err("Project is outside this grant".into());
        }
        let offset = args["offset"].as_u64().unwrap_or(0);
        let workspace=execution_workspace(&project,session)?;
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let value = cx
                .background_executor()
                .spawn(async move {
                    scoped_file(&workspace, &path, offset)
                        .unwrap_or_else(|e| json!({"error":e.to_string()}))
                })
                .await;
            let _ = this.update(cx, |app, _| app.remote_direct_reply(&id, value));
        })
        .detach();
        Ok(None)
    }
    pub(super) fn remote_review(
        &mut self,
        args: &Value,
        session: Option<&SessionSummary>,
        id: &str,
        cx: &mut Context<Self>,
    ) -> Result<Option<Value>, String> {
        let session = session.ok_or("Select a chat")?;
        let repository = self
            .checkpoint_repository
            .clone()
            .map_err(|e| e.to_string())?;
        let chat = session.id.clone();
        let id = id.to_owned();
        let args = args.clone();
        let restore = args["action"] == "restore";
        if restore
            && (!self.runs.is_empty() || !self.launches.is_empty() || self.voice_call.is_some())
        {
            return Err("Finish active work before restoring workspace files".into());
        }
        cx.spawn(async move|this,cx|{let value=cx.background_executor().spawn(async move{
            (||->anyhow::Result<Value>{
                let offset=args["offset"].as_u64().unwrap_or(0).min(100000)as usize;
                if let Some(checkpoint)=args["checkpoint_id"].as_str(){
                    let entry=repository.get(checkpoint)?;ensure!(entry.session==chat,"Checkpoint belongs to another chat");
                    let direction=if args["direction"]=="redo"{checkpoints::Direction::Redo}else{checkpoints::Direction::Undo};
                    if restore{ensure!(matches!(args["direction"].as_str(),Some("undo"|"redo")),"Choose undo or redo");repository.apply(checkpoint,direction,&std::sync::atomic::AtomicBool::new(false))?;return Ok(json!({"restored":true,"checkpoint_id":checkpoint,"direction":args["direction"],"scope":"workspace files; conversation and external effects preserved"}))}
                    if let Some(path)=args["path"].as_str(){let p=&args["position"];let page=repository.diff_page(checkpoint,std::path::Path::new(path),checkpoint_diff::Position{before:p["before"].as_u64().unwrap_or(0),after:p["after"].as_u64().unwrap_or(0),old_line:p["old_line"].as_u64().unwrap_or(1),new_line:p["new_line"].as_u64().unwrap_or(1)},&std::sync::atomic::AtomicBool::new(false))?;
                        return Ok(json!({"checkpoint_id":checkpoint,"path":path,"lines":page.lines.iter().map(|l|json!({"kind":format!("{:?}",l.kind),"text":l.text})).collect::<Vec<_>>(),"next_position":page.next.map(|p|json!({"before":p.before,"after":p.after,"old_line":p.old_line,"new_line":p.new_line}))}))}
                    let preview=repository.preview(checkpoint,if entry.undone{checkpoints::Direction::Redo}else{checkpoints::Direction::Undo},offset)?;
                    return Ok(json!({"checkpoint_id":checkpoint,"changes":preview.changes.iter().map(|c|json!({"path":c.path,"kind":c.kind})).collect::<Vec<_>>(),"total":preview.total,"next_offset":(offset+preview.changes.len()<preview.total).then_some(offset+preview.changes.len())}))
                }
                ensure!(!restore,"Select an exact checkpoint");let entries=repository.list(&chat,offset)?;
                Ok(json!({"checkpoints":entries.iter().map(|p|json!({"id":p.id,"title":p.title,"phase":p.phase,"undone":p.undone,"created_at":p.created})).collect::<Vec<_>>(),"next_offset":(entries.len()==checkpoints::PAGE).then_some(offset+entries.len())}))
            })().unwrap_or_else(|e|json!({"error":e.to_string()}))
        }).await;let _=this.update(cx,|app,cx|{app.remote_direct_reply(&id,value);if restore{app.engine.refresh();cx.notify();}});}).detach();
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn persistence_revisions_scope_and_size_are_enforced() {
        let root = std::env::temp_dir().join(format!("kai-artifact-{}", uuid::Uuid::new_v4()));
        let project = PathBuf::from("/project");
        let id = uuid::Uuid::new_v4().to_string();
        let mut args = json!({"action":"write_artifact","id":id,"name":"Plan","kind":"html","content":"<h1>Hello</h1>"});
        assert_eq!(command(&root, &args, &project).unwrap()["saved"], true);
        args["content"] = json!("Revision");
        command(&root, &args, &project).unwrap();
        let read = command(&root, &json!({"action":"read_artifact","id":id}), &project).unwrap();
        assert_eq!(read["revision"], 2);
        assert_eq!(read["content"], "Revision");
        assert!(command(&root, &args, std::path::Path::new("/other")).is_err());
        args["content"] = json!("x".repeat(512 * 1024 + 1));
        assert!(command(&root, &args, &project).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn file_pages_are_bounded_and_symlinks_cannot_escape() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("kai-file-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a"), b"abc").unwrap();
        assert_eq!(
            scoped_file(&root, &root.join("a"), 0).unwrap()["base64"],
            "YWJj"
        );
        symlink("/etc/passwd", root.join("escape")).unwrap();
        assert!(scoped_file(&root, &root.join("escape"), 0).is_err());
        let worktree=root.join("worktree");std::fs::create_dir_all(&worktree).unwrap();std::fs::write(worktree.join("a"),b"worktree").unwrap();
        let session=SessionSummary{id:"worktree-chat".into(),title:"Worktree".into(),agent:AgentKind::Codex,project_path:root.clone(),execution_path:Some(worktree.clone()),updated_at:0,external_id:None,transcript_path:None};
        let workspace=execution_workspace(&root,Some(&session)).unwrap();
        assert_eq!(scoped_file(&workspace,std::path::Path::new("a"),0).unwrap()["base64"],"d29ya3RyZWU=");
        assert!(scoped_file(&workspace,&root.join("a"),0).is_err(),"Selecting a chat confines reads to its execution workspace");
        assert!(execution_workspace(std::path::Path::new("/unrelated"),Some(&session)).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}

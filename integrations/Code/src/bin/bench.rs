//! Reproducible state-engine workloads, isolated from UI rendering and agent inference.
//! Run: cargo run --release --bin bench
#![allow(dead_code)]
#[path = "../chat_interactions.rs"]
mod chat_interactions;
#[path = "../discovery.rs"]
mod discovery;
#[path = "../handoff.rs"]
mod handoff;
#[path = "../history_artifact.rs"]
mod history_artifact;
#[path = "../presentation.rs"]
mod presentation;
#[path = "../process_env.rs"]
mod process_env;
#[path = "../response_buffer.rs"]
mod response_buffer;
// The benchmark needs protocol display types, without starting agent runtimes.
#[path = "../runtime/native_requests.rs"]
mod runtime;
#[path = "../store.rs"]
mod store;
#[path = "../usage.rs"]
mod usage;

use anyhow::{Context, Result, ensure};
use discovery::{AgentKind, SessionSummary};
use rusqlite::{Connection, params};
use serde_json::json;
use std::{hint::black_box, path::PathBuf, time::Instant};
use store::Store;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn main() -> Result<()> {
    ensure!(
        !cfg!(debug_assertions),
        "Run this harness in release mode: cargo run --release --bin bench"
    );
    let scratch =
        Scratch(std::env::temp_dir().join(format!("orbit-bench-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&scratch.0)?;
    let path = scratch.0.join("fixture.sqlite3");
    let start = Instant::now();
    let store = Store::open(&path)?;
    let open_ms = millis(start);
    let fixture: Vec<SessionSummary> = (0..10_000)
        .map(|index| SessionSummary {
            id: format!("fixture-{index:05}"),
            title: format!("Session {index:05} renderer"),
            agent: if index % 2 == 0 {
                AgentKind::Codex
            } else {
                AgentKind::Claude
            },
            project_path: PathBuf::from(format!("/fixture/project-{:03}", index % 100)),
            updated_at: 1_700_000_000 + index,
            external_id: None,
            transcript_path: None,
            execution_path: None,
        })
        .collect();

    // Exercise the application's actual upsert API. Each call is an implicit WAL
    // transaction; do not describe these timings as a bulk transaction benchmark.
    let start = Instant::now();
    for session in &fixture {
        store.upsert_session(black_box(session))?;
    }
    let insert_10k_ms = millis(start);
    let start = Instant::now();
    for session in &fixture {
        store.upsert_session(black_box(session))?;
    }
    let update_10k_ms = millis(start);

    let discovery_store = Store::open(&scratch.0.join("discovery.sqlite3"))?;
    let start = Instant::now();
    discovery_store.upsert_sessions(black_box(&fixture))?;
    let discovery_batch_10k_ms = millis(start);
    let start = Instant::now();
    discovery_store.upsert_sessions(black_box(&fixture))?;
    let unchanged_batch_10k_ms = millis(start);

    let mut connection = Connection::open(&path)?;
    let count: i64 = connection.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
    ensure!(count == 10_000, "Session fixture count mismatch");
    // Deterministic FTS fixture without save_message's wall-clock metadata update.
    {
        let transaction = connection.transaction()?;
        let mut statement = transaction
            .prepare("INSERT INTO messages(session_id,role,text) VALUES (?1,'assistant',?2)")?;
        for index in 0..300 {
            statement.execute(params![format!("fixture-{index:05}"), format!("Searchable transcript body {index:05}; SQLite persistence supports long conversations")])?;
        }
        drop(statement);
        transaction.commit()?;
    }
    // The Store sidebar API currently loads up to 6,000 rows. Measure it truthfully.
    let start = Instant::now();
    let summaries = black_box(store.sessions()?);
    let sidebar_6000_ms = millis(start);
    ensure!(summaries.len() == 6_000, "Sidebar fixture count mismatch");

    // Separately measure an indexed 300-summary SQL page on the same app schema.
    let start = Instant::now();
    let recent: Vec<SessionSummary> = {
        let mut query = connection.prepare("SELECT id,title,agent,project_path,updated_at,external_id,transcript_path,execution_path FROM sessions ORDER BY updated_at DESC,id LIMIT 300")?;
        query
            .query_map([], |row| {
                let command: String = row.get(2)?;
                Ok(SessionSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    agent: if command == "codex" {
                        AgentKind::Codex
                    } else {
                        AgentKind::Claude
                    },
                    project_path: PathBuf::from(row.get::<_, String>(3)?),
                    updated_at: row.get(4)?,
                    external_id: row.get(5)?,
                    transcript_path: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
                    execution_path: row.get::<_, Option<String>>(7)?.map(PathBuf::from),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let recent_300_ms = millis(start);
    ensure!(
        recent.len() == 300 && recent[0].id == "fixture-09999",
        "Recent page ordering mismatch"
    );
    black_box(recent);

    let start = Instant::now();
    let title_search = store.search(black_box("Session 00042"))?;
    let title_search_ms = millis(start);
    ensure!(
        title_search.len() == 1 && title_search[0].id == "fixture-00042",
        "Title search fixture mismatch"
    );
    let start = Instant::now();
    let body_search = store.search(black_box("SQLite persistence"))?;
    let fts_search_ms = millis(start);
    ensure!(body_search.len() == 100, "FTS result limit mismatch");

    const MARKDOWN_BYTES: usize = 4 * 1024 * 1024;
    let sample = "# Render architecture\n\nA responsive interface keeps expensive work outside the frame. 🦀\n\n```rust\nfn update(delta: &str) {\n    println!(\"{delta}\");\n}\n```\n\n- Virtualize visible rows\n- Coalesce streamed deltas\n\n> Preserve native input and session state.\n\n";
    let mut markdown = sample.repeat(MARKDOWN_BYTES / sample.len() + 1);
    let mut boundary = MARKDOWN_BYTES;
    while !markdown.is_char_boundary(boundary) {
        boundary -= 1;
    }
    markdown.truncate(boundary);
    let start = Instant::now();
    let rows = presentation::blocks(black_box(&markdown));
    let markdown_4mib_ms = millis(start);
    let row_count = rows.len();
    black_box(rows);

    // One long, heavily styled paragraph exercises cross-row inline spans.
    let dense = "**bold 🦀** *italic* `code` [link](https://example.com) ".repeat(10_000);
    let start = Instant::now();
    let dense_rows = presentation::blocks(black_box(&dense));
    let dense_markdown_ms = millis(start);
    let dense_row_count = dense_rows.len();
    black_box(dense_rows);

    let journal_mode: String = connection.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let database_bytes = std::fs::metadata(&path)
        .context("Read temporary benchmark database size")?
        .len();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "benchmark": "orbit-state-engine-v2",
            "build": "release",
            "os": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "session_count": count,
            "project_count": 100,
            "seeded_messages": 300,
            "journal_mode": journal_mode,
            "database_bytes_before_checkpoint": database_bytes,
            "workloads": {
                "open_and_schema_ms": open_ms,
                "insert_10000_implicit_transactions_ms": insert_10k_ms,
                "upsert_10000_implicit_transactions_ms": update_10k_ms,
                "discovery_10000_batch_insert_ms": discovery_batch_10k_ms,
                "discovery_10000_unchanged_batch_ms": unchanged_batch_10k_ms,
                "store_sidebar_6000_rows_ms": sidebar_6000_ms,
                "sql_recent_300_rows_ms": recent_300_ms,
                "store_title_search_ms": title_search_ms,
                "store_fts_search_ms": fts_search_ms,
                "markdown_parse_ms": markdown_4mib_ms,
                "markdown_bytes": markdown.len(),
                "markdown_rows": row_count,
                "dense_markdown_parse_ms": dense_markdown_ms,
                "dense_markdown_bytes": dense.len(),
                "dense_markdown_rows": dense_row_count
            },
            "scope": "Single-run state-engine workloads; not startup, input latency, memory, scrolling, FPS, or agent inference measurements."
        }))?
    );
    drop(connection);
    drop(store);
    Ok(())
}

fn millis(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.
}

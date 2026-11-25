use super::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)] pub provider:Option<AgentKind>,
    #[serde(default)] pub model:Option<String>,
    #[serde(default)] pub effort:Option<String>,
    #[serde(default)] pub workspace:Option<PathBuf>,
    pub key: String,
    pub session: String,
    pub checkpoint: Option<String>,
    pub submitted_ms: i64,
    pub first_output_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub elapsed_ms: Option<u64>,
    pub outcome: Option<String>,
}
pub struct Launch {
    pub caption: &'static str,
    pub preview: Vec<Row>,
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub started: Instant,
    pub key: String,
}
pub struct Live {
    pub started: Instant,
    pub key: String,
}
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{}h {:02}m", s / 3600, s / 60 % 60)
    }
}
impl Orbit {
    pub(super) fn set_turn_provenance(&mut self,key:&str,request:&runtime::RunRequest){if let Some(record)=self.turn_timings.iter_mut().rev().find(|r|r.key==key){record.provider=Some(request.agent);record.model=request.model.clone();record.effort=request.reasoning_effort.clone();record.workspace=Some(request.project.clone());}}
    pub(super) fn starting_current(&self) -> bool {
        if self.launches.is_empty() {
            self.starting
        } else {
            self.active
                .as_ref()
                .is_some_and(|id| self.launches.contains_key(id))
        }
    }
    pub(super) fn launch_caption(&self) -> &str {
        self.active
            .as_ref()
            .and_then(|id| self.launches.get(id))
            .map_or("Preparing workspace…", |l| l.caption)
    }
    pub(super) fn begin_turn_clock(
        &mut self,
        id: &str,
        key: &str,
        started: Instant,
        cx: &mut Context<Self>,
    ) {
        if self.turn_timings.len() >= 1024 {
            self.turn_timings.remove(0);
        }
        let source=self.sessions.iter().find(|s|s.id==id);
        self.turn_timings.push(Record {
            provider:source.map(|s|s.agent),model:self.session_models.get(id).cloned().flatten(),effort:self.usage.get(id).and_then(|u|u.reasoning_effort.clone()),workspace:source.map(|s|s.execution_path.clone().unwrap_or_else(||s.project_path.clone())),
            key: key.into(),
            session: id.into(),
            checkpoint: None,
            submitted_ms: chrono::Utc::now().timestamp_millis()
                - started.elapsed().as_millis() as i64,
            first_output_ms: None,
            finished_ms: None,
            elapsed_ms: None,
            outcome: None,
        });
        self.activity.prompted(id, self.turn_timings.last().unwrap().submitted_ms);
        self.rebuild_workspace_sessions();
        self.queue_workspace_save(cx);
        let id = id.to_owned();
        let key = key.to_owned();
        cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_secs(1)).await;
                let keep = this
                    .update(cx, |app, cx| {
                        let active = app.launches.get(&id).is_some_and(|l| l.key == key)
                            || app.team_starting.contains(&id)
                            || app
                                .runs
                                .get(&id)
                                .and_then(|r| r.timing.as_ref())
                                .is_some_and(|t| t.key == key);
                        if active && app.active.as_ref() == Some(&id) {
                            cx.notify();
                        }
                        active
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }
    pub(super) fn finish_turn_clock(
        &mut self,
        key: &str,
        started: Instant,
        outcome: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(t) = self
            .turn_timings
            .iter_mut()
            .rev()
            .find(|t| t.key == key && t.finished_ms.is_none())
        {
            t.finished_ms = Some(chrono::Utc::now().timestamp_millis());
            t.elapsed_ms = Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
            t.outcome = Some(outcome.into());
        }
        self.queue_workspace_save(cx);
    }
    pub(super) fn restore_launch_preview(&mut self, id: &str) {
        if let Some(launch) = self.launches.get(id) {
            self.rows.extend(launch.preview.clone());
            self.pending_response_start = Some(self.rows.len());
            return;
        }
        if let (Some(run), Some(checkpoint)) = (self.runs.get(id), self.checkpoint_runs.get(id)) {
            if run.timing.is_some()
                && !self
                    .rows
                    .iter()
                    .any(|r| checkpoint_ui::row_checkpoint(r) == Some(checkpoint))
            {
                self.rows.push(Row {
                    role: Some("user".into()),
                    block: MessageBlock::Paragraph(run.submitted_prompt.clone()),
                });
                for attachment in &run.submitted_attachments {
                    self.rows.push(Row {
                        role: None,
                        block: MessageBlock::Attachment {
                            name: attachment.name.clone(),
                            target: attachment.path.to_string_lossy().into_owned(),
                        },
                    });
                }
                self.rows.push(Row {
                    role: None,
                    block: MessageBlock::Activity {
                        label: "Prompt changes".into(),
                        details: checkpoint.clone(),
                    },
                });
                self.pending_response_start = Some(self.rows.len());
            }
        }
    }
    pub(super) fn elapsed_label(&self) -> Option<String> {
        let id = self.active.as_ref()?;
        let started = self
            .launches
            .get(id)
            .map(|l| l.started)
            .or_else(|| self.runs.get(id)?.timing.as_ref().map(|t| t.started))?;
        Some(duration(started.elapsed().as_millis() as u64))
    }
    pub(super) fn row_timing(&self, ix: usize) -> Option<&Record> {
        if self.turn_timings.is_empty() {
            return None;
        }
        if self.rows.get(ix).is_some_and(|r| {
            matches!(
                &r.block,
                MessageBlock::Voice { .. }
                    | MessageBlock::Interaction(chat_interactions::ChatInteraction::Speaker { .. })
            )
        }) {
            return None;
        }
        let session = self.active.as_ref()?;
        let start = (0..=ix.min(self.rows.len().saturating_sub(1)))
            .rev()
            .find(|i| self.rows[*i].role.as_deref() == Some("user"))?;
        let end = (start + 1..self.rows.len())
            .find(|i| self.rows[*i].role.as_deref() == Some("user"))
            .unwrap_or(self.rows.len());
        let checkpoint = self.rows[start..end]
            .iter()
            .find_map(checkpoint_ui::row_checkpoint);
        if checkpoint.is_none() && end == self.rows.len() {
            let key = self
                .launches
                .get(session)
                .map(|l| l.key.as_str())
                .or_else(|| {
                    self.runs
                        .get(session)?
                        .timing
                        .as_ref()
                        .map(|t| t.key.as_str())
                });
            if let Some(key) = key {
                return self.turn_timings.iter().rev().find(|t| t.key == key);
            }
        }
        self.turn_timings.iter().rev().find(|t| {
            &t.session == session && checkpoint.is_some_and(|c| t.checkpoint.as_deref() == Some(c))
        })
    }
    pub(super) fn timestamp_opacity(&self) -> f32 {
        self.timestamp_released.map_or(self.timestamp_peek, |at| {
            self.timestamp_peek * (1. - (at.elapsed().as_secs_f32() / 0.18).min(1.)).powi(3)
        })
    }
    pub(super) fn peek_timestamps(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if matches!(event.touch_phase, TouchPhase::Ended) {
            if self.timestamp_peek > 0. {
                self.timestamp_released = Some(Instant::now());
                cx.notify();
            }
            self.timestamp_direction = 0.;
            return;
        }
        let d = event.delta.pixel_delta(px(18.));
        let x = f32::from(d.x);
        let y = f32::from(d.y);
        if x.abs() <= y.abs() || x.abs() < 0.1 {
            return;
        }
        if self.timestamp_direction == 0. {
            self.timestamp_direction = x.signum();
        }
        self.timestamp_released = None;
        self.timestamp_peek =
            (self.timestamp_peek + x * self.timestamp_direction / 80.).clamp(0., 1.);
        cx.stop_propagation();
        cx.notify();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn duration_is_compact_and_keeps_total_minutes() {
        assert_eq!(duration(12_999), "12s");
        assert_eq!(duration(123_000), "2m 03s");
        assert_eq!(duration(3_661_000), "1h 01m");
    }
}

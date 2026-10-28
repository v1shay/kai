//! Native GPUI layout/paint and input integration tests, isolated from real agents.
use super::*;
// GPUI's glob import in the parent exposes its test macro; keep generated #[test]
// attributes bound to Rust's built-in test attribute instead of recursively expanding.
use gpui::{TestAppContext, VisualTestContext};
use std::prelude::v1::test;

#[gpui::test]
fn delayed_history_metadata_cannot_overwrite_live_reported_usage(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        let id = app.sessions[0].id.clone();
        app.active = Some(id.clone());
        app.runs.insert(id.clone(), idle_run(String::new()));
        let current = usage::Usage {
            input: Some(200),
            ..Default::default()
        };
        app.remember_usage(id.clone(), current);
        let old = usage::Usage {
            input: Some(10),
            ..Default::default()
        };
        app.receive(
            Event::Model(id.clone(), app.generation, None, Some(Box::new(old)), None),
            cx,
        );
        assert_eq!(app.usage[&id].input, Some(200));
        app.runs.remove(&id);
        app.broker_children.insert(id.clone(), 1);
        app.receive(
            Event::Model(
                id.clone(),
                app.generation,
                None,
                Some(Box::new(usage::Usage {
                    input: Some(12),
                    ..Default::default()
                })),
                None,
            ),
            cx,
        );
        assert_eq!(app.usage[&id].input, Some(200));
    });
}

pub(super) fn fixture(cx: &mut TestAppContext) -> (Entity<Orbit>, &mut VisualTestContext) {
    let scratch = std::env::temp_dir().join(format!("orbit-ui-test-{}", uuid::Uuid::new_v4()));
    let home = scratch.join("home");
    let root = scratch.join("workspace");
    let data = scratch.join("data");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let cleanup = scratch.clone();
    cx.on_quit(move || {
        let _ = std::fs::remove_dir_all(cleanup);
    });
    cx.update(bind_app_keys);
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let mut app = Orbit::with_paths(window, cx, home, root.clone(), data);
        assert!(
            !app.sidebar_split,
            "A fresh app defaults to the unified project tree"
        );
        // Existing two-pane tests opt into that layout; unified-tree behavior has
        // separate native tests, while production defaults remain unified.
        app.sidebar_split = true;
        app.sidebar_width = 200.;
        app.motion = false;
        app.discovering = false;
        app.agents = [
            AgentKind::Codex,
            AgentKind::Claude,
            AgentKind::Gemini,
            AgentKind::OpenCode,
        ]
        .into_iter()
        .map(|kind| Agent {
            kind,
            name: agent_name(kind).to_owned(),
            executable: None,
            available: false,
        })
        .collect();
        app.projects = vec![Project {
            name: "Fixture workspace".into(),
            path: root.clone(),
        }];
        app.sessions = vec![SessionSummary {
            id: "fixture-session".into(),
            title: "Fixture conversation".into(),
            agent: AgentKind::Codex,
            project_path: root,
            updated_at: 1_700_000_000,
            external_id: None,
            transcript_path: None,
            execution_path: None,
        }];
        app.visible_sessions = app.sessions.clone();
        app.rebuild_workspace_sessions();
        app
    });
    cx.simulate_resize(size(px(1180.), px(790.)));
    cx.update(|window, cx| {
        window.activate_window();
        view.read(cx).composer.read(cx).focus_handle.focus(window);
    });
    draw_frame(&view, cx);
    (view, cx)
}

pub(super) fn draw_frame(view: &Entity<Orbit>, cx: &mut VisualTestContext) {
    cx.draw(point(px(0.), px(0.)), size(px(1180.), px(790.)), |_, _| {
        view.clone().into_any_element()
    });
}

#[gpui::test]
fn chat_flow_aligns_user_bubbles_and_groups_commands_with_reopenable_finished_work(cx:&mut TestAppContext){
    let(view,cx)=fixture(cx);
    view.update(cx,|app,cx|{
        app.loading=false;app.active=Some("fixture-session".into());app.rows=vec![
            Row{role:Some("user".into()),block:MessageBlock::Paragraph("Hello".into())},
            Row{role:Some("assistant:codex".into()),block:MessageBlock::Paragraph("I will check the files".into())},
            Row{role:Some("continuation:codex".into()),block:MessageBlock::Activity{label:"Terminal command".into(),details:"pwd".into()}},
            Row{role:Some("continuation:codex".into()),block:MessageBlock::Activity{label:"Terminal command".into(),details:"ls".into()}},
            Row{role:Some("continuation:codex".into()),block:MessageBlock::Paragraph("Here is the final answer".into())},
        ];app.list.reset(app.rows.len());cx.notify();
    });
    draw_frame(&view,cx);
    let user=cx.debug_bounds("transcript-content-0").unwrap();let assistant=cx.debug_bounds("transcript-content-1").unwrap();
    assert!(user.left()>assistant.left(),"User bubble did not anchor to the opposite edge");
    assert!(cx.debug_bounds("interaction-2").is_some());assert!(cx.debug_bounds("interaction-3").is_none(),"Each command rendered a separate row");
    view.update(cx,|app,cx|{app.rows.push(Row{role:Some("continuation:codex".into()),block:MessageBlock::Interaction(chat_interactions::ChatInteraction::Status{title:"Completed".into(),details:String::new(),status:chat_interactions::InteractionStatus::Completed})});app.list.reset(app.rows.len());cx.notify();});
    draw_frame(&view,cx);assert!(cx.debug_bounds("work-summary-1").is_some());assert!(cx.debug_bounds("transcript-content-4").is_some());
    // GPUI retains old debug selectors across frames; use measured list rows.
    cx.update(|_,cx|{let app=view.read(cx);assert_eq!(app.work_reveal(1),0.);assert!(app.list.bounds_for_item(2).is_none_or(|b|b.size.height==px(0.)),"Collapsed command still occupies the transcript");});
    click_selector(cx,"work-summary-1");draw_frame(&view,cx);cx.update(|_,cx|assert_eq!(view.read(cx).work_reveal(1),1.));assert!(cx.debug_bounds("interaction-2").is_some(),"Finished work could not be reopened");
}

fn freeze_list_entrances(view: &Entity<Orbit>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.tick_list_reveals(window, cx);
            for reveal in [
                &mut app.project_reveal,
                &mut app.sessions_reveal,
                &mut app.transcript_reveal,
            ]
            .into_iter()
            .flatten()
            {
                reveal.test_elapsed = Some(0.);
            }
        })
    });
}

pub(super) fn click_selector(cx: &mut VisualTestContext, selector: &'static str) {
    let position = cx.debug_bounds(selector).unwrap().center();
    cx.simulate_click(position, Modifiers::none());
}

fn codex_protocol_fixture(
    executable: &std::path::Path,
    capture: &std::path::Path,
    calls: &std::path::Path,
    delegation: bool,
) {
    use std::os::unix::fs::PermissionsExt;
    let config = serde_json::json!({"capture":capture,"calls":calls,"delegation":delegation});
    let source = r#"#!/usr/bin/python3
import json,sys
config=CONFIG
thread='12345678-1234-4234-8234-123456789012'
def send(value): print(json.dumps(value),flush=True)
for line in sys.stdin:
 v=json.loads(line); method=v.get('method'); p=v.get('params',{})
 if method=='initialize': send({'id':v['id'],'result':{}})
 elif method=='config/read': send({'id':v['id'],'result':{'config':{'model_provider':'openai'}}})
 elif method=='command/exec':
  open(config['calls'],'w').write(json.dumps(v))
  send({'method':'command/exec/outputDelta','params':{'processId':p['processId'],'stream':'stdout','deltaBase64':'U0FOREJPWF9PSw==','capReached':False}})
  send({'id':v['id'],'result':{'exitCode':0,'stdout':'','stderr':''}})
 elif method in ['thread/start','thread/resume','thread/fork']:
  open(config['calls'],'w').write(json.dumps(v))
  mode=method
  send({'id':v['id'],'result':{'thread':{'id':thread}}})
 elif method=='thread/compact/start':
  open(config['calls'],'w').write(json.dumps(v))
  send({'id':v['id'],'result':{}})
  send({'method':'item/completed','params':{'threadId':thread,'turnId':'compact','item':{'type':'contextCompaction','id':'compact'}}})
  send({'method':'turn/completed','params':{'threadId':thread,'turn':{'id':'compact','status':'completed'}}})
 elif method=='review/start':
  open(config['calls'],'w').write(json.dumps(v))
  send({'id':v['id'],'result':{'turn':{'id':'review'}}})
  send({'method':'item/completed','params':{'threadId':thread,'item':{'id':'review','type':'exitedReviewMode','review':'Review complete'}}})
  send({'method':'turn/completed','params':{'threadId':thread,'turn':{'id':'review','status':'completed'}}})
 elif method in ['thread/goal/get','thread/goal/set','thread/goal/clear']:
  open(config['calls'],'w').write(json.dumps(v))
  send({'id':v['id'],'result':{'cleared':True} if method=='thread/goal/clear' else {'goal':{'objective':'Verify actions','status':'active','tokensUsed':3}}})
 elif method=='turn/start':
  prompt=''.join(x.get('text','') for x in p['input'])
  open(config['capture'],'w').write(prompt)
  send({'id':v['id'],'result':{'turn':{'id':'turn-1'}}})
  if config['delegation']:
   for tool,status,message in [('spawnAgent','running',''),('wait','completed','Parser verified MARKER-892')]:
    send({'method':'item/completed','params':{'threadId':thread,'turnId':'turn-1','item':{'id':tool,'type':'collabAgentToolCall','tool':tool,'receiverThreadIds':['child-native'],'status':'completed','agentsStates':{'child-native':{'status':status,'message':message}}}}})
   reply='Child result received'
  elif mode=='thread/fork': reply='Fork reply'
  elif mode=='thread/resume': reply='Resume reply'
  else: reply='Native reply works'
  send({'method':'item/completed','params':{'threadId':thread,'turnId':'turn-1','item':{'id':'reply','type':'agentMessage','text':reply}}})
  send({'method':'turn/completed','params':{'threadId':thread,'turn':{'id':'turn-1','status':'completed'}}})
"#;
    std::fs::write(
        executable,
        source.replace(
            "CONFIG",
            &format!(
                "json.loads({})",
                serde_json::to_string(&config.to_string()).unwrap()
            ),
        ),
    )
    .unwrap();
    std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o700)).unwrap();
}

fn composer_text(view: &Entity<Orbit>, cx: &mut VisualTestContext) -> String {
    cx.update(|_, cx| view.read(cx).composer.read(cx).text())
}

fn composer_focused(view: &Entity<Orbit>, cx: &mut VisualTestContext) -> bool {
    cx.update(|window, cx| {
        view.read(cx)
            .composer
            .read(cx)
            .focus_handle
            .is_focused(window)
    })
}

#[gpui::test]
fn native_delegation_button_preserves_draft_and_waits_for_submit(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("Inspect the parser without editing files");
    draw_frame(&view, cx);
    click_selector(cx, "delegate-task");
    assert_eq!(
        composer_text(&view, cx),
        "/delegate Inspect the parser without editing files"
    );
    assert!(composer_focused(&view, cx));
    click_selector(cx, "delegate-task");
    assert_eq!(
        composer_text(&view, cx),
        "/delegate Inspect the parser without editing files"
    );
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(!app.starting && app.runs.is_empty());
    });
}

#[gpui::test]
fn delegate_launches_inside_the_current_native_session_and_renders_child_result(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    let captured = cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            let executable = app.project.join("delegate-fixture.sh");
            let captured = app.project.join("delegate-input.txt");
            codex_protocol_fixture(
                &executable,
                &captured,
                &app.project.join("delegate-calls.json"),
                true,
            );
            app.active = Some("fixture-session".into());
            app.sessions[0].external_id = Some("abcdefab-1234-4234-8234-123456789012".into());
            app.open_tabs = vec!["fixture-session".into()];
            app.agents.push(Agent {
                kind: AgentKind::Codex,
                name: "Fixture Codex".into(),
                available: true,
                executable: Some(executable),
            });
            app.agents.remove(0);
            app.composer.update(cx, |input, cx| {
                input.set_text("/delegate Inspect the parser MARKER-892", cx)
            });
            app.send(window, cx);
            captured
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        view.update(cx, |app, cx| {
            while let Ok(event) = app.engine.events.try_recv() {
                if matches!(event, Event::Parsed(_, _)) {
                    app.receive(event, cx);
                }
            }
        });
        if cx.update(|_, cx| {
            let app = view.read(cx);
            !app.starting && app.runs.is_empty() && app.finalizing.is_empty()
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Delegation process did not complete"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let prompt = std::fs::read_to_string(captured).unwrap();
    assert!(prompt.contains("native spawn_agent") && prompt.contains("MARKER-892"));
    assert!(prompt.contains("permission restrictions"));
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.active.as_deref(), Some("fixture-session"));
        assert_eq!(app.open_tabs, ["fixture-session"]);
        assert_eq!(app.sessions.len(), 1);
        assert!(app.rows.iter().any(|row| matches!(&row.block, MessageBlock::Interaction(chat_interactions::ChatInteraction::Delegation { status:chat_interactions::InteractionStatus::Completed, agent:Some(agent), details, .. }) if agent.contains("child-native") && details.contains("MARKER-892"))));
        assert!(app.rows.iter().any(|row| matches!(&row.block, MessageBlock::Paragraph(text) if text == "Child result received")));
    });
}

#[test]
fn assistant_provider_continuations_never_add_headers_or_change_identity() {
    let mut rows = vec![
        Row {
            role: Some("assistant".into()),
            block: MessageBlock::Paragraph("Earlier".into()),
        },
        Row {
            role: None,
            block: MessageBlock::Paragraph("continued".into()),
        },
        Row {
            role: Some("user".into()),
            block: MessageBlock::Paragraph("Next".into()),
        },
        Row {
            role: None,
            block: MessageBlock::Paragraph("attachment".into()),
        },
    ];
    stamp_row_providers(&mut rows, AgentKind::Codex);
    stamp_row_providers(&mut rows, AgentKind::Claude);
    assert_eq!(
        agent_for_role(rows[0].role.as_deref(), AgentKind::OpenCode),
        AgentKind::Codex
    );
    assert_eq!(
        agent_for_role(rows[1].role.as_deref(), AgentKind::OpenCode),
        AgentKind::Codex
    );
    assert!(row_has_header(rows[0].role.as_deref()));
    assert!(!row_has_header(rows[1].role.as_deref()));
    assert!(rows[3].role.is_none());
    let history = Orbit::history_rows(vec![engine::ParsedMessage {
        role: "assistant:claude".into(),
        blocks: vec![
            MessageBlock::Paragraph("First".into()),
            MessageBlock::Paragraph("Second".into()),
        ],
    }]);
    assert_eq!(
        agent_for_role(history[1].role.as_deref(), AgentKind::Gemini),
        AgentKind::Claude
    );
    assert!(!row_has_header(history[1].role.as_deref()));
}

#[gpui::test]
fn staged_agent_selection_preserves_historical_and_live_provider_labels(cx: &mut TestAppContext) {
    use chat_interactions::{ChatInteraction, Speaker};
    let (view, cx) = fixture(cx);
    cx.simulate_resize(size(px(1180.), px(1400.)));
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        let mut other = app.sessions[0].clone();
        other.id = "unrelated-chat".into();
        other.title = "Unrelated Claude task".into();
        other.agent = AgentKind::Claude;
        app.sessions.push(other);
        app.agents = [
            AgentKind::Codex,
            AgentKind::Claude,
            AgentKind::Gemini,
            AgentKind::OpenCode,
            AgentKind::Antigravity,
            AgentKind::Cursor,
            AgentKind::Grok,
        ]
        .into_iter()
        .map(|kind| Agent {
            kind,
            name: kind.name().into(),
            executable: Some("/unused-selection-fixture".into()),
            available: true,
        })
        .collect();
        app.rows = Orbit::history_rows(vec![
            engine::ParsedMessage {
                role: "assistant".into(),
                blocks: vec![
                    MessageBlock::Paragraph("Earlier Codex task".into()),
                    MessageBlock::Interaction(ChatInteraction::Speaker {
                        speaker: Speaker::Assistant,
                        text: "Legacy speaker".into(),
                    }),
                ],
            },
            engine::ParsedMessage {
                role: "assistant:gemini".into(),
                blocks: vec![
                    MessageBlock::Paragraph("Earlier Gemini task".into()),
                    MessageBlock::Interaction(ChatInteraction::Speaker {
                        speaker: Speaker::Assistant,
                        text: "Explicit Gemini speaker".into(),
                    }),
                ],
            },
        ]);
        app.list.reset(app.rows.len());
        let mut run = idle_run("Live Codex output".into());
        run.live_blocks = vec![
            MessageBlock::Paragraph("Live Codex output".into()),
            MessageBlock::Interaction(ChatInteraction::Speaker {
                speaker: Speaker::Assistant,
                text: "Live Codex speaker".into(),
            }),
        ];
        app.runs.insert("fixture-session".into(), run);
        app.apply_live_rows("fixture-session");
        cx.notify();
    });
    for target in [
        AgentKind::Claude,
        AgentKind::Gemini,
        AgentKind::OpenCode,
        AgentKind::Antigravity,
        AgentKind::Cursor,
        AgentKind::Grok,
        AgentKind::Codex,
    ] {
        view.update_in(cx, |app, window, cx| {
            app.request_handoff(target, window, cx);
            // Repaint fresh streaming output after selecting a different target.
            // A run without a retry request must still retain its own identity.
            app.apply_live_rows("fixture-session");
            assert_eq!(app.agent, target);
            assert_eq!(app.transcript_agent(), AgentKind::Codex);
            assert_eq!(app.rows[4].role.as_deref(), Some("assistant:codex"));
            assert_eq!(app.rows[5].role.as_deref(), Some("continuation:codex"));
            assert!(app.handoff_pending.is_none());
            assert_eq!(app.sessions[0].title, "Fixture conversation");
            assert_eq!(app.sessions[0].agent, AgentKind::Codex);
            assert_eq!(app.sessions[1].title, "Unrelated Claude task");
            assert_eq!(app.sessions[1].agent, AgentKind::Claude);
            cx.notify();
        });
        draw_frame(&view, cx);
        assert!(cx.debug_bounds("transcript-provider-0-codex").is_some());
        assert!(cx.debug_bounds("interaction-speaker-1-codex").is_some());
        assert!(cx.debug_bounds("transcript-provider-2-gemini").is_some());
        assert!(cx.debug_bounds("interaction-speaker-3-gemini").is_some());
        assert!(cx.debug_bounds("transcript-provider-4-codex").is_some());
        assert!(cx.debug_bounds("interaction-speaker-5-codex").is_some());
    }
    view.update(cx, |app, cx| {
        app.runs.remove("fixture-session");
        app.agent = AgentKind::Cursor;
        assert_eq!(app.transcript_agent(), AgentKind::Codex);
        // Another chat uses its own binding, never the global composer target.
        app.active = Some("unrelated-chat".into());
        assert_eq!(app.transcript_agent(), AgentKind::Claude);
        cx.notify();
    });
}

#[gpui::test]
fn live_preview_is_bounded_and_save_failure_does_not_leave_a_chat_stuck(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            let mut run = idle_run(String::new());
            run.handle = RunHandle::test_events(
                (0..128)
                    .map(|_| RuntimeEvent::Text("🦀ab\n".repeat(600)))
                    .collect(),
            );
            app.runs.insert("fixture-session".into(), run);
            // Exactly one bounded drain batch leaves disconnect for the next pass.
            app.poll("fixture-session", cx);
            let run = app.runs.get("fixture-session").unwrap();
            assert!(run.text.len() <= 64 * 1024 && run.live_prefix_omitted);
            app.finalizing.insert("fixture-session".into());
            app.receive(
                Event::FinishError("fixture-session".into(), "Fixture disk unavailable".into()),
                cx,
            );
            assert!(!app.finalizing.contains("fixture-session"));
            assert!(app.notice.as_deref().unwrap().contains("disk unavailable"));
        })
    });
}

#[gpui::test]
fn native_composer_handles_unicode_multiline_clipboard_and_submit(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("A🦀e\u{301}");
    assert_eq!(composer_text(&view, cx), "A🦀e\u{301}");
    cx.simulate_keystrokes("backspace");
    assert_eq!(
        composer_text(&view, cx),
        "A🦀",
        "Backspace must remove a full grapheme"
    );
    cx.simulate_keystrokes("shift-enter");
    cx.simulate_input("日本語");
    assert_eq!(composer_text(&view, cx), "A🦀\n日本語");
    cx.simulate_keystrokes("cmd-shift-a cmd-c");
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("A🦀\n日本語")
    );
    cx.simulate_keystrokes("cmd-x");
    assert_eq!(composer_text(&view, cx), "");
    cx.simulate_keystrokes("cmd-v shift-left backspace");
    assert_eq!(composer_text(&view, cx), "A🦀\n日本");
    cx.simulate_keystrokes("enter");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(
            app.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("Install Codex")),
            "Enter must reach the app's submit handler"
        );
        assert!(
            app.runs.is_empty(),
            "Fixture submission must never launch a real agent"
        );
    });
    assert_eq!(composer_text(&view, cx), "A🦀\n日本");
}

#[gpui::test]
fn completed_paged_response_blocks_a_new_send_until_history_commit_is_visible(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            app.composer
                .update(cx, |input, cx| input.set_text("Next task stays intact", cx));
            app.receive(Event::PagedFinished("fixture-session".into()), cx);
            assert!(app.loading);
            app.send(window, cx);
            assert!(!app.starting && app.runs.is_empty());
            assert!(app.notice.is_none());
            app.receive(
                Event::Loaded(
                    "fixture-session".into(),
                    app.generation,
                    vec![],
                    false,
                    false,
                ),
                cx,
            );
            assert!(!app.loading);
        })
    });
    assert_eq!(composer_text(&view, cx), "Next task stays intact");
}

#[gpui::test]
fn oversized_paste_preserves_draft_and_displays_limit(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("Keep this 🦀");
    cx.write_to_clipboard(ClipboardItem::new_string(
        "é".repeat(input::MAX_DRAFT_BYTES),
    ));
    cx.simulate_keystrokes("cmd-v");
    assert_eq!(composer_text(&view, cx), "Keep this 🦀");
    cx.update(|_, cx| {
        assert!(
            view.read(cx)
                .notice
                .as_deref()
                .is_some_and(|notice| notice.contains("64 KB"))
        )
    });
}

#[gpui::test]
fn search_keyboard_navigation_and_modal_dismissal_restore_focus(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_keystrokes("cmd-k");
    draw_frame(&view, cx);
    cx.update(|window, cx| {
        let app = view.read(cx);
        assert!(app.modal == Some(Modal::Search));
        assert!(app.search.read(cx).focus_handle.is_focused(window));
    });
    cx.simulate_keystrokes("cmd-k");
    assert!(
        composer_focused(&view, cx),
        "Closing search with its shortcut must restore composer focus"
    );
    cx.simulate_keystrokes("cmd-k escape");
    assert!(
        composer_focused(&view, cx),
        "Escape must restore composer focus"
    );
    cx.simulate_keystrokes("cmd-k enter");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(
            app.modal.is_none(),
            "Enter must open the first search result"
        );
        assert_eq!(app.active.as_deref(), Some("fixture-session"));
    });
    assert!(composer_focused(&view, cx));
}

#[gpui::test]
fn home_agents_and_projects_paint_without_animation_or_processes(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    draw_frame(&view, cx);
    for modal in [Modal::Agents, Modal::Projects, Modal::Background] {
        cx.update(|window, cx| view.update(cx, |app, cx| app.show_modal(modal, window, cx)));
        draw_frame(&view, cx);
        cx.update(|_, cx| {
            if modal == Modal::Agents {
                assert!(view.read(cx).agent_picker.is_some());
                assert!(view.read(cx).modal.is_none());
            } else {
                assert!(view.read(cx).modal == Some(modal));
            }
            assert!(!view.read(cx).motion);
        });
        cx.simulate_keystrokes("escape");
        assert!(composer_focused(&view, cx));
    }
}

#[gpui::test]
fn collapsed_sidebar_has_a_visible_reopen_control(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_keystrokes("cmd-b");
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        assert!(!view.read(cx).sidebar);
        assert_eq!(view.read(cx).sidebar_width, 0.);
    });
    let button = cx
        .debug_bounds("show-sidebar")
        .expect("reopen button in workspace header");
    assert_eq!(button.size, size(px(28.), px(28.)));
    cx.simulate_click(button.center(), Modifiers::none());
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        assert!(view.read(cx).sidebar);
        assert_eq!(view.read(cx).sidebar_width, 200.);
    });
}

#[gpui::test]
fn chat_background_control_opens_settings_and_preserves_the_draft(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("keep this draft");
    draw_frame(&view, cx);
    for (control, expected) in [
        ("settings", Modal::Settings),
        ("chat-background", Modal::Background),
    ] {
        let button = cx.debug_bounds(control).unwrap();
        cx.simulate_click(button.center(), Modifiers::none());
        draw_frame(&view, cx);
        cx.update(|_, cx| assert!(view.read(cx).modal == Some(expected)));
        cx.simulate_keystrokes("escape");
        draw_frame(&view, cx);
    }
    assert_eq!(composer_text(&view, cx), "keep this draft");
    assert!(composer_focused(&view, cx));
}

#[gpui::test]
fn appearance_sliders_preserve_draft_and_persist_independent_values(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("Keep my words 🦀");
    draw_frame(&view, cx);
    click_selector(cx, "settings");
    cx.update(|_, cx| {
        view.read(cx)
            .appearance_search
            .as_ref()
            .unwrap()
            .clone()
            .update(cx, |input, cx| input.set_text("glass blur", cx))
    });
    draw_frame(&view, cx);
    let blur = cx.debug_bounds("glass-blur-slider").unwrap();
    cx.simulate_click(
        point(blur.right() - px(6.), blur.center().y),
        Modifiers::none(),
    );
    draw_frame(&view, cx);
    let opacity = cx.debug_bounds("glass-opacity-slider").unwrap();
    cx.simulate_click(
        point(opacity.left() + px(6.), opacity.center().y),
        Modifiers::none(),
    );
    draw_frame(&view, cx);
    let (store, expected) = cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            assert_eq!(app.appearance.blur_radius, 64);
            assert_eq!(app.appearance.opacity, 0.);
            let path = app
                .controls_home
                .parent()
                .unwrap()
                .join("saved-ui-workspace.json");
            let store = std::sync::Arc::new(workspace_state::StateStore::new(path).unwrap());
            app.workspace_store = Some(store.clone());
            app.save_workspace_now(cx);
            (store, app.appearance)
        })
    });
    store
        .flush()
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap();
    let restored = workspace_state::StateStore::new(cx.update(|_, cx| {
        view.read(cx)
            .controls_home
            .parent()
            .unwrap()
            .join("saved-ui-workspace.json")
    }))
    .unwrap();
    assert_eq!(restored.initial().appearance, expected);
    assert_eq!(
        restored.initial().drafts["__new_conversation__"],
        "Keep my words 🦀"
    );
    assert_eq!(composer_text(&view, cx), "Keep my words 🦀");
    cx.update(|_, cx| {
        view.read(cx)
            .appearance_search
            .as_ref()
            .unwrap()
            .clone()
            .update(cx, |input, cx| input.set_text("wallpaper", cx))
    });
    draw_frame(&view, cx);
    click_selector(cx, "wallpaper-settings");
    cx.update(|_, cx| assert!(view.read(cx).modal == Some(Modal::Background)));
}

#[gpui::test]
fn workspace_restores_active_view_and_drafts_without_warming_inactive_tabs(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            let mut inactive = app.sessions[0].clone();
            inactive.id = "inactive-tab".into();
            app.sessions.push(inactive);
            app.restore_pending = Some(workspace_state::WorkspaceState {
                project: Some(app.project.clone()),
                active_chat: Some("fixture-session".into()),
                open_tabs: vec!["inactive-tab".into(), "fixture-session".into()],
                drafts: std::collections::BTreeMap::from([
                    ("fixture-session".into(), "active unsent text".into()),
                    ("inactive-tab".into(), "cold unsent text".into()),
                ]),
                model: Some("saved-model".into()),
                sidebar: false,
                motion: false,
                allow_edits: true,
                appearance: appearance::Appearance {
                    blur_radius: 39,
                    opacity: 0.32,
                },
                transcript: Some(workspace_state::TranscriptAnchor {
                    history_offset: 42,
                    row_index: 1,
                    pixel_offset: -7.,
                    at_bottom: false,
                }),
                ..Default::default()
            });
            app.restore_workspace(window, cx);
            assert_eq!(app.active.as_deref(), Some("fixture-session"));
            assert_eq!(app.open_tabs, ["inactive-tab", "fixture-session"]);
            assert!(!app.sidebar && !app.motion && app.allow_edits);
            assert_eq!(app.sidebar_width, 0.);
            assert!(app.rows.is_empty() && app.loading);
            assert_eq!(app.drafts["inactive-tab"], "cold unsent text");
            app.receive(
                Event::Model(
                    "fixture-session".into(),
                    app.generation,
                    Some("old-sqlite-model".into()),
                    None,
                    None,
                ),
                cx,
            );
            assert_eq!(app.model.as_deref(), Some("saved-model"));
            app.receive(
                Event::ViewportRestored(
                    "fixture-session".into(),
                    app.generation,
                    42,
                    (0..3)
                        .map(|i| engine::ParsedMessage {
                            role: "assistant".into(),
                            blocks: vec![MessageBlock::Paragraph(format!("restored row {i}"))],
                        })
                        .collect(),
                    true,
                    true,
                ),
                cx,
            );
            assert_eq!(app.history_offset, 42);
            assert_eq!(app.history_pages[0].number, 42);
            assert_eq!(app.list.logical_scroll_top().item_ix, 1);
            assert_eq!(app.list.logical_scroll_top().offset_in_item, px(-7.));
            assert_eq!(app.rows.len(), 3);
            assert!(!app.following_bottom && !app.loading);
            assert!(app.restore_scroll.is_none());
            let snapshot = app.workspace_snapshot(cx);
            assert_eq!(snapshot.drafts["fixture-session"], "active unsent text");
            assert_eq!(snapshot.model.as_deref(), Some("saved-model"));
            assert_eq!(snapshot.appearance.blur_radius, 39);
        })
    });
    assert_eq!(composer_text(&view, cx), "active unsent text");
}

#[gpui::test]
fn switching_chats_cancels_pending_viewport_restoration_after_model_reply(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.active = Some("previous".into());
            app.restore_scroll = Some(workspace_state::TranscriptAnchor {
                history_offset: 42,
                ..Default::default()
            });
            app.restore_model = None; // The model reply may arrive before history.
            app.open_session(app.sessions[0].clone(), window, cx);
            assert!(app.restore_scroll.is_none());
            assert!(app.following_bottom);
            assert_eq!(app.active.as_deref(), Some("fixture-session"));
        })
    });
}

#[gpui::test]
fn normal_quit_flushes_the_final_draft_without_blocking_or_launching_an_agent(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("Final words before quitting 🦀");
    let path = cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            let path = app
                .controls_home
                .parent()
                .unwrap()
                .join("quit-workspace.json");
            app.workspace_store = Some(std::sync::Arc::new(
                workspace_state::StateStore::new(path.clone()).unwrap(),
            ));
            app.quit_after_save(window, cx);
            assert!(app.quit_pending);
            assert!(app.runs.is_empty());
            path
        })
    });
    cx.run_until_parked();
    let saved = workspace_state::StateStore::new(path).unwrap();
    assert_eq!(
        saved.initial().drafts["__new_conversation__"],
        "Final words before quitting 🦀"
    );
    cx.update(|_, cx| assert!(view.read(cx).notice.is_none()));
}

#[gpui::test]
fn normal_quit_stops_runs_and_saves_the_full_final_queue_before_history_flush(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    let database = cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            let mut run = idle_run(String::new());
            run.handle = RunHandle::test_events(
                (0..128)
                    .map(|i| RuntimeEvent::Text(format!("Quit output {i}\n")))
                    .collect(),
            );
            app.runs.insert("fixture-session".into(), run);
            app.quit_after_save(window, cx);
            assert!(app.runs["fixture-session"].handle.is_cancelled());
            assert!(app.quit_pending);
            app.controls_home
                .parent()
                .unwrap()
                .join("data/history.sqlite")
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        let done = cx.update(|_, cx| view.read(cx).runs.is_empty());
        let text = rusqlite::Connection::open(&database).ok().and_then(|db| db.query_row("SELECT text FROM messages WHERE session_id='fixture-session' ORDER BY id DESC LIMIT 1", [], |row| row.get::<_, String>(0)).ok());
        if done
            && text
                .as_ref()
                .is_some_and(|text| text.contains("Quit output 127"))
        {
            let text = text.unwrap();
            assert!(text.contains("Quit output 0\n"));
            assert!(text.contains("stopped"));
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Quit must persist final events before its history flush"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    cx.update(|_, cx| assert!(view.read(cx).notice.is_none()));
}

#[gpui::test]
fn every_installed_provider_can_be_selected_without_starting_a_process(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    for (kind, selector) in [
        (AgentKind::Codex, "agent-choice-codex"),
        (AgentKind::Claude, "agent-choice-claude"),
        (AgentKind::Gemini, "agent-choice-gemini"),
        (AgentKind::OpenCode, "agent-choice-opencode"),
        (AgentKind::Antigravity, "agent-choice-antigravity"),
        (AgentKind::Cursor, "agent-choice-cursor"),
        (AgentKind::Grok, "agent-choice-grok"),
    ] {
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.agents = vec![Agent {
                    kind,
                    name: kind.name().into(),
                    executable: Some("/usr/bin/false".into()),
                    available: true,
                }];
                app.show_modal(Modal::Agents, window, cx);
            })
        });
        draw_frame(&view, cx);
        let button = cx.debug_bounds(selector).unwrap().center();
        cx.simulate_click(button, Modifiers::none());
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(app.agent, kind);
            assert!(app.runs.is_empty());
            assert!(app.modal.is_none());
        });
    }
}

#[gpui::test]
fn ten_thousand_transcript_rows_render_only_a_viewport(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.loading = false;
        app.rows = (0..10_000)
            .map(|index| Row {
                role: (index % 10 == 0).then(|| "assistant".into()),
                block: MessageBlock::Paragraph(format!(
                    "Row {index:05}: bounded native transcript content 🦀"
                )),
            })
            .collect();
        app.list.reset(app.rows.len());
        cx.notify();
    });
    draw_frame(&view, cx);
    let rendered = cx.update(|_, cx| view.read(cx).rendered_rows.get());
    assert!(
        rendered > 0 && rendered < 100,
        "A 10,000-row transcript painted {rendered} rows at 1180×790"
    );
    let mut counts = vec![rendered];
    for item_ix in [0, 5_000, 9_990] {
        view.update(cx, |app, cx| {
            app.list.scroll_to(ListOffset {
                item_ix,
                offset_in_item: px(0.),
            });
            cx.notify();
        });
        draw_frame(&view, cx);
        let rendered = cx.update(|_, cx| view.read(cx).rendered_rows.get());
        assert!(
            rendered > 0 && rendered < 100,
            "Scrolling to row {item_ix} painted {rendered} rows"
        );
        counts.push(rendered);
    }
    eprintln!("10,000-row viewport callback counts at 1180×790: {counts:?}");
}

#[gpui::test]
fn native_ime_marking_uses_utf16_offsets_and_preserves_surrogate_pairs(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        let composer = view.read(cx).composer.clone();
        composer.update(cx, |input, cx| {
            input.set_text("A🦀Z", cx);
            input.replace_and_mark_text_in_range(Some(1..3), "日🦀", Some(1..3), window, cx);
            assert_eq!(input.text(), "A日🦀Z");
            assert_eq!(input.marked_text_range(window, cx), Some(1..4));
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                2..4
            );
            input.replace_text_in_range(None, "日本", window, cx);
            assert_eq!(input.text(), "A日本Z");
            assert_eq!(input.marked_text_range(window, cx), None);
            assert_eq!(
                input.selected_text_range(false, window, cx).unwrap().range,
                3..3
            );
        });
    });
    draw_frame(&view, cx);
    assert_eq!(composer_text(&view, cx), "A日本Z");
}

#[gpui::test]
fn loaded_projects_reveal_in_order_without_layout_shift_or_offscreen_replay(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.run_until_parked();
    view.update(cx, |app, cx| {
        app.motion = true;
        app.projects.clear();
        app.projects_revealed = false;
        cx.notify();
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).project_reveal.is_none()));
    let workspace = cx.debug_bounds("workspace-pane").unwrap();
    view.update(cx, |app, cx| {
        let projects = (0..100)
            .map(|i| Project {
                name: format!("Loaded project {i}"),
                path: app.project.join(format!("loaded-{i}")),
            })
            .collect();
        app.receive(
            Event::Ready(
                discovery::Discovery {
                    projects,
                    ..Default::default()
                },
                app.sessions.clone(),
                None,
            ),
            cx,
        );
    });
    freeze_list_entrances(&view, cx);
    draw_frame(&view, cx);
    assert!(
        cx.debug_bounds("project-99").is_none(),
        "Offscreen projects stay virtualized"
    );
    view.update(cx, |app, cx| {
        app.project_reveal.as_mut().unwrap().test_elapsed = Some(0.08);
        cx.notify();
    });
    draw_frame(&view, cx);
    let first = cx.debug_bounds("project-0").unwrap();
    let fourth = cx.debug_bounds("project-3").unwrap();
    assert!(
        cx.debug_bounds("project-content-0").unwrap().left()
            < cx.debug_bounds("project-content-3").unwrap().left(),
        "Earlier rows must enter before later rows"
    );
    assert_eq!(first.size.height, px(32.));
    assert_eq!(fourth.size.height, px(32.));
    assert_eq!(cx.debug_bounds("workspace-pane").unwrap(), workspace);
    view.update(cx, |app, cx| {
        app.project_reveal.as_mut().unwrap().test_elapsed = Some(2.);
        cx.notify();
    });
    draw_frame(&view, cx);
    let settled = cx.debug_bounds("project-0").unwrap();
    assert_eq!(settled.left(), cx.debug_bounds("project-3").unwrap().left());
    assert_eq!(settled.size, first.size);
    view.update(cx, |app, cx| {
        assert!(
            app.project_reveal.is_none(),
            "The entrance must release its timeline"
        );
        app.rebuild_projects();
        cx.notify();
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        assert!(
            view.read(cx).project_reveal.is_none(),
            "Refresh must not replay the entrance"
        )
    });
    view.update(cx, |app, cx| {
        app.projects_revealed = false;
        app.motion = false;
        cx.notify();
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(app.projects_revealed && app.project_reveal.is_none());
    });
    assert_eq!(cx.debug_bounds("project-0").unwrap(), settled);
}

#[gpui::test]
fn conversation_list_inherits_stagger_and_restarts_only_for_a_new_scope(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.run_until_parked();
    view.update(cx, |app, cx| {
        app.motion = true;
        app.sessions.clear();
        app.workspace_sessions.clear();
        app.sessions_revealed = false;
        cx.notify();
    });
    draw_frame(&view, cx);
    let pane = cx.debug_bounds("workspace-pane").unwrap();
    view.update(cx, |app, cx| {
        let template = SessionSummary {
            id: String::new(),
            title: "Loaded conversation".into(),
            agent: AgentKind::Codex,
            project_path: app.project.clone(),
            updated_at: 1,
            external_id: None,
            transcript_path: None,
            execution_path: None,
        };
        let sessions = (0..100)
            .map(|i| SessionSummary {
                id: format!("loaded-{i}"),
                ..template.clone()
            })
            .collect();
        app.receive(
            Event::Ready(discovery::Discovery::default(), sessions, None),
            cx,
        );
    });
    freeze_list_entrances(&view, cx);
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("session-99").is_none());
    view.update(cx, |app, cx| {
        app.sessions_reveal.as_mut().unwrap().test_elapsed = Some(0.08);
        cx.notify();
    });
    draw_frame(&view, cx);
    let row = cx.debug_bounds("session-0").unwrap();
    assert_eq!(row.size.height, px(68.));
    assert!(
        cx.debug_bounds("session-content-0").unwrap().left()
            < cx.debug_bounds("session-content-3").unwrap().left()
    );
    assert_eq!(cx.debug_bounds("workspace-pane").unwrap(), pane);
    view.update(cx, |app, cx| {
        app.sessions_reveal.as_mut().unwrap().test_elapsed = Some(2.);
        cx.notify();
    });
    draw_frame(&view, cx);
    assert_eq!(cx.debug_bounds("session-0").unwrap(), row);
    view.update(cx, |app, cx| {
        assert!(app.sessions_reveal.is_none());
        app.active = Some("loaded-20".into());
        app.rebuild_workspace_sessions();
        cx.notify();
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).sessions_reveal.is_none()));
    view.update(cx, |app, cx| {
        app.project = app.project.join("another-project");
        for session in &mut app.sessions {
            session.project_path = app.project.clone();
        }
        app.rebuild_workspace_sessions();
        cx.notify();
    });
    freeze_list_entrances(&view, cx);
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).sessions_reveal.is_some()));
    view.update(cx, |app, cx| {
        app.motion = false;
        cx.notify();
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).sessions_reveal.is_none()));
}

#[gpui::test]
fn opened_chat_inherits_stagger_at_the_visible_tail_without_moving_its_anchor(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.run_until_parked();
    view.update(cx, |app, cx| {
        app.motion = true;
        app.active = Some("fixture-session".into());
        app.receive(
            Event::Loaded(
                "fixture-session".into(),
                app.generation,
                (0..10_000)
                    .map(|i| engine::ParsedMessage {
                        role: "assistant".into(),
                        blocks: vec![MessageBlock::Paragraph(format!("Loaded text {i}"))],
                    })
                    .collect(),
                false,
                false,
            ),
            cx,
        );
        app.transcript_reveal.as_mut().unwrap().test_elapsed = Some(0.);
    });
    draw_frame(&view, cx);
    let (first, anchor) = cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(app.rendered_rows.get() < 100);
        let first = app
            .transcript_reveal
            .as_ref()
            .unwrap()
            .visible
            .get()
            .unwrap()
            .0;
        assert!(
            first > 9900,
            "Staggering must start at the mounted tail, not row zero"
        );
        (first, app.list.logical_scroll_top())
    });
    // GPUI's test selector API requires static names; these three small names
    // live only in this test process.
    let row_selector: &'static str = format!("transcript-row-{first}").leak();
    let first_selector: &'static str = format!("transcript-content-{first}").leak();
    let fourth_selector: &'static str = format!("transcript-content-{}", first + 3).leak();
    let bounds = cx.debug_bounds(row_selector).unwrap();
    view.update(cx, |app, cx| {
        app.transcript_reveal.as_mut().unwrap().test_elapsed = Some(0.08);
        cx.notify();
    });
    draw_frame(&view, cx);
    assert!(
        cx.debug_bounds(first_selector).unwrap().left()
            < cx.debug_bounds(fourth_selector).unwrap().left()
    );
    assert_eq!(cx.debug_bounds(row_selector).unwrap(), bounds);
    view.update(cx, |app, cx| {
        app.transcript_reveal.as_mut().unwrap().test_elapsed = Some(2.);
        cx.notify();
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(app.transcript_reveal.is_none());
        let current = app.list.logical_scroll_top();
        assert_eq!(current.item_ix, anchor.item_ix);
        assert_eq!(current.offset_in_item, anchor.offset_in_item);
    });
    view.update(cx, |app, cx| {
        app.list.scroll_to(ListOffset {
            item_ix: 5000,
            offset_in_item: px(0.),
        });
        cx.notify();
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(app.transcript_reveal.is_none());
        assert!(app.rendered_rows.get() < 100);
    });
    view.update(cx, |app, cx| {
        app.motion = false;
        app.receive(
            Event::Loaded(
                "fixture-session".into(),
                app.generation,
                vec![engine::ParsedMessage {
                    role: "user".into(),
                    blocks: vec![MessageBlock::Paragraph("Immediately visible".into())],
                }],
                false,
                false,
            ),
            cx,
        );
        assert!(app.transcript_reveal.is_none());
    });
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("transcript-content-0").is_some());
}

#[gpui::test]
fn streaming_text_enters_once_without_refading_older_messages(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.run_until_parked();
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.rows = vec![Row {
            role: Some("assistant:codex".into()),
            block: MessageBlock::Paragraph("An already visible message".into()),
        }];
        app.list.reset(1);
        cx.notify();
    });
    draw_frame(&view, cx);
    let old = cx.debug_bounds("transcript-content-0").unwrap();
    view.update(cx, |app, cx| {
        app.motion = true;
        let mut run = idle_run("First words".into());
        run.live_blocks = vec![MessageBlock::Paragraph("First words".into())];
        app.runs.insert("fixture-session".into(), run);
        app.apply_live_rows("fixture-session");
        app.transcript_reveal.as_mut().unwrap().test_elapsed = Some(0.);
        cx.notify();
    });
    draw_frame(&view, cx);
    view.update(cx, |app, cx| {
        let reveal = app.transcript_reveal.as_mut().unwrap();
        assert_eq!(reveal.only_from, 1);
        reveal.test_elapsed = Some(0.03);
        cx.notify();
    });
    draw_frame(&view, cx);
    // Bottom alignment moves the old message upward as a reply is added, but
    // its horizontal paint position must remain fully settled.
    assert_eq!(
        cx.debug_bounds("transcript-content-0").unwrap().left(),
        old.left()
    );
    assert!(cx.debug_bounds("transcript-content-1").unwrap().left() > old.left());
    view.update(cx, |app, cx| {
        app.runs.get_mut("fixture-session").unwrap().live_blocks = vec![MessageBlock::Paragraph(
            "First words and more streamed words".into(),
        )];
        app.apply_live_rows("fixture-session");
        assert_eq!(
            app.transcript_reveal.as_ref().unwrap().test_elapsed,
            Some(0.03),
            "A token update must not replace the entrance timeline"
        );
        cx.notify();
    });
    draw_frame(&view, cx);
    let position = cx.debug_bounds("chat-pane").unwrap().center();
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(16.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        assert!(
            view.read(cx).transcript_reveal.is_none(),
            "Reading/scrolling must settle the text immediately"
        )
    });
}

#[gpui::test]
fn project_navigation_opens_its_chats_and_keeps_columns_aligned(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, _| {
        let other = app.project.with_file_name("other-project");
        app.projects.push(Project {
            name: "Other project".into(),
            path: other.clone(),
        });
        let mut session = app.sessions[0].clone();
        session.id = "other-session".into();
        session.title = "A chat inside another project".into();
        session.project_path = other;
        app.sessions.push(session);
        app.rebuild_workspace_sessions();
    });
    draw_frame(&view, cx);
    let rail = cx
        .debug_bounds("project-rail")
        .expect("Project rail laid out");
    let workspace = cx
        .debug_bounds("workspace-pane")
        .expect("Sessions pane laid out");
    let chat = cx.debug_bounds("chat-pane").expect("Chat pane laid out");
    assert_eq!(rail.size.width, px(200.));
    assert_eq!(workspace.size.width, px(260.));
    assert_eq!(workspace.origin.x, rail.right());
    assert_eq!(chat.origin.x, workspace.right());
    assert_eq!(rail.size.height, workspace.size.height);
    assert_eq!(workspace.size.height, chat.size.height);
    let project = cx
        .debug_bounds("project-1")
        .expect("Second project visible");
    cx.simulate_click(project.center(), Modifiers::none());
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.active.as_deref(), Some("other-session"));
        assert_eq!(app.workspace_sessions.len(), 1);
        assert_eq!(app.workspace_sessions[0].id, "other-session");
        assert!(!app.all_projects);
    });
    let session = cx
        .debug_bounds("session-0")
        .expect("Project chat visible and accessible");
    assert_eq!(session.size.height, px(68.));
    assert!(session.left() >= workspace.left() && session.right() <= workspace.right());
}

#[gpui::test]
fn long_sidebar_labels_keep_equal_insets_and_session_text_stays_inside_cards(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        for (ix, name) in [
            "messages-plugin-messages-with-an-extremely-long-project-name",
            "海 日本語 🦀 a second project with a much longer name than the sidebar",
        ]
        .into_iter()
        .enumerate()
        {
            let path = app.project.with_file_name(format!("long-project-{ix}"));
            app.projects.push(Project {
                name: name.into(),
                path: path.clone(),
            });
            let mut session = app.sessions[0].clone();
            session.id = format!("long-session-{ix}");
            session.title = name.repeat(3);
            session.project_path = path;
            app.sessions.push(session);
        }
        app.all_projects = true;
        app.rebuild_workspace_sessions();
        cx.notify();
    });
    draw_frame(&view, cx);
    let rail = cx.debug_bounds("project-rail").unwrap();
    let workspace = cx.debug_bounds("workspace-pane").unwrap();
    for (project_id, label_id, card_id, provider_id, title_id, metadata_id) in [
        (
            "project-1",
            "project-label-1",
            "session-1",
            "session-provider-1",
            "session-title-1",
            "session-project-1",
        ),
        (
            "project-2",
            "project-label-2",
            "session-2",
            "session-provider-2",
            "session-title-2",
            "session-project-2",
        ),
    ] {
        let project = cx.debug_bounds(project_id).unwrap();
        let label = cx.debug_bounds(label_id).unwrap();
        assert_eq!(project.left() - rail.left(), px(8.));
        assert_eq!(rail.right() - project.right(), px(9.));
        assert_eq!(project.size.width, rail.size.width - px(17.));
        assert!(label.left() >= project.left() + px(8. + 15. + 8.));
        assert!(label.right() <= project.right() - px(8.));

        let card = cx.debug_bounds(card_id).unwrap();
        let provider = cx.debug_bounds(provider_id).unwrap();
        let title = cx.debug_bounds(title_id).unwrap();
        let project = cx.debug_bounds(metadata_id).unwrap();
        assert_eq!(card.left() - workspace.left(), px(8.));
        assert_eq!(workspace.right() - card.right(), px(9.));
        for text in [provider, title, project] {
            assert_eq!(text.left() - card.left(), px(10.));
            assert_eq!(card.right() - text.right(), px(10.));
            assert!(text.top() >= card.top() + px(6.));
            assert!(text.bottom() <= card.bottom() - px(6.));
        }
        assert_eq!(title.top() - provider.bottom(), px(2.));
        assert_eq!(project.top() - title.bottom(), px(2.));
    }
}

#[gpui::test]
fn rich_transcript_and_tool_disclosure_paint_without_protocol_leakage(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx,|app,cx|{
        app.active=Some("fixture-session".into());app.loading=false;
        app.rows=presentation::blocks("Read **the [documentation](https://example.com)** and `src/main.rs`.\n\n```rust\nfn main() {}\n```").into_iter().enumerate()
            .map(|(ix,block)|Row{role:(ix==0).then(||"assistant".into()),block}).collect();
        app.rows.push(Row{role:None,block:MessageBlock::Activity{label:"Ran a command".into(),details:"cargo check --bin orbit".into()}});
        app.list.reset(app.rows.len());app.list.scroll_to(ListOffset{item_ix:0,offset_in_item:px(0.)});cx.notify();
    });
    draw_frame(&view, cx);
    let ix = cx.update(|_, cx| view.read(cx).rows.len() - 1);
    view.update(cx, |app, cx| {
        app.expanded_tools.insert(ix);
        cx.notify();
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).expanded_tools.contains(&ix)));
}

#[gpui::test]
fn nested_project_chats_preserve_group_and_session_drafts(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let root = cx.update(|_, cx| view.read(cx).project.clone());
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            let mut nested = app.sessions[0].clone();
            nested.id = "nested-session".into();
            nested.title = "Nested chat".into();
            nested.project_path = root.join("child");
            nested.external_id = Some("fresh-native-id".into());
            app.sessions.insert(0, nested);
            app.select_project(root.clone(), window, cx);
            app.receive(
                Event::Loaded(
                    "nested-session".into(),
                    app.generation,
                    vec![],
                    false,
                    false,
                ),
                cx,
            );
        })
    });
    cx.simulate_input("Keep this draft 🦀");
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            assert_eq!(app.project, root);
            assert_eq!(app.workspace_sessions.len(), 2);
            let mut stale = app.sessions[0].clone();
            stale.external_id = None;
            let first = app.sessions[1].clone();
            app.open_session(first, window, cx);
            app.open_session(stale, window, cx);
            assert_eq!(app.project, root);
            app.receive(
                Event::ProjectSessions(root.clone(), app.project_generation, app.sessions.clone()),
                cx,
            );
            assert_eq!(app.workspace_sessions.len(), 2);
        })
    });
    assert_eq!(composer_text(&view, cx), "Keep this draft 🦀");
}

#[gpui::test]
fn earlier_history_prepends_and_latest_control_reopens_the_bounded_window(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.receive(
            Event::Loaded(
                "fixture-session".into(),
                app.generation,
                vec![engine::ParsedMessage {
                    role: "assistant".into(),
                    blocks: vec![MessageBlock::Paragraph("Latest history window".into())],
                }],
                true,
                false,
            ),
            cx,
        );
    });
    draw_frame(&view, cx);
    let earlier = cx
        .debug_bounds("load-earlier")
        .expect("Earlier control is available");
    cx.simulate_click(earlier.center(), Modifiers::none());
    view.update(cx, |app, cx| {
        assert_eq!(app.history_offset, 0);
        assert!(app.loading_earlier);
        app.receive(
            Event::OlderLoaded(
                "fixture-session".into(),
                app.generation,
                vec![engine::ParsedMessage {
                    role: "user".into(),
                    blocks: vec![MessageBlock::Paragraph("Earlier history window".into())],
                }],
                false,
            ),
            cx,
        );
        assert_eq!(app.rows.len(), 2);
        assert_eq!(app.history_offset, 1);
        assert!(app.has_newer);
        assert!(!app.has_older);
    });
    draw_frame(&view, cx);
    let newer = cx
        .debug_bounds("load-newer")
        .expect("Newer control replaces earlier at history start");
    cx.simulate_click(newer.center(), Modifiers::none());
    cx.update(|_, cx| assert!(view.read(cx).loading));
}

#[gpui::test]
fn prepending_history_preserves_the_visible_message_and_pixel_offset(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.receive(
            Event::Loaded(
                "fixture-session".into(),
                app.generation,
                (0..100)
                    .map(|ix| engine::ParsedMessage {
                        role: "assistant".into(),
                        blocks: vec![MessageBlock::Paragraph(format!("Existing message {ix}"))],
                    })
                    .collect(),
                true,
                false,
            ),
            cx,
        );
        app.list.scroll_to(ListOffset {
            item_ix: 20,
            offset_in_item: px(7.),
        });
    });
    draw_frame(&view, cx);
    let before = cx.update(|_, cx| view.read(cx).list.bounds_for_item(20).unwrap());
    view.update(cx, |app, cx| {
        app.loading_earlier = true;
        app.receive(
            Event::OlderLoaded(
                "fixture-session".into(),
                app.generation,
                (0..25)
                    .map(|ix| engine::ParsedMessage {
                        role: "user".into(),
                        blocks: vec![MessageBlock::Paragraph(format!(
                            "Earlier {ix}\n{}",
                            "Variable height text\n".repeat(ix % 5)
                        ))],
                    })
                    .collect(),
                false,
            ),
            cx,
        );
        let anchor = app.list.logical_scroll_top();
        assert_eq!(anchor.item_ix, 45);
        assert_eq!(anchor.offset_in_item, px(7.));
        assert!(
            matches!(&app.rows[45].block, MessageBlock::Paragraph(s) if s == "Existing message 20")
        );
    });
    draw_frame(&view, cx);
    let after = cx.update(|_, cx| view.read(cx).list.bounds_for_item(45).unwrap());
    assert_eq!(
        before.origin, after.origin,
        "The same message must remain at the same screen position"
    );
}

#[gpui::test]
fn scrolling_to_the_top_requests_one_older_page(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.receive(
            Event::Loaded(
                "fixture-session".into(),
                app.generation,
                (0..100)
                    .map(|ix| engine::ParsedMessage {
                        role: "assistant".into(),
                        blocks: vec![MessageBlock::Paragraph(format!("Message {ix}"))],
                    })
                    .collect(),
                true,
                false,
            ),
            cx,
        );
        app.list.scroll_to(ListOffset {
            item_ix: 5,
            offset_in_item: px(0.),
        });
    });
    draw_frame(&view, cx);
    let position = cx.update(|_, cx| view.read(cx).list.viewport_bounds().center());
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(1500.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(app.loading_earlier);
        assert!(app.list.logical_scroll_top().item_ix < 3);
        assert_eq!(
            app.history_offset, 0,
            "The cursor advances only when the page arrives"
        );
    });
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta: ScrollDelta::Pixels(point(px(0.), px(100.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).loading_earlier));
}

#[gpui::test]
fn distant_history_pages_evict_and_rehydrate_without_resetting_the_anchor(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let messages = |page| {
        (0..200)
            .map(|ix| engine::ParsedMessage {
                role: "assistant".into(),
                blocks: vec![MessageBlock::Paragraph(format!(
                    "Page {page}, message {ix}"
                ))],
            })
            .collect()
    };
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.receive(
            Event::Loaded(
                "fixture-session".into(),
                app.generation,
                messages(0),
                true,
                false,
            ),
            cx,
        );
        for page in 1..=12 {
            app.receive(
                Event::OlderLoaded(
                    "fixture-session".into(),
                    app.generation,
                    messages(page),
                    true,
                ),
                cx,
            );
        }
        assert_eq!(app.rows.len(), 2600);
        assert!(app.history_pages.iter().filter(|p| p.resident).count() <= 8);
        assert!(!app.deferred_rows.is_empty());
        let page = app.history_pages.iter().find(|p| !p.resident).unwrap();
        let (number, start, len) = (page.number, page.start, page.len);
        app.history_visible = start..start + 10;
        app.list.scroll_to(ListOffset {
            item_ix: start + 2,
            offset_in_item: px(5.),
        });
        app.receive(
            Event::HistoryPage(
                "fixture-session".into(),
                app.generation,
                number,
                messages(number),
                true,
            ),
            cx,
        );
        assert!(
            app.history_pages
                .iter()
                .find(|p| p.number == number)
                .unwrap()
                .resident
        );
        assert!(!(start..start + len).any(|ix| app.deferred_rows.contains_key(&ix)));
        assert_eq!(app.list.logical_scroll_top().item_ix, start + 2);
        assert_eq!(app.list.logical_scroll_top().offset_in_item, px(5.));
        assert!(app.history_pages.iter().filter(|p| p.resident).count() <= 8);
    });
    draw_frame(&view, cx);
}

#[gpui::test]
fn project_removal_is_reversible_and_survives_discovery_refresh(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let preferences = cx.update(|_, cx| view.read(cx).preferences.events.clone());
    let event = preferences.recv_blocking().unwrap();
    view.update(cx, |app, cx| {
        app.source_projects = app.projects.clone();
        app.receive_preferences(event, cx);
    });
    let path = cx.update(|_, cx| view.read(cx).project.clone());
    draw_frame(&view, cx);
    let menu = cx.debug_bounds("project-menu-0").unwrap().center();
    let workspace_before = cx.debug_bounds("workspace-pane").unwrap();
    cx.simulate_click(menu, Modifiers::none());
    draw_frame(&view, cx);
    let popup = cx.debug_bounds("project-action-menu").unwrap();
    assert_eq!(popup.size.width, px(160.));
    assert!(popup.size.height <= px(104.));
    assert!((popup.origin.x - menu.x).abs() < px(16.));
    assert_eq!(cx.debug_bounds("workspace-pane").unwrap(), workspace_before);
    cx.update(|_, cx| assert!(view.read(cx).modal.is_none()));
    let removal = cx.debug_bounds("remove-project").unwrap().center();
    cx.simulate_click(removal, Modifiers::none());
    view.update(cx, |app, cx| {
        assert!(app.projects.is_empty());
        assert!(app.hidden_projects.contains(&path));
        app.receive(
            Event::Ready(
                discovery::Discovery {
                    projects: vec![Project {
                        name: "Rediscovered".into(),
                        path: path.clone(),
                    }],
                    ..Default::default()
                },
                app.sessions.clone(),
                None,
            ),
            cx,
        );
        assert!(
            app.projects.is_empty(),
            "Refresh must not resurrect a removed project"
        );
        app.receive_preferences(preferences::Event::Error("fixture error".into()), cx);
        app.restore_project(cx);
        assert_eq!(app.projects.len(), 1);
        assert!(!app.hidden_projects.contains(&path));
    });
    assert!(
        path.is_dir(),
        "Removing from Orbit must retain the project directory"
    );
}

#[gpui::test]
fn all_agent_svg_marks_fit_the_strip_with_equal_spacing(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let kinds = [
        AgentKind::Codex,
        AgentKind::Claude,
        AgentKind::Gemini,
        AgentKind::OpenCode,
        AgentKind::Antigravity,
        AgentKind::Cursor,
        AgentKind::Grok,
    ];
    view.update(cx, |app, _| {
        app.agents = kinds
            .into_iter()
            .map(|kind| Agent {
                kind,
                name: kind.name().into(),
                executable: None,
                available: kind != AgentKind::Grok,
            })
            .collect();
    });
    draw_frame(&view, cx);
    let rail = cx.debug_bounds("project-rail").unwrap();
    let mut previous: Option<Bounds<Pixels>> = None;
    for kind in kinds {
        assert!(svg_assets::asset(agent_icon(kind)).is_some());
        let selector = match kind {
            AgentKind::Codex => "agent-strip-codex",
            AgentKind::Claude => "agent-strip-claude",
            AgentKind::Gemini => "agent-strip-gemini",
            AgentKind::OpenCode => "agent-strip-opencode",
            AgentKind::Antigravity => "agent-strip-antigravity",
            AgentKind::Cursor => "agent-strip-cursor",
            AgentKind::Grok => "agent-strip-grok",
        };
        let bounds = cx.debug_bounds(selector).unwrap();
        assert_eq!(bounds.size, size(px(14.), px(14.)));
        assert!(bounds.origin.x >= rail.origin.x + px(16.));
        assert!(bounds.right() < rail.right() - px(16.));
        if let Some(previous) = previous {
            assert_eq!(bounds.origin.y, previous.origin.y);
            assert_eq!(bounds.origin.x - previous.right(), px(6.));
        }
        previous = Some(bounds);
    }
}

#[gpui::test]
fn chat_dots_delete_with_undo_and_filter_rediscovery_and_search(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let session = cx.update(|_, cx| view.read(cx).sessions[0].clone());
    view.update(cx, |app, _| {
        app.active = Some(session.id.clone());
        app.open_tabs.push(session.id.clone());
        app.starting = true;
    });
    draw_frame(&view, cx);
    let workspace = cx.debug_bounds("workspace-pane").unwrap();
    click_selector(cx, "session-menu-0");
    draw_frame(&view, cx);
    let popup = cx.debug_bounds("thread-action-menu").unwrap();
    assert_eq!(popup.size.width, px(160.));
    assert!(popup.size.height <= px(134.));
    assert_eq!(workspace, cx.debug_bounds("workspace-pane").unwrap());
    click_selector(cx, "delete-chat");
    view.update(cx, |app, _| {
        assert!(
            !app.hidden_sessions.contains(&session.id),
            "Starting chats must be retained"
        );
        app.starting = false;
    });
    draw_frame(&view, cx);
    click_selector(cx, "delete-chat");
    view.update(cx, |app, cx| {
        assert!(app.hidden_sessions.contains(&session.id));
        assert!(app.active.is_none());
        assert!(app.open_tabs.is_empty());
        assert!(app.action_menu.is_none());
        app.receive(
            Event::Ready(discovery::Discovery::default(), vec![session.clone()], None),
            cx,
        );
        app.receive(
            Event::Search(app.search_generation, vec![session.clone()]),
            cx,
        );
        assert!(app.workspace_sessions.is_empty());
        assert!(app.visible_sessions.is_empty());
        // Removing only metadata visibility keeps the stored/native session available for Undo.
        assert!(app.sessions.iter().any(|item| item.id == session.id));
    });
    draw_frame(&view, cx);
    click_selector(cx, "undo-chat-deletion");
    view.update(cx, |app, cx| {
        assert!(!app.hidden_sessions.contains(&session.id));
        assert_eq!(app.workspace_sessions.len(), 1);
        // A delayed initial preference snapshot must not undo a newer restoration.
        app.receive_preferences(
            preferences::Event::Loaded(
                HashSet::new(),
                vec![],
                HashSet::from([session.id.clone()]),
                Default::default(),
            ),
            cx,
        );
        assert!(!app.hidden_sessions.contains(&session.id));
    });
}

#[gpui::test]
fn compact_menu_dismisses_clamps_to_window_and_releases_after_animation(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    draw_frame(&view, cx);
    click_selector(cx, "project-menu-0");
    draw_frame(&view, cx);
    cx.simulate_keystrokes("escape");
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).action_menu.is_none()));
    click_selector(cx, "project-menu-0");
    draw_frame(&view, cx);
    // Dismissal does not consume an outside click: the search action still opens.
    click_selector(cx, "search");
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        assert!(view.read(cx).action_menu.is_none());
        assert!(view.read(cx).modal == Some(Modal::Search));
    });
    view.update(cx, |app, _| {
        app.modal = None;
        app.motion = true;
        app.action_menu = Some(ActionMenu {
            target: MenuTarget::Project(app.projects[0].clone()),
            anchor: point(px(1178.), px(788.)),
            started: Instant::now() - Duration::from_millis(321),
            closing: false,
            closing_from: 1.,
        });
    });
    draw_frame(&view, cx);
    let popup = cx.debug_bounds("project-action-menu").unwrap();
    assert!(popup.right() <= px(1172.));
    assert!(popup.bottom() <= px(782.));
    view.update(cx, |app, _| {
        app.dismiss_action_menu();
        let menu = app.action_menu.as_mut().unwrap();
        assert!(menu.closing);
        menu.started = Instant::now() - Duration::from_millis(121);
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).action_menu.is_none()));
}

#[gpui::test]
fn file_explorer_is_lazy_and_keeps_a_usable_chat_width(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|_, cx| assert!(view.read(cx).file_browser.is_none()));
    cx.simulate_keystrokes("cmd-shift-e");
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        assert!(view.read(cx).files_visible);
        assert!(view.read(cx).file_browser.is_some());
    });
    let inspector = cx.debug_bounds("files-inspector").unwrap();
    let chat = cx.debug_bounds("chat-pane").unwrap();
    assert_eq!(inspector.size.width, px(360.));
    assert!(chat.size.width >= px(360.));
    cx.simulate_keystrokes("cmd-shift-e");
    cx.update(|_, cx| {
        assert!(!view.read(cx).files_visible);
        assert!(
            view.read(cx)
                .file_browser
                .as_ref()
                .unwrap()
                .read(cx)
                .is_suspended_and_lightweight()
        );
    });
}

#[gpui::test]
fn stale_history_errors_do_not_clear_the_current_conversation(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.generation = 7;
        app.loading = true;
        app.notice = None;
        app.receive(
            Event::LoadError(
                "previous-session".into(),
                6,
                "Previous import failed".into(),
            ),
            cx,
        );
        assert!(app.loading);
        assert!(app.notice.is_none());
        app.receive(
            Event::LoadError("fixture-session".into(), 6, "Older request failed".into()),
            cx,
        );
        assert!(app.loading);
        assert!(app.notice.is_none());
        app.receive(
            Event::LoadError("fixture-session".into(), 7, "Current import failed".into()),
            cx,
        );
        assert!(!app.loading);
        assert_eq!(app.notice.as_deref(), Some("Current import failed"));
    });
}

#[gpui::test]
fn project_refresh_preserves_a_running_sessions_new_native_id(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        app.sessions[0].external_id = Some("new-cli-session-id".into());
        app.sessions[0].execution_path = Some(app.project.join("actual-worktree"));
        let mut stale = app.sessions[0].clone();
        stale.external_id = None;
        stale.execution_path = None;
        app.receive(
            Event::ProjectSessions(app.project.clone(), app.project_generation, vec![stale]),
            cx,
        );
        assert_eq!(
            app.sessions[0].external_id.as_deref(),
            Some("new-cli-session-id")
        );
        assert_eq!(
            app.sessions[0].execution_path.as_ref(),
            Some(&app.project.join("actual-worktree"))
        );
    });
}

fn controls_fixture(app: &mut Orbit, kind: AgentKind, models: usize) {
    app.agents.push(Agent {
        kind,
        name: kind.name().into(),
        executable: Some("/usr/bin/false".into()),
        available: true,
    });
    app.controls.insert(
        kind.command(),
        (
            Instant::now(),
            agent_controls::ControlInfo {
                defaults:agent_controls::ModelDefaults{project:app.model_project(),..Default::default()},
                auth: agent_controls::AuthState::VerifiedSignedIn,
                account: None,
                limits: Vec::new(),
                catalog: None,
                models: (0..models)
                    .map(|ix| agent_controls::ModelChoice { reasoning_efforts:vec![],
                        id: format!("model-{ix}"),
                        label: format!("Model {ix}"),
                    })
                    .collect(),
                note: None,
            },
        ),
    );
}

#[gpui::test]
fn agent_picker_is_compact_aligned_and_virtualizes_models(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("Preserve this draft");
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.agents.clear();
            controls_fixture(app, AgentKind::Codex, 128);
            app.show_modal(Modal::Agents, window, cx);
        })
    });
    draw_frame(&view, cx);
    let pane = cx.debug_bounds("agent-picker").unwrap();
    assert_eq!(pane.size.width, px(280.));
    assert!(pane.size.height < px(360.), "Searchable model picker is too tall: {:?}",pane.size.height);
    assert!(cx.debug_bounds("model-search").is_some(),"Model search was omitted from the compact picker");
    let row = cx.debug_bounds("agent-choice-codex").unwrap();
    let logo = cx.debug_bounds("picker-logo-codex").unwrap();
    let name = cx.debug_bounds("picker-label-codex").unwrap();
    let lock = cx.debug_bounds("agent-account-codex").unwrap();
    for item in [logo, name, lock] {
        assert!((f32::from(item.center().y - row.center().y)).abs() < 1.);
    }
    assert!(lock.left() > name.left());
    assert!(
        cx.debug_bounds("model-choice-128").is_none(),
        "offscreen models must not be painted"
    );
    let model = cx.debug_bounds("model-choice-1").unwrap();
    cx.simulate_click(model.center(), Modifiers::none());
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        assert_eq!(view.read(cx).model.as_deref(), Some("model-0"));
        assert!(view.read(cx).runs.is_empty());
        assert!(view.read(cx).agent_picker.is_none());
    });
    assert_eq!(composer_text(&view, cx), "Preserve this draft");
}

#[gpui::test]
fn switching_agents_only_selects_composer_destination_without_losing_worktree_or_draft(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("Use the same checkout");
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.agents.clear();
            controls_fixture(app, AgentKind::Codex, 0);
            controls_fixture(app, AgentKind::Claude, 0);
            app.active = Some("fixture-session".into());
            app.sessions[0].external_id = Some("native-codex-id".into());
            let checkout = app.project.join("checkout");
            std::fs::create_dir_all(&checkout).unwrap();
            app.sessions[0].execution_path = Some(checkout);
            app.show_modal(Modal::Agents, window, cx);
        })
    });
    draw_frame(&view, cx);
    let button = cx.debug_bounds("agent-choice-codex").unwrap().center();
    cx.simulate_click(button, Modifiers::none());
    draw_frame(&view, cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            assert_eq!(app.active.as_deref(), Some("fixture-session"));
            app.show_modal(Modal::Agents, window, cx);
        })
    });
    draw_frame(&view, cx);
    let button = cx.debug_bounds("agent-choice-claude").unwrap().center();
    cx.simulate_click(button, Modifiers::none());
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.agent, AgentKind::Claude);
        assert_eq!(app.active.as_deref(), Some("fixture-session"));
        assert!(app.handoff_pending.is_none());
        assert!(!app.starting);
        assert_eq!(app.sessions[0].agent, AgentKind::Codex);
        assert!(app.handoff_packets.is_empty());
        assert_eq!(
            app.sessions[0].external_id.as_deref(),
            Some("native-codex-id")
        );
        assert!(app.runs.is_empty());
    });
    assert_eq!(composer_text(&view, cx), "Use the same checkout");
}

#[gpui::test]
fn stale_model_options_cannot_override_a_newly_opened_thread(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.generation = 7;
        app.receive(
            Event::Model(
                "previous-session".into(),
                7,
                Some("wrong".into()),
                None,
                None,
            ),
            cx,
        );
        app.receive(
            Event::Model(
                "fixture-session".into(),
                6,
                Some("stale".into()),
                None,
                None,
            ),
            cx,
        );
        assert!(app.model.is_none());
        app.receive(
            Event::Model(
                "fixture-session".into(),
                7,
                Some("right".into()),
                None,
                None,
            ),
            cx,
        );
        assert_eq!(app.model.as_deref(), Some("right"));
    });
}

#[gpui::test]
fn file_citation_opens_the_native_inspector_without_losing_the_draft(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("Keep editing");
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            std::fs::write(app.project.join("hello.rs"), "fn hello() {}\n").unwrap();
            app.open_citation("hello.rs:1", window, cx);
            assert!(app.files_visible);
            assert!(app.file_browser.is_some());
            assert!(app.runs.is_empty());
        })
    });
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("files-inspector").is_some());
    assert_eq!(composer_text(&view, cx), "Keep editing");
}

#[gpui::test]
fn native_voice_controls_and_question_choices_use_the_composer(cx: &mut TestAppContext) {
    use chat_interactions::{ChatInteraction, InteractionStatus, QuestionCard, QuestionOption};
    let (view, cx) = fixture(cx);
    assert!(cx.debug_bounds("attach-image").is_some());
    assert!(cx.debug_bounds("dictate").is_some());
    view.update(cx, |app, _| {
        app.active = Some("fixture-session".into());
        app.rows = vec![
            Row {
                role: Some("assistant".into()),
                block: MessageBlock::Interaction(ChatInteraction::Question(QuestionCard {
                    id: "question-1".into(),
                    header: "Approach".into(),
                    prompt: "Which implementation?".into(),
                    options: vec![QuestionOption {
                        label: "Native".into(),
                        description: "Use the existing Rust path".into(),
                    }],
                    answer: None,
                    historical: true,
                })),
            },
            Row {
                role: None,
                block: MessageBlock::Interaction(ChatInteraction::Delegation {
                    title: "File review".into(),
                    details: "Finished reviewing the changed files.".into(),
                    status: InteractionStatus::Completed,
                    agent: Some("Codex".into()),
                }),
            },
        ];
        app.list.reset(app.rows.len());
    });
    draw_frame(&view, cx);
    click_selector(cx, "question-option-0-0");
    draw_frame(&view, cx);
    assert_eq!(composer_text(&view, cx), "Which implementation?\n\nNative");
    assert!(composer_focused(&view, cx));
    click_selector(cx, "interaction-1");
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(app.expanded_tools.contains(&1));
        assert!(
            app.interaction_animations.is_empty(),
            "Motion-off controls must settle in one frame"
        );
        assert!(
            app.runs.is_empty(),
            "Choosing a historical reply must never auto-submit or launch a CLI"
        );
    });
}

fn image_fixture(view: &Entity<Orbit>, cx: &mut VisualTestContext) -> attachments::Attachment {
    let (source, managed) = cx.update(|_, cx| {
        let app = view.read(cx);
        (app.project.join("image.png"), app.attachment_dir.clone())
    });
    image::RgbaImage::from_pixel(48, 32, image::Rgba([40, 100, 170, 255]))
        .save(&source)
        .unwrap();
    // The isolated test imports synchronously; production imports use a worker.
    attachments::import(
        &source,
        &managed,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap()
}

#[gpui::test]
fn image_preview_hover_removal_and_provider_switch_preserve_drafts(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let attachment = image_fixture(&view, cx);
    let id = attachment.id;
    cx.simulate_input("Review the screenshot");
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.attachments.push(attachment);
            app.agents.clear();
            controls_fixture(app, AgentKind::Codex, 0);
            controls_fixture(app, AgentKind::Claude, 0);
            app.active = Some("fixture-session".into());
            app.show_modal(Modal::Agents, window, cx);
        })
    });
    draw_frame(&view, cx);
    click_selector(cx, "agent-choice-claude");
    draw_frame(&view, cx);
    assert_eq!(composer_text(&view, cx), "Review the screenshot");
    cx.update(|_, cx| assert_eq!(view.read(cx).attachments[0].id, id));
    let bounds = cx
        .debug_bounds(Box::leak(format!("attachment-{id}").into_boxed_str()))
        .unwrap();
    cx.simulate_click(bounds.center(), Modifiers::none());
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("attachment-preview").is_some());
    cx.simulate_keystrokes("escape");
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(app.attachment_preview.is_none());
        assert!(
            app.attachment_preview_cancel
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert_eq!(app.attachments.len(), 1);
    });
    cx.simulate_mouse_move(bounds.center(), None, Modifiers::none());
    draw_frame(&view, cx);
    let remove = cx
        .debug_bounds(Box::leak(
            format!("remove-attachment-{id}").into_boxed_str(),
        ))
        .unwrap();
    cx.simulate_click(remove.center(), Modifiers::none());
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        assert!(view.read(cx).attachments.is_empty());
        assert!(view.read(cx).attachment_preview.is_none());
    });
    assert_eq!(composer_text(&view, cx), "Review the screenshot");
}

#[gpui::test]
fn unsupported_image_provider_keeps_draft_and_import_cancellation_is_scoped(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    let attachment = image_fixture(&view, cx);
    cx.simulate_input("Keep this draft");
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.attachments.push(attachment);
            app.agents.clear();
            controls_fixture(app, AgentKind::Cursor, 0);
            app.active = Some("fixture-session".into());
            app.request_handoff(AgentKind::Cursor, window, cx);
            assert_eq!(app.agent, AgentKind::Cursor);
            assert_eq!(app.sessions[0].agent, AgentKind::Codex);
            assert!(app.handoff_pending.is_none());
            app.send(window, cx);
            assert!(
                app.notice
                    .as_deref()
                    .is_some_and(|s| s.contains("image input"))
            );
            assert_eq!(app.attachments.len(), 1);
            assert!(app.runs.is_empty());
            assert!(!app.starting);
            let cancel = app.attachment_cancel.clone();
            let generation = app.attachment_generation;
            app.reset_attachment_imports();
            assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
            assert!(
                !app.attachment_cancel
                    .load(std::sync::atomic::Ordering::Acquire)
            );
            assert!(app.attachment_generation > generation);
        })
    });
    assert_eq!(composer_text(&view, cx), "Keep this draft");
}

#[gpui::test]
fn an_image_removed_during_its_exit_animation_is_never_sent_or_saved(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let attachment = image_fixture(&view, cx);
    let id = attachment.id;
    cx.simulate_input("Only send the text");
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.motion = true;
            app.attachments.push(attachment);
            app.attachment_removals.insert(id, Instant::now());
            app.agent = AgentKind::Gemini;
            app.send(window, cx);
            assert!(app.attachments.is_empty());
            assert!(app.attachment_removals.is_empty());
            assert_eq!(
                app.prompt_with_attachments("Only send the text"),
                "Only send the text"
            );
            assert!(
                app.notice
                    .as_deref()
                    .is_some_and(|s| s.contains("Install Gemini")),
                "Removed images must not reach image preflight"
            );
            assert!(app.runs.is_empty());
        })
    });
    assert_eq!(composer_text(&view, cx), "Only send the text");
}

pub(super) fn idle_run(text: String) -> Running {
    Running { timing:None,
        agent: AgentKind::Codex,
        submitted_prompt: String::new(),
        handle: RunHandle::idle_test_handle(),
        text,
        live_prefix_omitted: false,
        status: "Working".into(),
        stream_start: None,
        revision: 1,
        parsed_revision: 0,
        parse_pending: false,
        live_blocks: vec![],
        live_earlier: false,
        parse_epoch: uuid::Uuid::new_v4(),
        retry: None,
        submitted_attachments: vec![],
    }
}

#[gpui::test]
fn live_parsing_coalesces_deltas_and_rejects_previous_run_results(cx: &mut TestAppContext) {
    use chat_interactions::{ChatInteraction, InteractionStatus};
    let (view, cx) = fixture(cx);
    let card = ChatInteraction::Status {
        title: "Completed".into(),
        details: String::new(),
        status: InteractionStatus::Completed,
    };
    let latest = format!(
        "Current response\n\n{}",
        chat_interactions::serialize_interaction(&card)
    );
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.runs
            .insert("fixture-session".into(), idle_run("Old response".into()));
        app.update_stream("fixture-session", cx);
        let run = app.runs.get_mut("fixture-session").unwrap();
        run.text = latest;
        run.revision += 1;
        assert!(run.parse_pending);
        app.update_stream("fixture-session", cx);
    });
    cx.run_until_parked();
    view.update(cx, |app, cx| {
        let run = app.runs.get("fixture-session").unwrap();
        assert!(!run.parse_pending);
        assert_eq!(run.parsed_revision, 2);
        assert!(run.live_blocks.contains(&MessageBlock::Interaction(card)));
        assert!(!format!("{:?}", run.live_blocks).contains("Old response"));
        // Schedule a parse, then immediately replace the run with the same id
        // and revision. Only the invocation epoch can reject the old result.
        let run = app.runs.get_mut("fixture-session").unwrap();
        run.text = "Abandoned invocation".into();
        run.revision = 1;
        run.parsed_revision = 0;
        app.update_stream("fixture-session", cx);
        app.runs
            .insert("fixture-session".into(), idle_run("New invocation".into()));
        app.update_stream("fixture-session", cx);
    });
    cx.run_until_parked();
    cx.update(|_, cx| {
        let run = view.read(cx).runs.get("fixture-session").unwrap();
        assert!(!run.parse_pending);
        assert_eq!(
            run.live_blocks,
            vec![MessageBlock::Paragraph("New invocation".into())]
        );
    });
}

#[gpui::test]
fn glass_panels_are_anchored_clamped_and_preserve_workspace_geometry(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let rail = cx.debug_bounds("project-rail").unwrap();
    let workspace = cx.debug_bounds("workspace-pane").unwrap();
    let chat = cx.debug_bounds("chat-pane").unwrap();
    for (kind, selector, width) in [
        (Modal::Search, "search-popover", 360.),
        (Modal::Projects, "workspace-popover", 320.),
        (Modal::Background, "wallpaper-popover", 340.),
    ] {
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.show_panel_at(kind, point(px(1178.), px(788.)), false, window, cx);
            })
        });
        draw_frame(&view, cx);
        let bounds = cx.debug_bounds(selector).unwrap();
        assert_eq!(bounds.size.width, px(width));
        assert!(bounds.left() >= px(8.) && bounds.top() >= px(8.));
        assert!(bounds.right() <= px(1172.) && bounds.bottom() <= px(782.));
        assert_eq!(cx.debug_bounds("project-rail").unwrap(), rail);
        assert_eq!(cx.debug_bounds("workspace-pane").unwrap(), workspace);
        assert_eq!(cx.debug_bounds("chat-pane").unwrap(), chat);
        assert!(cx.debug_bounds("modal").is_none());
        click_selector(cx, "close-panel");
        draw_frame(&view, cx);
        let close_bounds = cx.debug_bounds("close-panel");
        cx.update(|_, cx| {
            assert!(
                view.read(cx).modal.is_none(),
                "close failed for {selector}; button {close_bounds:?}, surface {bounds:?}"
            );
            assert!(view.read(cx).panel_popover.is_none());
        });
    }
    // No page-wide scrim consumes this click: Search opens immediately.
    cx.update(|window, cx| {
        view.update(cx, |app, cx| app.show_modal(Modal::Background, window, cx))
    });
    draw_frame(&view, cx);
    click_selector(cx, "search");
    draw_frame(&view, cx);
    cx.update(|window, cx| {
        assert!(view.read(cx).modal == Some(Modal::Search));
        assert!(
            view.read(cx)
                .search
                .read(cx)
                .focus_handle
                .is_focused(window)
        );
    });
    cx.simulate_keystrokes("escape");
    assert!(composer_focused(&view, cx));
}

#[gpui::test]
fn glass_dropdown_motion_is_finite_and_exit_releases_the_surface(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.motion = true;
            app.show_panel_at(Modal::Search, point(px(8.), px(82.)), false, window, cx);
        })
    });
    // Use native frames: manual draw reuses GPUI's previous frame timestamp,
    // which can precede a newly created element animation under suite load.
    cx.run_until_parked();
    let initial = cx.debug_bounds("search-popover").unwrap();
    view.update(cx, |app, cx| {
        app.panel_popover.as_mut().unwrap().started = Instant::now() - Duration::from_secs(1);
        cx.notify();
    });
    cx.run_until_parked();
    assert_eq!(
        initial.size,
        cx.debug_bounds("search-popover").unwrap().size
    );
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        assert!(view.read(cx).modal.is_none());
        assert!(view.read(cx).panel_popover.as_ref().unwrap().closing);
    });
    cx.run_until_parked(); // Exit also schedules frames safely.
    view.update(cx, |app, cx| {
        app.panel_popover.as_mut().unwrap().started = Instant::now() - Duration::from_millis(121);
        cx.notify();
    });
    cx.run_until_parked();
    cx.update(|_, cx| assert!(view.read(cx).panel_popover.is_none()));
    assert!(composer_focused(&view, cx));
}

#[gpui::test]
fn glass_dropdown_trigger_toggles_without_reopening_on_outside_mouse_down(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    for motion in [false, true] {
        view.update(cx, |app, cx| {
            app.motion = motion;
            cx.notify();
        });
        cx.run_until_parked();
        click_selector(cx, "settings");
        cx.run_until_parked();
        cx.update(|_, cx| assert!(view.read(cx).modal == Some(Modal::Settings)));
        click_selector(cx, "settings");
        cx.run_until_parked();
        cx.update(|_, cx| assert!(view.read(cx).modal.is_none()));
    }
}

#[gpui::test]
fn search_and_workspace_dropdowns_virtualize_large_lists(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            let template = app.sessions[0].clone();
            app.sessions = (0..10_000)
                .map(|i| {
                    let mut session = template.clone();
                    session.id = format!("dropdown-session-{i}");
                    session.title = format!("Conversation {i}");
                    session
                })
                .collect();
            app.projects = (0..10_000)
                .map(|i| Project {
                    name: format!("Workspace {i}"),
                    path: app.project.join(format!("workspace-{i}")),
                })
                .collect();
            app.show_modal(Modal::Search, window, cx);
        })
    });
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("search-result-0").is_some());
    assert!(cx.debug_bounds("search-result-100").is_none());
    assert!(cx.debug_bounds("search-result-9999").is_none());
    cx.update(|window, cx| view.update(cx, |app, cx| app.show_modal(Modal::Projects, window, cx)));
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("workspace-choice-0").is_some());
    assert!(cx.debug_bounds("workspace-choice-100").is_none());
    assert!(cx.debug_bounds("workspace-choice-9999").is_none());
}

fn failed_run(agent: AgentKind, project: PathBuf, error: &str) -> Running {
    let mut run = idle_run(String::new());
    run.handle = RunHandle::test_events(vec![RuntimeEvent::Failed(error.into())]);
    run.agent = agent;
    run.submitted_prompt = "Original submitted prompt".into();
    run.retry = Some(RunRequest { service_tier:None, reasoning_effort:None,
        agent,
        executable: agent.command().into(),
        project,
        prompt: "Original submitted prompt".into(),
        resume_id: None,
        model: None,
        images: vec![],
        allow_edits: false,
        codex_approvals: crate::runtime::CodexApprovals::default(),
        codex_access: Default::default(),
        trust_workspace: false,
        fork_session: false,
    });
    run
}

#[gpui::test]
fn failed_prompt_restores_draft_and_trust_requires_explicit_click(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.agent = AgentKind::Cursor;
        app.sessions[0].agent = AgentKind::Cursor;
        let project = app.project.clone();
        app.runs.insert(
            "fixture-session".into(),
            failed_run(AgentKind::Cursor, project, "Workspace Trust Required"),
        );
        assert!(!app.poll("fixture-session", cx));
        assert!(app.workspace_trust.is_some());
        assert!(app.workspace_retry.is_none());
        assert!(app.trusted_workspaces.is_empty());
        // Hold final parsing to verify retry cannot race it.
        app.finalizing.insert("fixture-session".into());
    });
    assert_eq!(composer_text(&view, cx), "Original submitted prompt");
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("workspace-trust-popover").is_some());
    click_selector(cx, "approve-workspace-trust");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(
            app.trusted_workspaces
                .contains(&(AgentKind::Cursor, app.project.clone()))
        );
        assert!(!app.allow_edits);
        assert!(app.workspace_retry.is_some());
        assert!(!app.starting);
    });
}

#[gpui::test]
fn failed_run_preserves_new_typing_and_inactive_failure_does_not_replace_notice(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.composer
            .update(cx, |input, cx| input.set_text("New unsent draft", cx));
        app.runs.insert(
            "fixture-session".into(),
            failed_run(
                AgentKind::Codex,
                app.project.clone(),
                "Credit balance is too low",
            ),
        );
        app.poll("fixture-session", cx);
    });
    assert_eq!(composer_text(&view, cx), "New unsent draft");
    view.update(cx, |app, cx| {
        app.active = None;
        app.notice = Some("Active chat notice".into());
        app.runs.insert(
            "fixture-session".into(),
            failed_run(
                AgentKind::Cursor,
                app.project.clone(),
                "Workspace Trust Required",
            ),
        );
        app.poll("fixture-session", cx);
        assert_eq!(app.notice.as_deref(), Some("Active chat notice"));
        assert!(app.workspace_trust.is_none());
        assert!(!app.workspace_trust_open);
        assert_eq!(
            app.drafts.get("fixture-session").map(String::as_str),
            Some("Original submitted prompt")
        );
    });
}

#[gpui::test]
fn trust_cancel_keeps_prompt_and_never_grants_or_queues_execution(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            app.workspace_trust = Some(WorkspaceTrust {
                session: "fixture-session".into(),
                agent: AgentKind::Codex,
                project: app.project.clone(),
            });
            app.composer
                .update(cx, |input, cx| input.set_text("Retained draft", cx));
            app.show_modal(Modal::WorkspaceTrust, window, cx);
        })
    });
    draw_frame(&view, cx);
    click_selector(cx, "cancel-workspace-trust");
    assert_eq!(composer_text(&view, cx), "Retained draft");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(app.workspace_trust.is_none());
        assert!(app.workspace_retry.is_none());
        assert!(app.trusted_workspaces.is_empty());
    });
}

#[gpui::test]
fn composer_runs_native_cli_and_renders_response_and_completion(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let project = cx.update(|_, cx| view.read(cx).project.clone());
    let cli = project.join("fixture-codex");
    let captured = project.join("submitted.txt");
    codex_protocol_fixture(&cli, &captured, &project.join("calls.json"), false);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            app.agents[0].executable = Some(cli);
            app.agents[0].available = true;
            app.composer.update(cx, |input, cx| {
                input.set_text("Run this native prompt 🌊", cx)
            });
            app.send(window, cx);
            assert!(app.starting);
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        // Fixtures suppress automatic discovery subscriptions. Deliver the
        // real worker's final response while retaining the isolated fixtures.
        view.update(cx, |app, cx| {
            while let Ok(event) = app.engine.events.try_recv() {
                match event {
                    Event::Parsed(_, _) => app.receive(event, cx),
                    Event::Error(error) => panic!("State worker failed: {error}"),
                    _ => {}
                }
            }
        });
        let complete = cx.update(|_, cx| {
            let app = view.read(cx);
            !app.starting && app.runs.is_empty() && app.finalizing.is_empty()
                && app.rows.iter().any(|row| matches!(&row.block, MessageBlock::Paragraph(text) if text == "Native reply works"))
        });
        if complete {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Native submission did not complete"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        std::fs::read_to_string(captured).unwrap(),
        "Run this native prompt 🌊"
    );
    assert_eq!(composer_text(&view, cx), "");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(
            app.sessions[0].external_id.as_deref(),
            Some("12345678-1234-4234-8234-123456789012")
        );
        assert!(app.rows.iter().any(|row| matches!(
            &row.block,
            MessageBlock::Interaction(chat_interactions::ChatInteraction::Status {
                status: chat_interactions::InteractionStatus::Completed,
                ..
            })
        )));
        assert!(app.notice.is_none());
    });
    draw_frame(&view, cx);
}

#[gpui::test]
fn cancelled_full_queue_and_failed_runs_finish_as_typed_chat_cards(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    for (cancelled, expected) in [
        (true, chat_interactions::InteractionStatus::Cancelled),
        (false, chat_interactions::InteractionStatus::Failed),
    ] {
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            app.rows.clear();
            app.list.reset(0);
            app.pending_response_start = Some(0);
            let mut run = if cancelled {
                let mut run = idle_run(String::new());
                run.handle = RunHandle::test_events(
                    (0..128)
                        .map(|_| RuntimeEvent::Status("Working".into()))
                        .collect(),
                );
                run.handle.cancel();
                run
            } else {
                failed_run(
                    AgentKind::Codex,
                    app.project.clone(),
                    "Credit balance is too low",
                )
            };
            run.status = "Working".into();
            app.runs.insert("fixture-session".into(), run);
            if app.poll("fixture-session", cx) {
                assert!(!app.poll("fixture-session", cx));
            }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            let done = view.update(cx, |app, cx| {
                while let Ok(event) = app.engine.events.try_recv() {
                    if matches!(event, Event::Parsed(_, _)) {
                        app.receive(event, cx);
                    }
                }
                !app.finalizing.contains("fixture-session")
            });
            if done {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Status did not reach the state worker"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(app.rows.iter().filter(|row| matches!(&row.block, MessageBlock::Interaction(chat_interactions::ChatInteraction::Status { status, .. }) if status == &expected)).count(), 1);
        });
    }
}

#[gpui::test]
fn active_writer_conflict_has_one_compact_failure_and_explicit_copy_recovery(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    let error = "2026-10-13 WARN icon path ignored\n2026-10-13 ERROR thread-store conflict: thread original already has an active writer\nError: thread/resume failed: thread original already has an active writer (code -32600)";
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.runs.insert(
            "fixture-session".into(),
            failed_run(AgentKind::Codex, app.project.clone(), error),
        );
        app.poll("fixture-session", cx);
        assert!(
            app.notice.is_none(),
            "Do not duplicate native diagnostics above the composer"
        );
        assert!(app.session_conflict.is_some());
        assert!(app.fork_next.is_none());
    });
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("session-conflict-popover").is_some());
    click_selector(cx, "continue-session-copy");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.fork_next.as_deref(), Some("fixture-session"));
        assert!(app.workspace_retry.is_some());
        assert!(!app.allow_edits);
        assert!(
            !app.starting,
            "Wait until the previous turn is finalized before retrying"
        );
    });
    assert_eq!(composer_text(&view, cx), "Original submitted prompt");
}

#[gpui::test]
fn imported_codex_forks_once_and_subsequent_prompts_resume_owned_child(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let project = cx.update(|_, cx| view.read(cx).project.clone());
    let source = project.join("source.jsonl");
    let source_bytes = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"original\"}}\n";
    std::fs::write(&source, source_bytes).unwrap();
    let args_path = project.join("cli-args.txt");
    let cli = project.join("fixture-codex");
    codex_protocol_fixture(&cli, &project.join("prompt.txt"), &args_path, false);
    view.update(cx, |app, _| {
        app.sessions[0].id = "codex:abcdefab-1234-4234-8234-123456789012".into();
        app.sessions[0].external_id = Some("abcdefab-1234-4234-8234-123456789012".into());
        app.sessions[0].transcript_path = Some(source.clone());
        app.active = Some("codex:abcdefab-1234-4234-8234-123456789012".into());
        app.agents[0].executable = Some(cli);
        app.agents[0].available = true;
    });
    for (prompt, reply, mode, native_id) in [
        (
            "First continuation",
            "Fork reply",
            "fork",
            "abcdefab-1234-4234-8234-123456789012",
        ),
        (
            "Second continuation",
            "Resume reply",
            "resume",
            "12345678-1234-4234-8234-123456789012",
        ),
    ] {
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.composer
                    .update(cx, |input, cx| input.set_text(prompt, cx));
                app.send(window, cx);
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            let complete = view.update(cx, |app, cx| {
                while let Ok(event) = app.engine.events.try_recv() {
                    if matches!(event, Event::Parsed(_, _)) {
                        app.receive(event, cx);
                    }
                }
                !app.starting
                    && app.runs.is_empty()
                    && app.finalizing.is_empty()
                    && app.rows.iter().any(
                        |row| matches!(&row.block, MessageBlock::Paragraph(text) if text == reply),
                    )
            });
            if complete {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Imported continuation did not complete"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let call: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&args_path).unwrap()).unwrap();
        assert_eq!(call["method"], format!("thread/{mode}"));
        assert_eq!(call["params"]["threadId"], native_id);
        cx.update(|_, cx| {
            let app = view.read(cx);
            assert_eq!(
                app.sessions[0].external_id.as_deref(),
                Some("12345678-1234-4234-8234-123456789012")
            );
            assert!(!imported_codex_needs_fork(&app.sessions[0]));
            assert!(app.notice.is_none());
        });
    }
    assert_eq!(
        std::fs::read_to_string(source).unwrap(),
        source_bytes,
        "Do not write or unlock the source chat"
    );
    draw_frame(&view, cx);
}

#[gpui::test]
fn all_kai_states_paint_beside_status_caption_without_reduced_motion_frames(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, _| {
        app.active = Some("fixture-session".into());
        app.runs
            .insert("fixture-session".into(), idle_run(String::new()));
    });
    let mut expected = None;
    for scene in kai_icons::SCENES {
        view.update(cx, |app, _| {
            app.runs.get_mut("fixture-session").unwrap().status = scene.into();
        });
        draw_frame(&view, cx);
        let bounds = cx.debug_bounds("live-status-indicator").unwrap();
        assert_eq!(bounds.size, size(px(28.), px(18.)));
        let caption = cx.debug_bounds("live-status-caption").unwrap();
        let status = cx.debug_bounds("live-agent-status").unwrap();
        assert!(status.contains(&bounds.center()));
        assert!(bounds.right() <= caption.left());
        assert!((bounds.center().y - caption.center().y).abs() <= px(1.));
        if let Some(expected) = expected {
            assert_eq!(bounds.center().y, expected);
        }
        expected = Some(bounds.center().y);
        cx.update(|_, cx| {
            let indicator = view.read(cx).live_indicator.read(cx);
            assert_eq!(indicator.current_scene(), scene);
            let (canvas, paths) = indicator.painted_geometry();
            assert_eq!(canvas.size, size(px(28.), px(18.)));
            assert_eq!(canvas, bounds);
            assert!(paths > 0, "{scene} must paint visible vector paths");
            assert!(
                !indicator.needs_frames(),
                "Reduced motion must not schedule glyph loops"
            );
        });
    }
}

#[gpui::test]
fn live_status_actually_moves_and_returns_to_header_after_completion(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    // GPUI dispatches native activation asynchronously after activate_window.
    cx.run_until_parked();
    view.update(cx, |app, _| {
        app.motion = true;
        app.active = Some("fixture-session".into());
        let mut run = idle_run(String::new());
        run.status = "Thinking".into();
        app.runs.insert("fixture-session".into(), run);
    });
    draw_frame(&view, cx);
    cx.update(|window, cx| {
        assert!(window.is_window_active());
        let indicator = view.read(cx).live_indicator.clone();
        assert!(indicator.read(cx).needs_frames());
        indicator.update(cx, |indicator, cx| indicator.advance_clock(0.6, cx));
    });
    draw_frame(&view, cx);
    let before = cx.update(|_, cx| view.read(cx).live_indicator.read(cx).geometry());
    cx.update(|_, cx| {
        let indicator = view.read(cx).live_indicator.clone();
        indicator.update(cx, |indicator, cx| indicator.advance_clock(0.4, cx));
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let indicator = view.read(cx).live_indicator.read(cx);
        assert_ne!(
            before,
            indicator.geometry(),
            "The mounted glyph must change between frames"
        );
        assert!(indicator.needs_frames());
        assert!(indicator.painted_geometry().1 > 0);
    });
    view.update(cx, |app, cx| {
        app.runs.get_mut("fixture-session").unwrap().handle =
            RunHandle::test_events(vec![RuntimeEvent::Finished]);
        app.poll("fixture-session", cx);
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let indicator = view.read(cx).live_indicator.clone();
        indicator.update(cx, |indicator, cx| indicator.advance_clock(1., cx));
    });
    draw_frame(&view, cx);
    let header = cx.debug_bounds("live-agent-indicator").unwrap();
    cx.update(|_, cx| {
        let indicator = view.read(cx).live_indicator.read(cx);
        assert_eq!(indicator.current_scene(), "success");
        assert!(
            !indicator.needs_frames(),
            "Completed work must stop its animation loop"
        );
        assert_eq!(indicator.painted_geometry().0, header);
    });
}

#[gpui::test]
fn launching_agent_shows_status_glyph_before_a_runtime_exists(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, _| app.starting = true);
    draw_frame(&view, cx);
    let bounds = cx.debug_bounds("live-status-indicator").unwrap();
    let caption = cx.debug_bounds("live-status-caption").unwrap();
    assert!(bounds.right() <= caption.left());
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(app.runs.is_empty());
        let indicator = app.live_indicator.read(cx);
        assert_eq!(indicator.current_scene(), "thinking");
        assert_eq!(indicator.painted_geometry().0, bounds);
        assert!(indicator.painted_geometry().1 > 0);
    });
}

#[gpui::test]
fn completion_click_opens_exact_prompt_diff_and_close_releases_it(cx: &mut TestAppContext) {
    use chat_interactions::{ChatInteraction, InteractionStatus};
    let (view, cx) = fixture(cx);
    let root = cx.update(|_, cx| view.read(cx).project.clone());
    std::fs::write(root.join("main.rs"), "fn main() { let x = 1; }\n").unwrap();
    let id = view.update(cx, |app, _| {
        app.checkpoint_repository
            .as_ref()
            .unwrap()
            .capture_before(
                &root,
                "fixture-session",
                "first",
                "First prompt",
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap()
    });
    std::fs::write(root.join("main.rs"), "fn main() { let x = 2; }\n").unwrap();
    view.update(cx, |app, _| {
        app.checkpoint_repository
            .as_ref()
            .unwrap()
            .seal(&id, &std::sync::atomic::AtomicBool::new(false))
            .unwrap();
        app.active = Some("fixture-session".into());
        app.rows = vec![
            Row {
                role: Some("user".into()),
                block: MessageBlock::Paragraph("Change x".into()),
            },
            Row {
                role: None,
                block: MessageBlock::Activity {
                    label: "Prompt changes".into(),
                    details: id.clone(),
                },
            },
            Row {
                role: Some("assistant".into()),
                block: MessageBlock::Interaction(ChatInteraction::Status {
                    title: "Completed".into(),
                    details: String::new(),
                    status: InteractionStatus::Completed,
                }),
            },
        ];
        app.list = ListState::new(3, ListAlignment::Top, px(200.));
    });
    draw_frame(&view, cx);
    click_selector(cx, "completion-label-2");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        draw_frame(&view, cx);
        if cx.debug_bounds("diff-lines").is_some() && cx.debug_bounds("completion-undo-2").is_some()
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(
            app.checkpoint_timeline.is_none(),
            "Completed opens a real native inspector, not the history popup"
        );
        assert_eq!(
            app.diff_view.as_ref().unwrap().read(cx).checkpoint_id(),
            Some(id.as_str())
        );
        assert!(!app.files_visible);
        assert!(!app.expanded_tools.contains(&2));
    });
    assert!(
        cx.debug_bounds("prompt-checkpoint-1").is_none(),
        "Undo lives on Completed, not a duplicate marker row"
    );
    click_selector(cx, "completion-undo-2");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        draw_frame(&view, cx);
        if cx.debug_bounds("confirm-prompt-restore").is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        std::fs::read_to_string(root.join("main.rs"))
            .unwrap()
            .contains("= 2"),
        "Review never mutates files"
    );
    click_selector(cx, "confirm-prompt-restore");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        draw_frame(&view, cx);
        if view.update(cx, |app, _| {
            app.checkpoint_busy == 0
                && app.checkpoint_actions.get(&id) == Some(&Some(checkpoints::Direction::Redo))
        }) {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        std::fs::read_to_string(root.join("main.rs"))
            .unwrap()
            .contains("= 1")
    );
    // The same Completed control changes to Redo; it still opens a review.
    click_selector(cx, "completion-undo-2");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        draw_frame(&view, cx);
        if cx.debug_bounds("confirm-prompt-restore").is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    click_selector(cx, "confirm-prompt-restore");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        draw_frame(&view, cx);
        if view.update(cx, |app, _| {
            app.checkpoint_busy == 0
                && app.checkpoint_actions.get(&id) == Some(&Some(checkpoints::Direction::Undo))
        }) {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        std::fs::read_to_string(root.join("main.rs"))
            .unwrap()
            .contains("= 2")
    );
    std::fs::write(root.join("main.rs"), "user’s newer edit\n").unwrap();
    click_selector(cx, "completion-undo-2");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        draw_frame(&view, cx);
        if cx.debug_bounds("confirm-prompt-restore").is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    click_selector(cx, "confirm-prompt-restore");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        draw_frame(&view, cx);
        if cx.update(|_, cx| {
            let app = view.read(cx);
            app.checkpoint_busy == 0
                && app
                    .diff_view
                    .as_ref()
                    .is_some_and(|review| review.read(cx).review_error().is_some())
        }) {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        std::fs::read_to_string(root.join("main.rs")).unwrap(),
        "user’s newer edit\n",
        "Undo must refuse to overwrite newer edits"
    );
    click_selector(cx, "close-changes");
    cx.run_until_parked();
    cx.update(|_, cx| {
        assert!(
            view.read(cx).diff_view.is_none(),
            "Closing releases decoded patches and cancellation guard"
        )
    });
    draw_frame(&view, cx);
}

fn wait_for_native_review(view: &Entity<Orbit>, cx: &mut VisualTestContext) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        draw_frame(view, cx);
        if cx.debug_bounds("diff-lines").is_some() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn completion_row() -> Row {
    Row {
        role: Some("assistant".into()),
        block: MessageBlock::Interaction(chat_interactions::ChatInteraction::Status {
            title: "Completed".into(),
            details: String::new(),
            status: chat_interactions::InteractionStatus::Completed,
        }),
    }
}
#[gpui::test]
fn native_completion_legacy_review_uses_reported_diff_or_labeled_workspace_fallback(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    let root = cx.update(|_, cx| view.read(cx).project.clone());
    let run = |args: &[&str]| {
        assert!(
            std::process::Command::new("/usr/bin/git")
                .arg("-C")
                .arg(&root)
                .args([
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid"
                ])
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    };
    run(&["init", "-q"]);
    std::fs::write(root.join("workspace.rs"), "old\n").unwrap();
    run(&["add", "."]);
    run(&["commit", "-qm", "Fixture"]);
    std::fs::write(root.join("workspace.rs"), "new current workspace edit\n").unwrap();
    view.update(cx, |app, _| {
        app.active = Some("fixture-session".into());
        app.rows = vec![
            Row {
                role: Some("user".into()),
                block: MessageBlock::Paragraph("Old prompt".into()),
            },
            completion_row(),
        ];
        app.list = ListState::new(2, ListAlignment::Top, px(200.));
    });
    draw_frame(&view, cx);
    click_selector(cx, "completion-label-1");
    wait_for_native_review(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        let review = app.diff_view.as_ref().unwrap().read(cx);
        assert!(review.is_workspace_fallback());
        assert_eq!(
            review.selected_key(),
            Some(std::path::Path::new("workspace.rs"))
        );
        assert!(review.checkpoint_action().is_none());
        assert!(app.checkpoint_timeline.is_none());
    });
    assert!(cx.debug_bounds("prompt-review-restore").is_none());
    // A real persisted native `turn/diff/updated` card takes precedence over
    // today's working tree. It provides review, never invented historical Undo.
    view.update(cx,|app,_| {
        app.rows.insert(1,Row {role:Some("assistant".into()),block:MessageBlock::Interaction(chat_interactions::ChatInteraction::Status {
            title:"Changes".into(),details:"diff --git a/reported.rs b/reported.rs\n--- a/reported.rs\n+++ b/reported.rs\n@@ -1 +1 @@\n-before\n+after\n".into(),status:chat_interactions::InteractionStatus::Working})});
        app.list=ListState::new(3,ListAlignment::Top,px(200.));
    });
    draw_frame(&view, cx);
    click_selector(cx, "completion-label-2");
    wait_for_native_review(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        let review = app.diff_view.as_ref().unwrap().read(cx);
        assert!(review.is_provider_review());
        assert_eq!(
            review.selected_key(),
            Some(std::path::Path::new("reported.rs"))
        );
        assert!(review.checkpoint_action().is_none());
    });
    assert!(cx.debug_bounds("prompt-review-restore").is_none());
    assert_eq!(
        std::fs::read_to_string(root.join("workspace.rs")).unwrap(),
        "new current workspace edit\n"
    );
}
#[gpui::test]
fn native_completion_keeps_painted_prompt_identity_after_history_prepend(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let (target, id, root) = view.update(cx, |app, _| {
        let root = app.project.clone();
        let repository = app.checkpoint_repository.as_ref().unwrap();
        std::fs::write(root.join("exact.rs"), "before\n").unwrap();
        let id = repository
            .capture_before(
                &root,
                "fixture-session",
                "exact",
                "Painted prompt",
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
        std::fs::write(root.join("exact.rs"), "after\n").unwrap();
        repository
            .seal(&id, &std::sync::atomic::AtomicBool::new(false))
            .unwrap();
        app.active = Some("fixture-session".into());
        app.rows = vec![
            Row {
                role: Some("user".into()),
                block: MessageBlock::Paragraph("Painted prompt".into()),
            },
            Row {
                role: None,
                block: MessageBlock::Activity {
                    label: "Prompt changes".into(),
                    details: id.clone(),
                },
            },
            completion_row(),
        ];
        let target = app.completion_target(2).unwrap();
        let earlier = vec![
            Row {
                role: Some("user".into()),
                block: MessageBlock::Paragraph("Earlier unrelated prompt".into()),
            },
            Row {
                role: None,
                block: MessageBlock::Activity {
                    label: "Prompt changes".into(),
                    details: uuid::Uuid::new_v4().to_string(),
                },
            },
            completion_row(),
        ];
        app.rows.splice(0..0, earlier);
        app.list = ListState::new(6, ListAlignment::Top, px(200.));
        (target, id, root)
    });
    view.update(cx, |app, cx| {
        app.open_completion_target(target.clone(), true, cx)
    });
    wait_for_native_review(&view, cx);
    cx.update(|_, cx| {
        assert_eq!(
            view.read(cx)
                .diff_view
                .as_ref()
                .unwrap()
                .read(cx)
                .checkpoint_id(),
            Some(id.as_str())
        )
    });
    assert!(cx.debug_bounds("confirm-prompt-restore").is_some());
    assert_eq!(
        std::fs::read_to_string(root.join("exact.rs")).unwrap(),
        "after\n"
    );
    // A stale painted handler must never cross into another chat.
    view.update(cx, |app, cx| {
        app.diff_view = None;
        app.active = Some("other-chat".into());
        app.open_completion_target(target, false, cx);
        assert!(app.diff_view.is_none());
    });
    assert_eq!(
        std::fs::read_to_string(root.join("exact.rs")).unwrap(),
        "after\n"
    );
}

#[gpui::test]
fn kai_tool_identity_and_terminal_state_survive_streaming_without_extra_history_players(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, _| {
        app.active = Some("fixture-session".into());
        let mut run = idle_run(String::new());
        run.status = "Using mcp__codex_apps__github/get_pull_request".into();
        app.runs.insert("fixture-session".into(), run);
    });
    draw_frame(&view, cx);
    assert_eq!(
        activity_caption("Using mcp__codex_apps__github/get_pull_request"),
        "Using · GitHub"
    );
    assert_eq!(
        activity_icon("mcp__codex_apps__github/get_pull_request"),
        "assets/kai/integrations/github.svg"
    );
    view.update(cx, |app, cx| {
        app.runs.get_mut("fixture-session").unwrap().handle =
            RunHandle::test_events(vec![RuntimeEvent::Failed("fixture failure".into())]);
        app.poll("fixture-session", cx);
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.live_indicator.read(cx).current_scene(), "failure");
        assert!(!app.live_indicator.read(cx).needs_frames());
        assert!(app.runs.is_empty());
    });
}

#[gpui::test]
fn slash_picker_filters_navigates_completes_and_dispatches_without_a_cli(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("/");
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("slash-picker").is_some());
    let first = cx.update(|_, cx| view.read(cx).selected_command().unwrap().to_owned());
    cx.simulate_keystrokes("down");
    let second = cx.update(|_, cx| view.read(cx).selected_command().unwrap().to_owned());
    assert_ne!(first, second);
    cx.simulate_keystrokes("up");
    cx.update(|_, cx| assert_eq!(view.read(cx).selected_command(), Some(first.as_str())));
    cx.simulate_input("mod");
    cx.simulate_keystrokes("tab");
    assert_eq!(composer_text(&view, cx), "/model ");
    cx.simulate_keystrokes("enter");
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(app.agent_picker.is_some());
        assert!(app.runs.is_empty());
    });
    assert!(composer_text(&view, cx).is_empty());
}

#[gpui::test]
fn unsupported_commands_do_not_become_paid_prompts_and_paths_remain_text(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("/unknown-command");
    cx.simulate_keystrokes("enter");
    assert_eq!(composer_text(&view, cx), "/unknown-command");
    cx.update(|_, cx| {
        assert!(view.read(cx).runs.is_empty());
        assert!(view.read(cx).notice.is_some());
    });
    cx.simulate_keystrokes("escape");
    cx.simulate_keystrokes("cmd-shift-a");
    cx.simulate_input("/Users/me/project.rs");
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).slash.is_none()));
    assert_eq!(composer_text(&view, cx), "/Users/me/project.rs");
}

#[gpui::test]
fn slash_escape_and_cursor_changes_keep_existing_panels_and_text(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| view.update(cx, |app, cx| app.toggle_files(window, cx)));
    cx.simulate_input("/status");
    cx.simulate_keystrokes("cmd-left");
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        assert!(view.read(cx).slash.is_none());
        assert!(view.read(cx).files_visible);
    });
    cx.simulate_keystrokes("cmd-right");
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).slash.is_some()));
    cx.simulate_keystrokes("escape");
    cx.update(|_, cx| {
        assert!(view.read(cx).slash.is_none());
        assert!(view.read(cx).files_visible);
    });
    assert_eq!(composer_text(&view, cx), "/status");
}

#[gpui::test]
fn inline_slash_completes_at_the_caret_and_keeps_the_surrounding_draft(cx:&mut TestAppContext){
    let(view,cx)=fixture(cx);
    cx.simulate_input("日本語 please /mod");cx.simulate_keystrokes("tab");
    assert_eq!(composer_text(&view,cx),"日本語 please /model ");
    cx.simulate_keystrokes("enter");draw_frame(&view,cx);
    cx.update(|_,cx|{assert!(view.read(cx).agent_picker.is_some());assert!(view.read(cx).runs.is_empty());});
    assert_eq!(composer_text(&view,cx),"日本語 please  ");
}

#[gpui::test]
fn real_provider_svg_rotates_in_place_during_loading_then_settles_and_stops(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.run_until_parked();
    view.update(cx, |app, cx| {
        app.motion = true;
        app.discovering = true;
        cx.notify();
    });
    draw_frame(&view, cx);
    let logo = cx.update(|_, cx| view.read(cx).loading_logo.clone());
    let bounds = cx.debug_bounds("loading-provider-logo").unwrap();
    assert_eq!(bounds.size, size(px(34.), px(34.)));
    assert_eq!(
        cx.debug_bounds("loading-provider-svg").unwrap().center(),
        bounds.center()
    );
    logo.update(cx, |logo, cx| {
        assert!(logo.needs_frames());
        logo.advance(0.3, cx);
        assert!(logo.angle() > 0.1);
    });
    draw_frame(&view, cx);
    assert_eq!(
        cx.debug_bounds("loading-provider-svg").unwrap().size,
        bounds.size
    );
    assert_eq!(
        cx.debug_bounds("loading-provider-svg").unwrap().center(),
        cx.debug_bounds("loading-provider-logo").unwrap().center()
    );
    view.update(cx, |app, cx| {
        app.discovering = false;
        cx.notify();
    });
    draw_frame(&view, cx);
    logo.update(cx, |logo, cx| {
        logo.advance(3., cx);
        assert_eq!(logo.angle(), 0.);
        assert!(!logo.needs_frames());
    });
    draw_frame(&view, cx);
    view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        app.loading = true;
        cx.notify();
    });
    draw_frame(&view, cx);
    assert_eq!(
        cx.debug_bounds("loading-provider-logo").unwrap().size,
        size(px(18.), px(18.))
    );
    assert_eq!(
        cx.debug_bounds("loading-provider-logo").unwrap().center(),
        cx.debug_bounds("live-agent-indicator").unwrap().center()
    );
    view.update(cx, |app, cx| {
        app.motion = false;
        cx.notify();
    });
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(!logo.read(cx).needs_frames()));
}

#[gpui::test]
fn requested_shortcuts_preserve_drafts_and_toggle_real_native_views(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.simulate_input("Preserve the draft");
    let before = cx.debug_bounds("chat-pane").unwrap();
    cx.simulate_keystrokes("cmd-e");
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(!view.read(cx).conversations_visible));
    assert!(
        (before.left() - cx.debug_bounds("chat-pane").unwrap().left() - px(260.)).abs() < px(1.)
    );
    cx.simulate_keystrokes("cmd-e cmd-f");
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        assert!(view.read(cx).conversations_visible);
        assert!(view.read(cx).files_visible);
    });
    cx.simulate_keystrokes("cmd-d");
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).diff_view.is_some()));
    cx.simulate_keystrokes("cmd-d cmd-s");
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).modal == Some(Modal::Search)));
    cx.simulate_input("Select this search text");
    cx.simulate_keystrokes("cmd-a cmd-c");
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().as_deref(),
        Some("Select this search text")
    );
    cx.update(|_, cx| assert!(view.read(cx).modal == Some(Modal::Search)));
    cx.simulate_keystrokes("escape cmd-a cmd-c");
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().as_deref(),
        Some("Preserve the draft")
    );
    cx.update(|window, cx| view.read(cx).focus.focus(window));
    cx.simulate_keystrokes("cmd-a");
    draw_frame(&view, cx);
    cx.update(|_, cx| assert!(view.read(cx).modal == Some(Modal::Settings)));
    cx.simulate_keystrokes("escape cmd-a cmd-c");
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().as_deref(),
        Some("Preserve the draft")
    );
    cx.update(|_, cx| view.update(cx, |app, _| app.active = Some("fixture-session".into())));
    cx.simulate_keystrokes("cmd-t");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(app.active.is_none());
        assert_eq!(app.drafts["fixture-session"], "Preserve the draft");
        assert!(app.runs.is_empty());
    });
}

#[gpui::test]
fn cli_catalog_is_scoped_bounded_and_native_commands_wait_between_turns(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.agent = AgentKind::Claude;
            app.sessions[0].agent = AgentKind::Claude;
            app.sessions[0].external_id = Some("claude-native".into());
            app.active = Some("fixture-session".into());
            app.native_commands.insert(
                (AgentKind::Claude, app.project.clone()),
                vec!["compact".into(), "project-skill".into()],
            );
            app.native_command_revision += 1;
        })
    });
    cx.simulate_input("/comp");
    draw_frame(&view, cx);
    cx.update(|_, cx| assert_eq!(view.read(cx).selected_command(), Some("compact")));
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.runs
                .insert("fixture-session".into(), idle_run(String::new()));
        })
    });
    cx.simulate_keystrokes("enter");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(!app.runs["fixture-session"].handle.is_cancelled());
        assert!(app.notice.as_deref().unwrap().contains("between turns"));
    });
    assert_eq!(composer_text(&view, cx), "/compact");
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.agent = AgentKind::Gemini;
            app.update_slash(window, cx);
        })
    });
    cx.simulate_keystrokes("cmd-shift-a");
    cx.simulate_input("/stats");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.selected_command(), Some("stats"));
        assert!(!app.native_command_available("compact"));
    });
}

#[gpui::test]
fn model_change_keeps_native_context_and_does_not_cancel_the_current_run(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            app.sessions[0].external_id = Some("same-native-thread".into());
            app.runs
                .insert("fixture-session".into(), idle_run("Still working".into()));
            app.choose_model(Some("next-model".into()), cx);
            assert_eq!(app.model.as_deref(), Some("next-model"));
            assert_eq!(
                app.sessions[0].external_id.as_deref(),
                Some("same-native-thread")
            );
            assert!(!app.runs["fixture-session"].handle.is_cancelled());
            assert!(app.notice.as_deref().unwrap().contains("next turn"));
        })
    });
}

#[gpui::test]
fn status_context_is_per_session_and_commands_do_not_stop_running_agents(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.active = Some("fixture-session".into());
            app.runs
                .insert("fixture-session".into(), idle_run(String::new()));
            app.remember_usage(
                "fixture-session".into(),
                usage::Usage {
                    input: Some(123),
                    output: Some(17),
                    ..Default::default()
                },
            );
            app.remember_usage(
                "another-session".into(),
                usage::Usage {
                    input: Some(999),
                    ..Default::default()
                },
            );
        })
    });
    cx.simulate_input("/status");
    cx.simulate_keystrokes("enter");
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("status-popover").is_some());
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.usage["fixture-session"].input, Some(123));
        assert_eq!(app.usage["another-session"].input, Some(999));
        assert!(!app.runs["fixture-session"].handle.is_cancelled());
    });
}

#[gpui::test]
fn handoff_selection_never_cancels_active_work_and_explicit_send_queues_target(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.agents.clear();
            controls_fixture(app, AgentKind::Claude, 1);
            app.active = Some("fixture-session".into());
            app.sessions[0].external_id = Some("source-native".into());
            app.runs
                .insert("fixture-session".into(), idle_run("Partial work".into()));
            app.composer
                .update(cx, |input, cx| input.set_text("One next instruction", cx));
            app.request_handoff(AgentKind::Claude, window, cx);
            app.advance_handoff(window, cx);
            assert!(!app.runs["fixture-session"].handle.is_cancelled());
            assert!(app.handoff_pending.is_none());
            assert!(!app.starting);
            assert_eq!(app.sessions[0].agent, AgentKind::Codex);
            assert_eq!(
                app.sessions[0].external_id.as_deref(),
                Some("source-native")
            );
            assert_eq!(app.agent, AgentKind::Claude);
            app.send(window, cx);
            assert_eq!(app.prompt_queue.len(), 1);
            assert_eq!(app.prompt_queue[0].agent, AgentKind::Claude);
            assert_eq!(app.prompt_queue[0].text, "One next instruction");
            assert!(!app.runs["fixture-session"].handle.is_cancelled());
            assert!(app.handoff_pending.is_none());
            assert!(app.composer.read(cx).text().is_empty());
            // Completion of the source admits the queued target submission;
            // async preparation is not treated as failed queue admission.
            app.runs.remove("fixture-session");
            app.advance_queue(window, cx);
            assert!(app.handoff_pending.is_some());
            assert!(!app.prompt_queue[0].held);
            assert_eq!(
                app.queue_dispatch.as_deref(),
                Some(app.prompt_queue[0].id.as_str())
            );
            let token = app.handoff_pending.as_ref().unwrap().token;
            app.receive(
                Event::Handoff(token, Err("queued context failure".into())),
                cx,
            );
            assert!(app.prompt_queue[0].held && app.queue_dispatch.is_none());
            app.advance_queue(window, cx);
            assert!(app.handoff_pending.is_none());
            assert_eq!(app.composer.read(cx).text(), "One next instruction");
        })
    });
}

#[gpui::test]
fn failed_handoff_preparation_preserves_newer_draft_and_source_binding(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let attachment = image_fixture(&view, cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.agents.clear();
            controls_fixture(app, AgentKind::Claude, 0);
            app.active = Some("fixture-session".into());
            app.sessions[0].external_id = Some("source-native".into());
            app.attachments.push(attachment.clone());
            app.composer
                .update(cx, |input, cx| input.set_text("Submitted instruction", cx));
            app.request_handoff(AgentKind::Claude, window, cx);
            assert!(app.handoff_pending.is_none());
            app.send(window, cx);
            let token = app.handoff_pending.as_ref().unwrap().token;
            app.composer
                .update(cx, |input, cx| input.set_text("Newer unsent draft", cx));
            app.receive(
                Event::Handoff(token, Err("fixture read failure".into())),
                cx,
            );
            app.advance_handoff(window, cx);
            assert!(app.handoff_pending.is_none());
            assert!(!app.starting && app.runs.is_empty());
            assert_eq!(app.sessions[0].agent, AgentKind::Codex);
            assert_eq!(
                app.sessions[0].external_id.as_deref(),
                Some("source-native")
            );
            assert_eq!(app.attachments[0].id, attachment.id);
            assert_eq!(app.composer.read(cx).text(), "Newer unsent draft");
            assert!(
                app.notice
                    .as_deref()
                    .unwrap()
                    .contains("fixture read failure")
            );
        })
    });
}

#[gpui::test]
fn handoff_binding_rejection_preserves_source_model_usage_and_current_draft(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.agents.clear();
            controls_fixture(app, AgentKind::Claude, 0);
            app.active = Some("fixture-session".into());
            app.sessions[0].external_id = Some("source-native".into());
            app.session_models
                .insert("fixture-session".into(), Some("source-model".into()));
            app.remember_usage(
                "fixture-session".into(),
                usage::Usage {
                    input: Some(123),
                    ..Default::default()
                },
            );
            app.engine
                .save(app.sessions[0].clone(), "user", "Original task".into());
            app.composer
                .update(cx, |input, cx| input.set_text("Submitted task", cx));
            app.request_handoff(AgentKind::Claude, window, cx);
            app.send(window, cx);
            let token = app.handoff_pending.as_ref().unwrap().token;
            let mut packet = handoff::Packet::build(
                &app.sessions[0],
                None,
                Some("Original task"),
                vec![],
                false,
                false,
            );
            packet.sources.last_mut().unwrap().native_id = Some("stale-source".into());
            app.receive(Event::Handoff(token, Ok(packet)), cx);
            app.advance_handoff(window, cx);
            app.composer
                .update(cx, |input, cx| input.set_text("Latest unsent draft", cx));
        })
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                while let Ok(event) = app.engine.events.try_recv() {
                    if matches!(event, Event::Handoff(_, _) | Event::AgentSwitched(_, _)) {
                        app.receive(event, cx);
                    }
                }
                app.advance_handoff(window, cx);
            })
        });
        if cx.update(|_, cx| view.read(cx).handoff_pending.is_none()) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Atomic handoff rejection did not arrive"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.sessions[0].agent, AgentKind::Codex);
        assert_eq!(
            app.sessions[0].external_id.as_deref(),
            Some("source-native")
        );
        assert_eq!(
            app.session_models["fixture-session"].as_deref(),
            Some("source-model")
        );
        assert_eq!(app.usage["fixture-session"].input, Some(123));
        assert!(!app.starting && app.runs.is_empty());
        assert!(
            app.notice
                .as_deref()
                .unwrap()
                .contains("Could not switch agents")
        );
    });
    assert_eq!(composer_text(&view, cx), "Latest unsent draft");
}

#[gpui::test]
fn handoff_stop_cancels_only_the_pending_send_and_late_context_cannot_launch(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.agents.clear();
            controls_fixture(app, AgentKind::Claude, 0);
            app.active = Some("fixture-session".into());
            app.composer
                .update(cx, |input, cx| input.set_text("Submitted message", cx));
            app.request_handoff(AgentKind::Claude, window, cx);
            app.send(window, cx);
            let token = app.handoff_pending.as_ref().unwrap().token;
            app.composer
                .update(cx, |input, cx| input.set_text("New unsent message", cx));
            // Enter must not submit newer typing or cancel the existing Send.
            app.send(window, cx);
            assert!(app.handoff_pending.is_some());
            assert!(app.cancel_send_handoff(cx));
            let packet = handoff::Packet::build(&app.sessions[0], None, None, vec![], false, false);
            app.receive(Event::Handoff(token, Ok(packet)), cx);
            app.advance_handoff(window, cx);
            assert!(app.handoff_pending.is_none());
            assert!(!app.starting && app.runs.is_empty());
            assert_eq!(app.sessions[0].agent, AgentKind::Codex);
            assert_eq!(app.composer.read(cx).text(), "New unsent message");
        })
    });
}

#[gpui::test]
fn failed_handoff_never_puts_internal_context_in_the_composer(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|_, cx| view.update(cx, |app, cx| {
        app.active = Some("fixture-session".into());
        let mut run = failed_run(AgentKind::Claude, app.project.clone(), "Credit limit");
        run.submitted_prompt = "Only my actual prompt".into();
        run.retry.as_mut().unwrap().prompt = "<code_handoff_context>PRIVATE TRANSFER</code_handoff_context>\nOnly my actual prompt".into();
        app.runs.insert("fixture-session".into(), run);
        app.poll("fixture-session", cx);
        assert_eq!(app.drafts["fixture-session"], "Only my actual prompt");
    }));
    assert_eq!(composer_text(&view, cx), "Only my actual prompt");
}

#[gpui::test]
fn handoff_send_combines_one_snapshot_with_context_images_and_preserves_newer_typing(
    cx: &mut TestAppContext,
) {
    snapshot_handoff_round_trip(cx, false);
}
#[gpui::test]
fn handoff_send_after_lazy_history_load_preserves_the_original_submission(cx: &mut TestAppContext) {
    snapshot_handoff_round_trip(cx, true);
}
fn snapshot_handoff_round_trip(cx: &mut TestAppContext, older_history: bool) {
    use std::os::unix::fs::PermissionsExt;
    let (view, cx) = fixture(cx);
    let submitted_image = image_fixture(&view, cx);
    let newer_image = image_fixture(&view, cx);
    let (source, executable, captured, starts) = cx.update(|_, cx| {
        let app = view.read(cx);
        (
            app.sessions[0].clone(),
            app.project.join("destination.py"),
            app.project.join("destination-input.json"),
            app.project.join("destination-starts.txt"),
        )
    });
    std::fs::write(&executable, r#"#!/usr/bin/python3
import json,sys
def send(v): print(json.dumps(v),flush=True)
open('destination-starts.txt','a').write('start\n')
v=json.loads(sys.stdin.readline())
assert v['type']=='control_request' and v['request']['subtype']=='initialize',v
send({'type':'control_response','response':{'subtype':'success','request_id':'code-initialize','response':{'commands':[]}}})
v=json.loads(sys.stdin.readline())
assert v['type']=='user',v
open('destination-input.json','w').write(json.dumps({'input':v,'args':sys.argv[1:]}))
send({'type':'system','subtype':'init','session_id':'destination-native','slash_commands':[]})
send({'type':'assistant','message':{'id':'reply','content':[{'type':'text','text':'Continuing the marker task'}]}})
send({'type':'result','subtype':'success','is_error':False,'result':'Continuing the marker task'})
"#).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let original_reference = context_refs::Reference {
        id: "attached-reference".into(),
        title: "Saved reference".into(),
        project: "/other".into(),
        provider: "Codex".into(),
        excerpt: "FROZEN ATTACHED CONTEXT".into(),
        older_omitted: true,
    };
    let mut newer_reference = original_reference.clone();
    newer_reference.excerpt = "NEW UNSENT CONTEXT".into();
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.agents.clear();
            app.agents.push(Agent {
                kind: AgentKind::Claude,
                name: "Fixture Claude".into(),
                executable: Some(executable),
                available: true,
            });
            app.active = Some(source.id.clone());
            app.sessions[0].external_id = Some("source-native-stays".into());
            app.engine.save(
                app.sessions[0].clone(),
                "user",
                "Original objective MARKER-742".into(),
            );
            app.engine.save(
                app.sessions[0].clone(),
                "assistant",
                "A verified earlier decision".into(),
            );
            app.open_tabs = vec![source.id.clone()];
            app.rows = vec![Row {
                role: Some("assistant".into()),
                block: MessageBlock::Paragraph("Earlier visible answer".into()),
            }];
            app.list.reset(app.rows.len());
            app.attachments.push(submitted_image.clone());
            app.chat_context
                .insert(source.id.clone(), vec![original_reference.clone()]);
            app.composer.update(cx, |input, cx| {
                input.set_text("ONE EXPLICIT USER INSTRUCTION", cx)
            });
            app.request_handoff(AgentKind::Claude, window, cx);
            app.model = Some("frozen-model".into());
            assert_eq!(app.agent, AgentKind::Claude);
            assert_eq!(app.sessions[0].agent, AgentKind::Codex);
            assert!(app.handoff_pending.is_none() && !app.starting && app.runs.is_empty());
            assert!(!captured.exists() && !starts.exists());
            app.history_offset = usize::from(older_history);
            app.send(window, cx);
            if older_history {
                assert!(app.send_after_load.is_some());
                app.receive(
                    Event::Model(
                        source.id.clone(),
                        app.generation,
                        Some("stale-source-model".into()),
                        None,
                        None,
                    ),
                    cx,
                );
                assert_eq!(app.model.as_deref(), Some("frozen-model"));
            } else {
                assert!(app.handoff_pending.is_some());
            }
            assert!(
                !starts.exists(),
                "Selection/preparation must not invoke the destination"
            );
            // Every mutable composer input changes after Send. Only the exact
            // submitted snapshot may enter the target invocation.
            app.composer.update(cx, |input, cx| {
                input.set_text("NEXT DRAFT MUST NEVER SEND", cx)
            });
            app.attachments = vec![newer_image.clone()];
            app.chat_context
                .insert(source.id.clone(), vec![newer_reference.clone()]);
            app.model = Some("later-model".into());
            app.allow_edits = true;
        })
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        cx.run_until_parked();
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                while let Ok(event) = app.engine.events.try_recv() {
                    if matches!(
                        event,
                        Event::Handoff(_, _)
                            | Event::Parsed(_, _)
                            | Event::AgentSwitched(_, _)
                            | Event::Loaded(_, _, _, _, _)
                            | Event::Model(_, _, _, _, _)
                    ) {
                        app.receive(event, cx);
                    }
                }
                app.advance_handoff(window, cx);
            })
        });
        if cx.update(|_, cx| {
            let app = view.read(cx);
            app.handoff_pending.is_none()
                && app.send_after_load.is_none()
                && !app.starting
                && app.runs.is_empty()
                && app.finalizing.is_empty()
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Explicit handoff Send did not finish"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&captured).unwrap()).unwrap();
    let content = value["input"]["message"]["content"].as_array().unwrap();
    let text = content
        .iter()
        .filter_map(|item| item["text"].as_str())
        .collect::<String>();
    assert_eq!(text.matches("ONE EXPLICIT USER INSTRUCTION").count(), 1);
    assert_eq!(text.matches("<code_handoff_context>").count(), 1);
    assert!(
        text.contains("Original objective MARKER-742")
            && text.contains("verified earlier decision")
    );
    assert!(text.contains("FROZEN ATTACHED CONTEXT"));
    assert!(!text.contains("NEXT DRAFT MUST NEVER SEND") && !text.contains("NEW UNSENT CONTEXT"));
    assert_eq!(
        content
            .iter()
            .filter(|item| item["type"] == "image")
            .count(),
        1
    );
    let args = value["args"].as_array().unwrap();
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "--model" && pair[1] == "frozen-model")
    );
    assert!(
        args.windows(2)
            .any(|pair| pair[0] == "--permission-mode" && pair[1] == "plan")
    );
    assert_eq!(std::fs::read_to_string(&starts).unwrap().lines().count(), 1);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert_eq!(app.active.as_deref(), Some(source.id.as_str()));
        assert_eq!(app.sessions.len(), 1);
        assert_eq!(app.sessions[0].agent, AgentKind::Claude);
        assert_eq!(
            app.sessions[0].external_id.as_deref(),
            Some("destination-native")
        );
        assert_eq!(app.open_tabs.as_slice(), std::slice::from_ref(&source.id));
        if older_history {
            assert!(
                app.rows
                    .iter()
                    .any(|row| row.role.as_deref() == Some("assistant:codex"))
            );
        } else {
            assert_eq!(app.rows[0].role.as_deref(), Some("assistant:codex"));
        }
        assert_eq!(app.attachments.len(), 1);
        assert_eq!(app.attachments[0].id, newer_image.id);
        assert_eq!(app.chat_context[&source.id], vec![newer_reference]);
        assert_eq!(app.model.as_deref(), Some("later-model"));
        assert_eq!(
            app.session_models[&source.id].as_deref(),
            Some("frozen-model")
        );
    });
    assert_eq!(composer_text(&view, cx), "NEXT DRAFT MUST NEVER SEND");
}

#[gpui::test]
fn large_cli_command_catalog_is_virtualized_and_keyboard_scrolls_to_the_last_entry(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.update(|_, cx| {
        view.update(cx, |app, _| {
            app.agent = AgentKind::Claude;
            app.native_commands.insert(
                (AgentKind::Claude, app.project.clone()),
                (0..128)
                    .map(|i| std::sync::Arc::<str>::from(format!("skill-{i:03}")))
                    .collect(),
            );
            app.native_command_revision += 1;
        })
    });
    cx.simulate_input("/skill-");
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("slash-command-skill-000").is_some());
    assert!(cx.debug_bounds("slash-command-skill-064").is_none());
    assert!(cx.debug_bounds("slash-command-skill-127").is_none());
    cx.simulate_keystrokes("up");
    draw_frame(&view, cx);
    cx.update(|_, cx| assert_eq!(view.read(cx).selected_command(), Some("skill-127")));
    assert!(cx.debug_bounds("slash-command-skill-127").is_some());
    assert!(cx.debug_bounds("slash-command-skill-064").is_none());
    assert_eq!(composer_text(&view, cx), "/skill-");
}

#[gpui::test]
fn expanded_subagent_reports_scroll_inside_their_card(cx: &mut TestAppContext) {
    use chat_interactions::{ChatInteraction, InteractionStatus};
    let (view, cx) = fixture(cx);
    view.update(cx, |app, _| {
        app.active = Some("fixture-session".into());
        app.rows = vec![Row {
            role: Some("assistant:codex".into()),
            block: MessageBlock::Interaction(ChatInteraction::Delegation {
                title: "Parser review".into(),
                details: (0..60)
                    .map(|i| format!("Finding {i}: review output remains readable.\n"))
                    .collect(),
                status: InteractionStatus::Completed,
                agent: Some("Reviewer".into()),
            }),
        }];
        app.list.reset(app.rows.len());
    });
    draw_frame(&view, cx);
    click_selector(cx, "interaction-0");
    draw_frame(&view, cx);
    let viewport = cx.debug_bounds("interaction-detail-0").unwrap();
    assert!(viewport.size.height <= px(240.));
    let before = cx.debug_bounds("interaction-detail-text-0").unwrap();
    cx.simulate_event(ScrollWheelEvent {
        position: viewport.center(),
        delta: ScrollDelta::Pixels(point(px(0.), px(-300.))),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    draw_frame(&view, cx);
    let after = cx.debug_bounds("interaction-detail-text-0").unwrap();
    assert!(
        after.top() < before.top(),
        "The full report must scroll instead of being clipped"
    );
    assert_eq!(cx.debug_bounds("interaction-detail-0").unwrap(), viewport);
    assert!(cx.debug_bounds("interaction-0").is_some());
}

#[gpui::test]
fn repeated_question_ids_choose_only_the_clicked_question(cx: &mut TestAppContext) {
    use chat_interactions::{ChatInteraction, QuestionCard, QuestionOption};
    let (view, cx) = fixture(cx);
    let question = QuestionCard {
        id: String::new(),
        header: "Choice".into(),
        prompt: "Which approach?".into(),
        options: vec![QuestionOption {
            label: "Native".into(),
            description: String::new(),
        }],
        answer: None,
        historical: true,
    };
    view.update(cx, |app, _| {
        app.active = Some("fixture-session".into());
        app.rows = (0..2)
            .map(|_| Row {
                role: None,
                block: MessageBlock::Interaction(ChatInteraction::Question(question.clone())),
            })
            .collect();
        app.list.reset(app.rows.len());
    });
    draw_frame(&view, cx);
    click_selector(cx, "question-option-0-0");
    draw_frame(&view, cx);
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(
            app.question_choices
                .contains_key(&chat_interactions::question_choice_key(&question, 0))
        );
        assert!(
            !app.question_choices
                .contains_key(&chat_interactions::question_choice_key(&question, 1))
        );
        assert!(app.runs.is_empty());
    });
    click_selector(cx, "question-option-1-0");
    draw_frame(&view, cx);
    cx.update(|_, cx| assert_eq!(view.read(cx).question_choices.len(), 2));
}

#[gpui::test]
fn historical_speaker_card_keeps_provider_after_agent_switch(cx: &mut TestAppContext) {
    use chat_interactions::{ChatInteraction, Speaker};
    let (view, cx) = fixture(cx);
    view.update(cx, |app, _| {
        app.active = Some("fixture-session".into());
        app.agent = AgentKind::Gemini;
        app.rows = vec![Row {
            role: Some("assistant:claude".into()),
            block: MessageBlock::Interaction(ChatInteraction::Speaker {
                speaker: Speaker::Assistant,
                text: "The previous agent's spoken response.".into(),
            }),
        }];
        app.list.reset(app.rows.len());
    });
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("interaction-speaker-0-claude").is_some());
    assert!(cx.debug_bounds("interaction-speaker-0-gemini").is_none());
    view.update(cx, |app, _| app.agent = AgentKind::OpenCode);
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("interaction-speaker-0-claude").is_some());
    assert!(cx.debug_bounds("interaction-speaker-0-opencode").is_none());
}

#[gpui::test]
fn sandbox_terminal_and_compact_use_protocol_actions_without_model_prompts(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    let project = cx.update(|_, cx| view.read(cx).project.clone());
    let executable = project.join("native-actions");
    let prompt = project.join("unexpected-prompt");
    let calls = project.join("native-actions.json");
    codex_protocol_fixture(&executable, &prompt, &calls, false);
    view.update(cx, |app, _| {
        app.agents[0].executable = Some(executable);
        app.agents[0].available = true;
        app.active = Some("fixture-session".into());
    });
    for command in [
        "/terminal printf test",
        "/compact",
        "/review base main",
        "/goal set Verify actions",
        "/goal",
        "/goal clear",
    ] {
        if !command.starts_with("/terminal") {
            view.update(cx, |app, _| {
                app.sessions[0].external_id = Some("12345678-1234-4234-8234-123456789012".into())
            });
        }
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.composer
                    .update(cx, |input, cx| input.set_text(command, cx));
                app.send(window, cx);
            })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            let done = view.update(cx, |app, cx| {
                while let Ok(event) = app.engine.events.try_recv() {
                    if matches!(event, Event::Parsed(_, _)) {
                        app.receive(event, cx);
                    }
                }
                !app.starting
                    && app.runs.is_empty()
                    && app.finalizing.is_empty()
                    && app.settling.is_empty()
            });
            if done {
                break;
            }
            assert!(Instant::now() < deadline, "Native action did not complete");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !prompt.exists(),
            "Native actions must use their RPC instead of a model prompt"
        );
        let call: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&calls).unwrap()).unwrap();
        if command.starts_with("/terminal") {
            assert_eq!(call["method"], "command/exec");
            assert_eq!(
                call["params"]["command"],
                serde_json::json!(["/bin/zsh", "-c", "printf test"])
            );
            assert_eq!(call["params"]["sandboxPolicy"]["type"], "readOnly");
            assert_eq!(call["params"]["sandboxPolicy"]["networkAccess"], false);
            cx.update(|_, cx| assert!(view.read(cx).rows.iter().any(|row| matches!(&row.block, MessageBlock::Code {text,..} if text.contains("SANDBOX_OK")))));
        } else if command == "/compact" {
            assert_eq!(call["method"], "thread/compact/start");
        } else if command.starts_with("/review") {
            assert_eq!(call["method"], "review/start");
            assert_eq!(call["params"]["delivery"], "inline");
            assert_eq!(call["params"]["target"]["branch"], "main");
        } else {
            assert_eq!(
                call["method"],
                if command == "/goal" {
                    "thread/goal/get"
                } else if command == "/goal clear" {
                    "thread/goal/clear"
                } else {
                    "thread/goal/set"
                }
            );
        }
    }
}

#[gpui::test]
fn rejected_native_action_preserves_its_arguments_and_does_not_start_a_run(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    for command in [
        "/review base main",
        "/goal set Keep this objective",
        "/review commit --all",
    ] {
        cx.update(|window, cx| {
            view.update(cx, |app, cx| {
                app.active = None;
                app.composer
                    .update(cx, |input, cx| input.set_text(command, cx));
                app.send(window, cx);
            })
        });
        assert_eq!(composer_text(&view, cx), command);
        view.update(cx, |app, _| assert!(app.runs.is_empty() && !app.starting));
    }
}

#[gpui::test]
fn normal_quit_cancels_a_launch_that_returns_after_quit_started(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    let database = cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.active = Some("fixture-session".into());
            app.starting = true;
            cx.spawn(async move |this, cx| {
                Timer::after(Duration::from_millis(40)).await;
                let _ = this.update(cx, |app, cx| {
                    let mut run = idle_run(String::new());
                    run.handle = RunHandle::test_events(vec![RuntimeEvent::Text(
                        "Late launch output".into(),
                    )]);
                    app.runs.insert("fixture-session".into(), run);
                    app.starting = false;
                    cx.notify();
                });
            })
            .detach();
            app.quit_after_save(window, cx);
            app.controls_home
                .parent()
                .unwrap()
                .join("data/history.sqlite")
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        cx.run_until_parked();
        let text = rusqlite::Connection::open(&database).ok().and_then(|db| db.query_row("SELECT text FROM messages WHERE session_id='fixture-session' ORDER BY id DESC LIMIT 1", [], |row| row.get::<_, String>(0)).ok());
        if let Some(text) = text.filter(|text| text.contains("Late launch output")) {
            assert!(
                text.contains("stopped"),
                "The late handle must be canceled before final save"
            );
            cx.update(|_, cx| {
                assert!(view.read(cx).runs.is_empty());
                assert!(!view.read(cx).starting);
                assert!(view.read(cx).notice.is_none());
            });
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Quit did not wait for the in-flight launch"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui::test]
fn checkpoint_work_blocks_new_prompts_and_terminals_without_consuming_drafts(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.checkpoint_busy = 1;
            app.composer
                .update(cx, |input, cx| input.set_text("Keep this request", cx));
            app.send(window, cx);
            assert_eq!(app.composer.read(cx).text(), "Keep this request");
            assert!(!app.starting && app.runs.is_empty());
            app.toggle_terminal(window, cx);
            assert!(app.terminal_panel.is_none() && !app.terminal_visible);
            app.run_native_terminal(window, cx);
            assert_eq!(app.composer.read(cx).text(), "Keep this request");
            assert!(!app.starting && app.runs.is_empty());
        });
    });
}

#[gpui::test]
fn pending_team_launches_reserve_capacity_against_native_commands(cx: &mut TestAppContext) {
    let (view, cx) = fixture(cx);
    cx.update(|window, cx| {
        view.update(cx, |app, cx| {
            app.active = Some(app.sessions[0].id.clone());
            app.sessions[0].external_id = Some(uuid::Uuid::new_v4().to_string());
            app.team_starting
                .extend((0..4).map(|i| format!("starting-{i}")));
            app.composer
                .update(cx, |input, cx| input.set_text("printf kept", cx));
            let rows = app.rows.len();
            app.run_native_terminal(window, cx);
            assert_eq!(app.composer.read(cx).text(), "printf kept");
            assert_eq!(app.rows.len(), rows);
            assert!(!app.starting && app.runs.is_empty());
            assert!(app.notice.as_deref().unwrap().contains("Wait"));
            app.compact_native_session(window, cx);
            assert!(!app.starting && app.runs.is_empty());
            assert_eq!(app.composer.read(cx).text(), "printf kept");
        });
    });
}

#[gpui::test]
fn fn_control_routes_from_composer_stops_dictation_and_keeps_pending_words(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        app.starting = true; // A repeated shortcut must not start real microphone capture.
        app.composer.update(cx, |input, cx| {
            input.set_text("Draft", cx);
            input.begin_dictation(cx);
        });
        app.speech = Some(speech::SpeechHandle::test_pending("spoken 日本語"));
        app.dictation_status = Some("Listening on device");
    });
    draw_frame(&view, cx);
    let ctrl = Modifiers {
        control: true,
        ..Modifiers::none()
    };
    let chord = Modifiers {
        function: true,
        ..ctrl
    };
    cx.simulate_modifiers_change(ctrl);
    view.update(cx, |app, _| assert!(app.speech.is_some()));
    cx.simulate_modifiers_change(chord);
    view.update(cx, |app, cx| {
        assert!(app.speech.is_none());
        assert!(app.dictation_status.is_none());
        assert!(app.dictation_hotkey_down);
        assert_eq!(app.composer.read(cx).text(), "Draft spoken 日本語");
    });
    // Render/repeated flags cannot toggle again, and modifier release isn't Stop.
    draw_frame(&view, cx);
    cx.simulate_modifiers_change(chord);
    view.update(cx, |app, _| assert!(app.dictation_hotkey_down));
    cx.simulate_modifiers_change(ctrl);
    view.update(cx, |app, _| assert!(!app.dictation_hotkey_down));
    cx.simulate_modifiers_change(chord);
    view.update(cx, |app, _| assert!(app.dictation_hotkey_down));
    view.update(cx, |app, cx| {
        assert_eq!(app.composer.read(cx).text(), "Draft spoken 日本語")
    });
}

#[gpui::test]
fn composer_permissions_open_real_approval_controls_and_save_the_selected_mode(
    cx: &mut TestAppContext,
) {
    let (view, cx) = fixture(cx);
    view.update(cx, |app, cx| {
        app.agent = AgentKind::Codex;
        app.allow_edits = true;
        cx.notify();
    });
    draw_frame(&view, cx);
    let position = cx
        .debug_bounds("mode")
        .expect("composer mode control")
        .center();
    cx.simulate_click(position, Modifiers::none());
    draw_frame(&view, cx);
    assert!(cx.debug_bounds("permissions-popover").is_some());
    click_selector(cx, "codex-approval-auto-review");
    view.update(cx, |app, cx| {
        assert!(app.allow_edits);
        assert_eq!(app.codex_approvals, runtime::CodexApprovals::AutoReview);
        let saved = app.workspace_snapshot(cx);
        assert_eq!(saved.codex_approvals, runtime::CodexApprovals::AutoReview);
        let restored: workspace_state::WorkspaceState =
            serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(
            restored.codex_approvals,
            runtime::CodexApprovals::AutoReview
        );
    });
    cx.update(|window, cx| {
        view.update(cx, |app, cx| app.show_modal(Modal::Permissions, window, cx))
    });
    draw_frame(&view, cx);
    click_selector(cx, "codex-approval-never");
    view.update(cx, |app, _| {
        assert_eq!(app.codex_approvals, runtime::CodexApprovals::Never);
        assert!(app.allow_edits);
        let old: workspace_state::WorkspaceState = serde_json::from_str("{}").unwrap();
        assert_eq!(old.codex_approvals, runtime::CodexApprovals::Ask);
        assert_eq!(old.codex_access,runtime::CodexAccess::Workspace);
        assert_eq!(app.codex_access,runtime::CodexAccess::Workspace);
    });
    cx.update(|window,cx|view.update(cx,|app,cx|app.show_modal(Modal::Permissions,window,cx)));
    draw_frame(&view,cx);click_selector(cx,"workspace-access-2");
    view.update(cx,|app,cx|{
        assert!(app.allow_edits);assert_eq!(app.codex_access,runtime::CodexAccess::Full);
        assert_eq!(app.codex_approvals,runtime::CodexApprovals::Never);
        let restored:workspace_state::WorkspaceState=serde_json::from_slice(&serde_json::to_vec(&app.workspace_snapshot(cx)).unwrap()).unwrap();
        assert_eq!(restored.codex_access,runtime::CodexAccess::Full);
    });
}

#[gpui::test]
fn pending_launch_belongs_to_its_chat_and_keeps_its_prompt_on_reload(cx:&mut TestAppContext){
    let(view,cx)=fixture(cx);let cancel=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    view.update_in(cx,|app,window,cx|{
        app.active=Some("fixture-session".into());app.loading=false;app.runs.clear();
        app.launches.insert("fixture-session".into(),chat_timing::Launch{caption:"Preparing workspace…",preview:vec![Row{role:Some("user".into()),block:MessageBlock::Paragraph("Keep submitted prompt".into())}],cancel:cancel.clone(),started:Instant::now(),key:"launch-one".into()});app.starting=true;
        assert!(app.starting_current());assert!(!app.unified_send_ready());
        app.new_conversation(window,cx);
        assert!(!app.starting_current());assert!(app.unified_send_ready());
        assert_eq!(app.unified_snapshot(cx)["sessionActivity"]["scene"],"off");
        assert!(!app.cancel_send_handoff(cx));assert!(!cancel.load(std::sync::atomic::Ordering::Acquire));
        app.active=Some("fixture-session".into());
        app.receive(Event::Loaded("fixture-session".into(),app.generation,vec![],false,false),cx);
        assert!(app.rows.iter().any(|r|matches!(&r.block,MessageBlock::Paragraph(t)if t=="Keep submitted prompt")));
        assert!(app.starting_current());assert!(app.cancel_send_handoff(cx));assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
    });
}

#[gpui::test]
fn timestamp_swipe_reveals_real_turn_time_and_preserves_completed_duration(cx:&mut TestAppContext){
    let(view,cx)=fixture(cx);let checkpoint=uuid::Uuid::new_v4().to_string();
    view.update(cx,|app,cx|{
        let now=chrono::Utc::now().timestamp_millis();app.active=Some("fixture-session".into());app.loading=false;
        app.rows=vec![Row{role:Some("user".into()),block:MessageBlock::Paragraph("Work on this".into())},Row{role:None,block:MessageBlock::Activity{label:"Prompt changes".into(),details:checkpoint.clone()}},Row{role:Some("assistant:codex".into()),block:MessageBlock::Paragraph("Ready".into())}];
        app.turn_timings=vec![chat_timing::Record{key:uuid::Uuid::new_v4().to_string(),session:"fixture-session".into(),checkpoint:Some(checkpoint.clone()),submitted_ms:now-10_000,first_output_ms:Some(now-5000),finished_ms:Some(now-1000),elapsed_ms:Some(9000),outcome:Some("completed".into())}];
        app.list.reset(app.rows.len());cx.notify();
    });draw_frame(&view,cx);
    assert!(cx.debug_bounds("turn-duration-2").is_some(),"Completed response lost its small duration footer");
    let position=cx.debug_bounds("transcript-row-2").unwrap().center();
    cx.simulate_event(ScrollWheelEvent{position,delta:ScrollDelta::Pixels(point(px(-90.),px(0.))),modifiers:Modifiers::none(),touch_phase:TouchPhase::Moved});draw_frame(&view,cx);
    cx.update(|_,cx|{let app=view.read(cx);assert!(app.timestamp_peek>0.9,"The actual transcript list consumed the timestamp swipe");assert_eq!(app.workspace_snapshot(cx).turn_timings[0].elapsed_ms,Some(9000));});
    cx.simulate_event(ScrollWheelEvent{position,delta:ScrollDelta::Pixels(point(px(0.),px(0.))),modifiers:Modifiers::none(),touch_phase:TouchPhase::Ended});
    cx.update(|_,cx|assert!(view.read(cx).timestamp_released.is_some(),"Releasing the swipe did not return the timestamp rail"));
}

#[gpui::test]
fn model_search_selects_the_filtered_native_model_without_losing_the_draft(cx:&mut TestAppContext){
    let(view,cx)=fixture(cx);
    view.update_in(cx,|app,window,cx|{
        app.loading=false;app.composer.update(cx,|input,cx|input.set_text("Keep my chat draft",cx));
        app.native_models.insert(app.agent,vec![agent_controls::ModelChoice{id:"model-first".into(),label:"First model".into(),reasoning_efforts:vec![]},agent_controls::ModelChoice{id:"model-target".into(),label:"Target model".into(),reasoning_efforts:vec!["high".into()]}]);
        app.toggle_agent_picker(point(px(20.),px(650.)),window,cx);
        app.agent_picker.as_ref().unwrap().search.update(cx,|input,cx|input.set_text("target",cx));
    });
    draw_frame(&view,cx);assert!(cx.debug_bounds("model-search").is_some());assert!(cx.debug_bounds("model-choice-2").is_none());
    click_selector(cx,"model-choice-1");
    view.update(cx,|app,cx|{assert_eq!(app.model.as_deref(),Some("model-target"));assert_eq!(app.composer.read(cx).text(),"Keep my chat draft");});
}

#[gpui::test]
fn new_chat_welcome_is_centered_in_its_full_content_area(cx:&mut TestAppContext){
    let(view,cx)=fixture(cx);
    view.update(cx,|app,_|{app.active=None;app.loading=false;app.rows.clear();});
    for (width,height) in [(1180.,790.),(1520.,980.)]{
        cx.simulate_resize(size(px(width),px(height)));
        cx.draw(point(px(0.),px(0.)),size(px(width),px(height)),|_,_|view.clone().into_any_element());
        let content=cx.debug_bounds("new-chat-content").expect("New-chat content");
        let greeting=cx.debug_bounds("new-chat-greeting").expect("New-chat welcome");
        assert!(content.size.height>px(300.));
        assert!((content.center().x-greeting.center().x).abs()<px(1.));
        assert!((content.center().y-greeting.center().y).abs()<px(1.),"Welcome must center in the content, not stick to its top: content={content:?}, greeting={greeting:?}");
    }
}

#[gpui::test]
fn first_submission_starts_at_the_latest_line_and_keeps_following(cx:&mut TestAppContext){
    let (view,cx)=fixture(cx);
    cx.run_until_parked();
    view.update_in(cx,|app,window,cx|{
        app.new_conversation(window,cx);
        app.agent=AgentKind::Codex;
        app.agents[0].available=true;
        app.agents[0].executable=Some(PathBuf::from("/usr/bin/true"));
        app.trusted_workspaces.insert((AgentKind::Codex,app.project.clone()));
        app.composer.update(cx,|input,cx|input.set_text(&(String::from("A long first prompt\n")+&"More context for the first turn.\n".repeat(100)+"FINAL PROMPT LINE"),cx));
        app.send(window,cx);
        assert!(app.active.is_some());
        assert_eq!(app.list.logical_scroll_top().item_ix,app.rows.len(),"Submitting a new chat pinned its viewport to the first line");
        assert!(app.following_bottom);
    });
    draw_frame(&view,cx);
    let prompt=cx.debug_bounds("transcript-row-0").expect("Submitted prompt");
    let viewport=cx.update(|_,cx|view.read(cx).list.viewport_bounds());
    assert!(prompt.top()<viewport.top(),"A long new conversation must reveal its tail");
    assert!((prompt.bottom()-viewport.bottom()).abs()<px(2.));
}

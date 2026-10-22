#!/usr/bin/env python3
"""Persistent JSON-lines adapter between the notch UI and Kai's App Server state."""

from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import queue
import sys
import threading
import time
from typing import Any

from kai import AppServer, Kai, image_file, item_lines, list_thread_rows, unique_thread_rows
from kai_activity import SessionActivityMonitor
from kai_dot import DotActivityMonitor
from kai_code import CodeConnection, CodeConnectionError, UnavailableCodex
from kai_voice import VoiceSession


def thread_refresh_interval(app: Kai, active_tasks: list[dict[str, Any]]) -> float:
    if app.projects_loading:
        return 2.0
    return 5.0 if active_tasks or app.active_turn or app.approvals else 30.0


def activity(app: Kai) -> dict[str, str]:
    approval = app._approval_for_selected()
    if approval:
        params = approval.get("params") or {}
        return {"scene": "permission", "text": str(params.get("reason") or params.get("command") or "Approval needed")}
    if app.active_turn:
        for turn in app.turns:
            if turn.get("id") != app.active_turn:
                continue
            for item in reversed(turn.get("items") or []):
                if item.get("status") not in ("inProgress", "in_progress"):
                    continue
                kind = item.get("type")
                if kind == "commandExecution":
                    return {"scene": "terminal", "text": str(item.get("command") or "Running command")}
                if kind == "webSearch":
                    return {"scene": "search", "text": str(item.get("query") or "Searching")}
                if kind == "fileChange":
                    return {"scene": "files", "text": "Editing files"}
                if kind == "imageView":
                    return {"scene": "image", "text": "Viewing image"}
                if kind in {"mcpToolCall", "dynamicToolCall"}:
                    return {"scene": "tool", "text": str(item.get("tool") or "Using tool")}
        return {"scene": "thinking", "text": "Working"}
    status = app.thread.get("status") or {}
    if status.get("type") == "active":
        return {"scene": "thinking", "text": "Working in Codex"}
    if status.get("type") == "systemError":
        return {"scene": "failure", "text": "Codex task error"}
    return {"scene": "off", "text": "Ready"}


def remaining_limit_percent(result: dict[str, Any]) -> int | None:
    buckets = result.get("rateLimitsByLimitId") or {}
    bucket = buckets.get("codex") if isinstance(buckets, dict) else None
    if not isinstance(bucket, dict):
        bucket = result.get("rateLimits")
    if not isinstance(bucket, dict):
        return None
    primary = bucket.get("primary")
    used = primary.get("usedPercent") if isinstance(primary, dict) else None
    if isinstance(used, bool) or not isinstance(used, (int, float)) or not math.isfinite(used):
        return None
    return round(100 - max(0, min(100, used)))


def tool_activity_line(item: dict[str, Any]) -> str | None:
    """A bounded summary of public tool events; never serialize tool payloads."""
    kind = item.get("type")
    labels = {
        "commandExecution": ("Running a command", "Ran a command"),
        "imageView": ("Viewing an image", "Viewed an image"),
        "imageGeneration": ("Generating an image", "Generated an image"),
        "fileChange": ("Editing files", "Edited files"),
        "webSearch": ("Searching the web", "Searched the web"),
    }
    if kind in {"mcpToolCall", "dynamicToolCall"}:
        tool = str(item.get("tool") or "").lower()
        if "view_image" in tool:
            label = labels["imageView"]
        elif "imagegen" in tool or "image_gen" in tool:
            label = labels["imageGeneration"]
        elif "exec_command" in tool or "write_stdin" in tool:
            label = labels["commandExecution"]
        elif "apply_patch" in tool:
            label = labels["fileChange"]
        elif "web" in tool and ("search" in tool or "run" in tool):
            label = labels["webSearch"]
        else:
            name = "/".join(str(value) for value in (item.get("server"), item.get("tool") or "tool") if value).replace("_", " ")[:96]
            label = (f"Using tool: {name}", f"Used tool: {name}")
    else:
        label = labels.get(kind)
    if label is None:
        return None
    status = item.get("status")
    if status in {"inProgress", "in_progress"}:
        return label[0] + "…"
    if status in {"failed", "declined", "cancelled", "canceled"}:
        return label[1] + " · " + status
    if kind == "commandExecution" and item.get("exitCode") not in (None, 0):
        return label[1] + " · failed"
    return label[1]


def activity_icon(item: dict[str, Any]) -> str:
    summary = tool_activity_line(item) or ""
    for words, icon in ((('command',), 'terminal'), (('image',), 'image'),
                        (('files',), 'files'), (('web',), 'search')):
        if any(word in summary.lower() for word in words):
            return icon
    return "tool"


def activity_summary(items: list[dict[str, Any]]) -> str:
    counts: dict[str, int] = {}
    for item in items:
        label = tool_activity_line(item)
        if label is not None:
            counts[label] = counts.get(label, 0) + 1
    phrases = []
    for label, count in list(counts.items())[:4]:
        if count > 1:
            label = label.replace("a command", "commands").replace("an image", "images")
        phrases.append(label if not phrases else label[0].lower() + label[1:])
    if len(counts) > 4:
        phrases.append("other tools")
    return ", ".join(phrases)


def snapshot(app: Kai, include_files: bool = True,
             remaining_percent: int | None = None,
             active_tasks: list[dict[str, Any]] | None = None) -> dict[str, Any]:
    active_tasks = active_tasks or []
    active_ids = {task["id"] for task in active_tasks}
    focus_id = active_tasks[0]["id"] if active_tasks else app.thread_id
    pending_approval = (app._approval_for_thread(focus_id) or app._approval_for_selected()
                        or next(iter(app.approvals), None))
    approval_thread_id = (pending_approval.get("params") or {}).get("threadId") if pending_approval else None
    approval_params = (pending_approval.get("params") or {}) if pending_approval else {}
    approval_activity = {"scene": "permission", "text": str(approval_params.get("reason") or
                         approval_params.get("command") or "Approval needed")}
    # Walk backward only to the requested prompt boundary. Do not format or
    # serialize the rest of a large transcript for the native text view.
    selected = []
    prompts = 0
    has_more = False
    boundary = None
    voice = getattr(app, "voice", None)
    history_items = (reversed(voice.history_items()) if voice and voice.has_voice_history else
                     ((turn, item) for turn in reversed(app.turns) for item in reversed(turn.get("items") or [])))
    for turn, item in history_items:
        if item.get("type") not in {"userMessage", "agentMessage"} and tool_activity_line(item) is None:
            continue
        if item.get("type") == "userMessage":
            prompts += 1
            if prompts > app.history_prompt_limit:
                has_more = True
                if boundary is not None:
                    selected = selected[:boundary]
                break
        if turn.get("status") in {"completed", "interrupted", "failed"} and item.get("status") in {"inProgress", "in_progress"}:
            item = {**item, "status": "completed" if turn["status"] == "completed" else "failed"}
        selected.append(item)
        if prompts == app.history_prompt_limit and item.get("type") == "userMessage":
            boundary = len(selected)
    history = []
    pending_tools = []

    def flush_tools():
        if not pending_tools:
            return
        history.append({
            "id": pending_tools[0].get("id") or "",
            "kind": "activity",
            "activityIcon": activity_icon(pending_tools[0]),
            "speechText": None,
            "lines": [{"text": activity_summary(pending_tools), "style": "activity", "mathEnabled": False}],
        })
        pending_tools.clear()

    for item in reversed(selected):
        if tool_activity_line(item) is not None:
            pending_tools.append(item)
            continue
        if item.get("type") == "agentMessage" and not str(item.get("text") or "").strip():
            continue
        flush_tools()
        lines = item_lines(item)
        if item.get("_voice"):
            from kai import DisplayLine
            lines = [DisplayLine(item.get("text") or "", "voice:voiceprompt" if item.get("type") == "userMessage" else "voice:voicetranscript", math_enabled=False)]
        history.append({
            "id": item.get("id") or "",
            "kind": "user" if item.get("type") == "userMessage" else "assistant",
            "speechText": item.get("text", "") if item.get("type") == "agentMessage" and not item.get("_voice") else None,
            "lines": [{"text": str(line), "style": getattr(line, "style", ""),
                       "mathEnabled": getattr(line, "math_enabled", True)} for line in lines],
        })
    flush_tools()
    selected_task = next((task for task in active_tasks if task["id"] == app.thread_id), None)
    selected_activity = ({"scene": selected_task["scene"], "text": selected_task["text"]}
                         if selected_task else activity(app))
    state = {
        "voice": voice.state() if voice else None,
        "canGoBack": bool(app.navigation_back),
        "canGoForward": bool(app.navigation_forward),
        "models": app.models,
        "selectedModel": app.selected_model,
        "reasoningEffort": app.reasoning_effort,
        "project": str(app.project),
        "projectsLoading": app.projects_loading,
        "projects": [{"path": str(path), "name": path.name or str(path)} for path in app.project_paths],
        "chats": [{"id": row.get("id") or "", "name": row.get("name") or row.get("preview") or "Untitled",
                   "status": "active" if row.get("id") in active_ids else "idle"}
                  for row in app._project_threads()] if app.project != app.standalone_root else [],
        "standaloneChats": [{"id": row.get("id") or "", "name": row.get("name") or row.get("preview") or "Untitled",
                             "status": "active" if row.get("id") in active_ids else "idle"}
                            for row in app.threads if isinstance(row.get("cwd"), str)
                            and app.thread_directory(row) == app.standalone_root],
        "standalone": app.project == app.standalone_root,
        "threadId": app.thread_id,
        "threadName": app.thread.get("name") or "",
        "history": history,
        "historyHasMore": has_more,
        "readOnly": app.read_only,
        "activeTurnId": app.active_turn,
        "approval": pending_approval is not None,
        "approvalThreadId": approval_thread_id,
        "approvalRequestId": str(pending_approval["id"]) if pending_approval and "id" in pending_approval else None,
        "approvalSummary": json.dumps(approval_params,ensure_ascii=False,indent=2) if pending_approval else None,
        "activity": approval_activity if pending_approval else
                    ({"scene": active_tasks[0]["scene"], "text": active_tasks[0]["text"]}
                     if active_tasks else activity(app)),
        "sessionActivity": selected_activity,
        "activeTasks": [{key: task[key] for key in ("id", "name", "scene", "text")}
                        for task in active_tasks],
        "focusThreadId": active_tasks[0]["id"] if active_tasks else None,
        "notice": app.notice,
        "attachments": [str(path) for path in app.attachments],
        "rateLimitRemainingPercent": remaining_percent,
    }
    if include_files:
        state["files"] = [{"path": str(path), "name": path.name, "depth": depth,
                           "directory": is_dir, "collapsed": path in app.file_collapsed}
                          for path, depth, is_dir in app.file_visible_entries]
    return state


def apply(app: Kai, request: dict[str, Any], code: CodeConnection | None = None) -> None:
    action = request.get("action")
    requested_model = None
    uses_default = action in {"new", "new_project"}
    code_send = action == "send" and code is not None and code.selected_id
    if uses_default and not code_send and "defaultModel" in request:
        model_id = request["defaultModel"]
        if not isinstance(model_id, str) or not model_id:
            raise ValueError("Invalid default model")
        requested_model = next((m for m in app.models if m.get("model") == model_id and not m.get("hidden")), None)
        if requested_model is None:
            raise ValueError("Saved default model is unavailable; choose an available model in Kai's menu")
    guarded={"send","interrupt","context","attachments","settings","approve","reject","files","folder","history_more","voice_start"}
    target=request.get("targetThreadId")
    if action in guarded and isinstance(target,str):
        if target.startswith("code:"):
            if code is None or "code:" + (code.selected_id or "") != target:
                raise ValueError("Code session changed; no command was sent")
        elif code is not None and code.selected_id or app.thread_id != target:
            raise ValueError("Session changed; no command was sent")
    voice = getattr(app, "voice", None)
    if action in {"voice_start", "voice_stop", "voice_audio"}:
        if voice is None:
            raise ValueError("Voice is unavailable")
        if action == "voice_start":
            voice.start(request, code)
        elif request.get("sessionId") == voice.session_id:
            if action == "voice_stop": voice.stop()
            else: voice.audio(request)
        return
    if voice and action in {"chat", "project", "new", "new_project", "navigate", "code_connection"}:
        voice.stop()
    if voice and action == "send" and voice.phase in {"starting", "live", "stopping"}:
        text = request.get("text")
        if not isinstance(text, str) or not text.strip():
            raise ValueError("Enter a prompt")
        voice.text(text)
        return
    if code is not None:
        if action == "code_connection":
            code.enabled = request.get("enabled") is True
            if not code.enabled: code.connected = False; code.leave()
            code._next_poll = 0
            app.dirty = True
            return
        if action == "chat" and str(request.get("id", "")).startswith("code:"):
            code.select(request["id"])
            app.context_text = ""
            app.context_images = []
            app.dirty = True
            return
        if action in {"chat", "project", "new", "new_project", "navigate"}:
            code.leave()
        elif code.selected_id:
            if action == "send":
                text = request.get("text")
                if not isinstance(text, str) or not text.strip(): raise ValueError("Prompt is empty")
                if app.attachments or app.context_images: raise ValueError("Send images from Code; this connection supports text prompts")
                if app.context_text: text = "Context captured when dictation began:\n" + app.context_text + "\n\n" + text
                code.mutate("send", text)
                if isinstance(request.get("requestId"),str):code.accepted_request=(code.selected_id,request["requestId"])
                app.context_text = ""
                app.notice = code.notice
            elif action == "interrupt": code.mutate("cancel")
            elif action == "context":
                if request.get("threadId") != "code:" + code.selected_id: raise ValueError("Context belongs to another session")
                if request.get("paths"): raise ValueError("Send images from Code; the current bridge supports text context")
                app.context_text = str(request.get("text", ""))[:12000]
                app.notice = "Context captured"
                code.notice = app.notice
            elif action == "refresh": code.poll(force=True)
            else: raise ValueError("Use this control in Code; no Codex action was sent")
            app.dirty = True
            return
    if action == "settings":
        model = next((m for m in app.models if m["model"] == request.get("model")), None)
        if model is None:
            raise ValueError("Model is not available from Codex")
        effort = request.get("effort", app.reasoning_effort)
        supported = [e["reasoningEffort"] for e in model["supportedReasoningEfforts"]]
        if effort not in supported:
            effort = model["defaultReasoningEffort"]
        app.selected_model = model["model"]
        app.reasoning_effort = effort
        app.dirty = True
    elif action == "context":
        if request.get("threadId") != app.thread_id:
            raise ValueError("Context belongs to a different chat")
        text = str(request.get("text", ""))[:12000]
        paths = [Path(path).resolve(strict=True) for path in request.get("paths", [])]
        if any(not image_file(path) for path in paths):
            raise ValueError("Context must be a supported image")
        app.context_text = text
        app.context_images = paths
        app.notice = "Context captured"
        app.dirty = True
    elif action == "project":
        path = Path(request["path"]).resolve()
        if path not in app.project_paths:
            raise ValueError("Project is not in the Codex task list")
        app.open_project(app.project_paths.index(path))
        app.file_tree_entries.clear()
        app.file_visible_entries.clear()
        app.file_collapsed.clear()
    elif action == "navigate":
        source = app.navigation_back if request.get("direction") == "back" else app.navigation_forward
        destination = app.navigation_forward if request.get("direction") == "back" else app.navigation_back
        if source:
            target = source[-1]
            previous = app.thread_id
            app.select_thread(target)
            source.pop()
            if previous:
                destination.append(previous)
    elif action == "history_more":
        if request.get("threadId") != app.thread_id:
            raise ValueError("History belongs to a different chat")
        app.history_prompt_limit += 2
        app.dirty = True
    elif action == "chat":
        previous = app.thread_id
        app.select_thread(app._find_thread(request["id"]))
        if previous and previous != app.thread_id:
            app.navigation_back.append(previous)
            app.navigation_forward.clear()
    elif action == "new":
        app.new_chat(projectless=True, model_override=requested_model)
    elif action == "new_project":
        if app.project == app.standalone_root:
            raise ValueError("Select a project first")
        app.new_chat(model_override=requested_model)
    elif action == "send":
        text = request.get("text")
        if not isinstance(text, str) or (not text.strip() and not app.attachments):
            raise ValueError("Enter a prompt")
        app.send_prompt(text)
    elif action == "attach":
        app.command("/attach " + request["path"])
    elif action == "attachments":
        paths = request.get("paths") or []
        if not isinstance(paths, list):
            raise ValueError("Invalid attachment list")
        checked = [Path(path).resolve(strict=True) for path in paths]
        if any(not image_file(path) for path in checked):
            raise ValueError("Attachment must be a supported image")
        app.attachments = checked
        app.dirty = True
    elif action in {"approve", "reject"}:
        if request.get("approvalRequestId") is not None:
            pending = app._approval_for_thread(request.get("threadId")) if request.get("threadId") else app._approval_for_selected()
            if not pending or str(pending.get("id")) != request["approvalRequestId"]:
                raise ValueError("Approval changed; review the current request before deciding")
        app.decide(action == "approve", request.get("threadId"))
    elif action == "interrupt":
        app.command("/interrupt")
    elif action == "files":
        app.refresh_file_tree(preserve_collapsed=True)
    elif action == "folder":
        path = Path(request["path"])
        app.menu_selection["files"] = next(
            (i for i, row in enumerate(app.file_visible_entries) if row[0] == path), -1)
        if app.menu_selection["files"] < 0:
            raise ValueError("Folder is not in this project")
        app.toggle_selected_directory()
    elif action == "refresh":
        app.refresh_threads()
        if app.thread_id:
            result = app.rpc.call("thread/read", {"threadId": app.thread_id, "includeTurns": True})
            thread = result.get("thread") or {}
            app.thread.update({k: v for k, v in thread.items() if k != "turns"})
            if app.read_only:
                app.turns = thread.get("turns") or []
    else:
        raise ValueError("Unknown action")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cwd", type=Path, default=Path.cwd())
    parser.add_argument("--codex", default="codex")
    args = parser.parse_args()
    requests: queue.Queue[dict[str, Any]] = queue.Queue()

    def read_requests() -> None:
        for line in sys.stdin:
            try:
                value = json.loads(line)
                if isinstance(value, dict):
                    requests.put(value)
            except json.JSONDecodeError:
                continue
        requests.put({"action": "quit"})

    threading.Thread(target=read_requests, daemon=True).start()
    try:
        rpc = AppServer(args.codex)
    except Exception as error:
        if os.environ.get("KAI_CODE_ENABLED", "1") == "0": raise
        rpc = UnavailableCodex(error)
    try:
        app = Kai(rpc, args.cwd, service_name="kai_notch", initial_thread_pages=1)
        if isinstance(rpc, UnavailableCodex): app.notice = "Codex unavailable; open Code to use its agents"
        try:
            cursor = None
            while True:
                result = rpc.call("model/list", {"cursor": cursor})
                app.models.extend(m for m in result.get("data", []) if not m.get("hidden"))
                cursor = result.get("nextCursor")
                if not cursor:
                    break
            default = next((m for m in app.models if m.get("isDefault")), next(iter(app.models), None))
            if default:
                app.selected_model = default["model"]
                app.reasoning_effort = default["defaultReasoningEffort"]
        except Exception as error:
            app.notice = "Model discovery unavailable: " + str(error)
        parent_pid = os.getppid()
        previous = ""
        previous_files: tuple[tuple[str, bool], ...] | None = None
        completed_request_id: str | None = None
        completed_request_error: str | None = None
        last_refresh = time.monotonic() - (30 if app.projects_loading else 0)
        thread_fetching = False
        last_full_refresh = last_refresh
        thread_results: queue.Queue[list[dict[str, Any]] | None] = queue.Queue()
        last_limit_fetch = 0.0
        limit_fetching = False
        limit_percent: int | None = None
        limit_results: queue.Queue[dict[str, Any] | None] = queue.Queue()
        codex_retry_results = queue.Queue()
        codex_retry_busy = False
        last_codex_retry = time.monotonic()
        def reconnect_codex():
            candidate = None
            try:
                candidate = AppServer(args.codex)
                models = candidate.call("model/list", {"cursor": None}).get("data", [])
                codex_retry_results.put((candidate, models))
            except Exception:
                if candidate is not None: candidate.close()
                codex_retry_results.put(None)
        code = CodeConnection(enabled=os.environ.get("KAI_CODE_ENABLED", "1") != "0")
        def emit_voice(event):
            print(json.dumps(event, separators=(",", ":")), flush=True)
        app.voice = VoiceSession(app, emit_voice)
        app.event_handler = app.voice.event
        monitor = SessionActivityMonitor()
        dot_monitor = DotActivityMonitor()
        monitor_results: queue.Queue[dict[str, Any]] = queue.Queue()
        monitor_busy = False
        last_monitor = 0.0
        active_tasks: list[dict[str, Any]] = []

        def scan_sessions(rows: list[dict[str, Any]], fallback: list[dict[str, Any]]) -> None:
            try:
                monitored=monitor.poll(rows)
                code_changed=code.poll()
                code_tasks=code.tasks()
                owned=code.owned_thread_ids()
                monitored["activeTasks"]=[task for task in monitored["activeTasks"] if task["id"] not in owned] + code_tasks
                monitored["changed"]=monitored["changed"] or code_changed
                monitor_results.put(monitored)
            except Exception:
                monitor_results.put({"activeTasks": fallback, "changed": False})

        def fetch_limits() -> None:
            try:
                limit_results.put(rpc.call("account/rateLimits/read", timeout=10))
            except Exception:
                limit_results.put(None)

        def fetch_threads(full: bool) -> None:
            try:
                rows, _ = list_thread_rows(rpc, max_pages=None if full else 1)
                thread_results.put(rows)
            except Exception:
                thread_results.put(None)

        while True:
            if os.getppid() != parent_pid:
                break
            try:
                request = requests.get(timeout=0.12)
            except queue.Empty:
                request = None
            if request:
                if request.get("action") == "quit":
                    break
                try:
                    apply(app, request, code)
                    if request.get("action") != "voice_audio":
                        completed_request_error = None
                except Exception as exc:
                    app.notice = str(exc)
                    if request.get("action") != "voice_audio":
                        completed_request_error = str(exc)
                    if code.selected_id: code.notice = "Code: " + str(exc)
                if request.get("action") != "voice_audio":
                    completed_request_id = request.get("requestId")
                    app.dirty = True
            if isinstance(rpc, UnavailableCodex) and not codex_retry_busy and time.monotonic() - last_codex_retry > 30:
                last_codex_retry = time.monotonic(); codex_retry_busy = True
                threading.Thread(target=reconnect_codex, daemon=True).start()
            try: reconnected = codex_retry_results.get_nowait()
            except queue.Empty: pass
            else:
                codex_retry_busy = False
                if reconnected:
                    rpc, models = reconnected
                    app.rpc = rpc; app.models = [model for model in models if not model.get("hidden")]
                    default = next((model for model in app.models if model.get("isDefault")), next(iter(app.models), None))
                    if default: app.selected_model = default["model"]
                    app.notice = "Codex reconnected"; app.dirty = True
                    last_refresh = 0
            app.drain()
            app.voice.sync()
            if rpc.proc.poll() is not None:
                raise RuntimeError("Codex App Server disconnected")
            if not thread_fetching and time.monotonic() - last_refresh >= thread_refresh_interval(app, active_tasks):
                thread_fetching = True
                full_refresh = app.projects_loading or time.monotonic() - last_full_refresh >= 300
                threading.Thread(target=fetch_threads, args=(full_refresh,), daemon=True).start()
            try:
                refreshed = thread_results.get_nowait()
            except queue.Empty:
                pass
            else:
                thread_fetching = False
                last_refresh = time.monotonic()
                if refreshed is not None:
                    if full_refresh:
                        app.set_threads(refreshed)
                        app.projects_loading = False
                        app.dirty = True
                        last_full_refresh = time.monotonic()
                    else:
                        # Poll recent metadata quickly; discover/archive older projects
                        # on the slower full sweep instead of paging everything every 5s.
                        app.set_threads(unique_thread_rows(refreshed + app.threads))
                elif app.projects_loading:
                    app.notice = "Could not refresh projects; retrying"
                    app.dirty = True
            if not monitor_busy and time.monotonic() - last_monitor >= 0.75:
                last_monitor = time.monotonic()
                monitor_busy = True
                threading.Thread(target=scan_sessions, args=(list(app.threads), list(active_tasks)), daemon=True).start()
            try:
                monitored = monitor_results.get_nowait()
            except queue.Empty:
                pass
            else:
                monitor_busy = False
                dot_tasks = dot_monitor.poll()["activeTasks"]
                combined = monitored["activeTasks"] + dot_tasks
                combined.sort(key=lambda task: (task.get("eventTime", ""), task.get("sequence", 0)), reverse=True)
                if combined != active_tasks or monitored.get("changed"):
                    active_tasks = combined
                    app.dirty = True
            if app.rate_limits_changed or time.monotonic() - last_limit_fetch > 120:
                app.rate_limits_changed = False
                if not limit_fetching:
                    last_limit_fetch = time.monotonic()
                    limit_fetching = True
                    threading.Thread(target=fetch_limits, daemon=True).start()
            try:
                limit_result = limit_results.get_nowait()
            except queue.Empty:
                pass
            else:
                limit_fetching = False
                updated_percent = remaining_limit_percent(limit_result or {})
                if updated_percent != limit_percent:
                    limit_percent = updated_percent
                    app.dirty = True
            if app.dirty:
                files = tuple((str(row[0]), row[0] in app.file_collapsed)
                              for row in app.file_visible_entries)
                state = snapshot(app, include_files=files != previous_files,
                                 remaining_percent=limit_percent, active_tasks=active_tasks)
                state=code.overlay(state)
                state["completedRequestId"] = completed_request_id
                state["completedRequestError"] = completed_request_error
                previous_files = files
                encoded = json.dumps({"type": "state", **state}, ensure_ascii=False, separators=(",", ":"))
                if encoded != previous:
                    print(encoded, flush=True)
                    previous = encoded
                app.dirty = False
    finally:
        if 'app' in locals() and hasattr(app, "voice"):
            app.voice.close()
        rpc.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

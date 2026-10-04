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

from kai import AppServer, Kai, image_file, item_lines, unique_thread_rows
from kai_activity import SessionActivityMonitor
from kai_dot import DotActivityMonitor


def activity(app: Kai) -> dict[str, str]:
    approval = app._approval_for_selected()
    if approval:
        params = approval.get("params") or {}
        return {"scene": "permission", "text": str(params.get("reason") or params.get("command") or "Approval needed")}
    if app.active_turn:
        for turn in reversed(app.turns):
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
    history = []
    for turn in app.turns:
        for item in turn.get("items") or []:
            if item.get("type") not in {"userMessage", "agentMessage"}:
                continue
            lines = item_lines(item)
            history.append({
                "id": item.get("id") or "",
                "kind": "user" if item.get("type") == "userMessage" else "assistant",
                "speechText": item.get("text", "") if item.get("type") == "agentMessage" else None,
                "lines": [{"text": str(line), "style": getattr(line, "style", ""),
                           "mathEnabled": getattr(line, "math_enabled", True)} for line in lines],
            })
    state = {
        "canGoBack": bool(app.navigation_back),
        "canGoForward": bool(app.navigation_forward),
        "models": app.models,
        "selectedModel": app.selected_model,
        "reasoningEffort": app.reasoning_effort,
        "project": str(app.project),
        "projects": [{"path": str(path), "name": path.name or str(path)} for path in app.project_paths],
        "chats": [{"id": row.get("id") or "", "name": row.get("name") or row.get("preview") or "Untitled",
                   "status": "active" if row.get("id") in active_ids else "idle"}
                  for row in app._project_threads()] if app.project != app.standalone_root else [],
        "standaloneChats": [{"id": row.get("id") or "", "name": row.get("name") or row.get("preview") or "Untitled",
                             "status": "active" if row.get("id") in active_ids else "idle"}
                            for row in app.threads if isinstance(row.get("cwd"), str)
                            and Path(row["cwd"]).resolve() == app.standalone_root],
        "standalone": app.project == app.standalone_root,
        "threadId": app.thread_id,
        "threadName": app.thread.get("name") or "",
        "history": history,
        "readOnly": app.read_only,
        "activeTurnId": app.active_turn,
        "approval": pending_approval is not None,
        "approvalThreadId": approval_thread_id,
        "activity": approval_activity if pending_approval else
                    ({"scene": active_tasks[0]["scene"], "text": active_tasks[0]["text"]}
                     if active_tasks else activity(app)),
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


def apply(app: Kai, request: dict[str, Any]) -> None:
    action = request.get("action")
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
        app.context_text = str(request.get("text", ""))[:12000]
        paths = [Path(path).resolve(strict=True) for path in request.get("paths", [])]
        if any(not image_file(path) for path in paths):
            raise ValueError("Context must be a supported image")
        app.context_images = paths
        app.dirty = True
    elif action == "project":
        app.refresh_threads()
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
    elif action == "chat":
        previous = app.thread_id
        app.select_thread(app._find_thread(request["id"]))
        if previous and previous != app.thread_id:
            app.navigation_back.append(previous)
            app.navigation_forward.clear()
    elif action == "new":
        app.new_chat(projectless=True)
    elif action == "new_project":
        if app.project == app.standalone_root:
            raise ValueError("Select a project first")
        app.new_chat()
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
    elif action == "approve":
        app.decide(True, request.get("threadId"))
    elif action == "reject":
        app.decide(False, request.get("threadId"))
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
    rpc = AppServer(args.codex)
    try:
        app = Kai(rpc, args.cwd, service_name="kai_notch")
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
        last_refresh = time.monotonic()
        thread_fetching = False
        thread_results: queue.Queue[list[dict[str, Any]] | None] = queue.Queue()
        last_limit_fetch = 0.0
        limit_fetching = False
        limit_percent: int | None = None
        limit_results: queue.Queue[dict[str, Any] | None] = queue.Queue()
        monitor = SessionActivityMonitor()
        dot_monitor = DotActivityMonitor()
        monitor_results: queue.Queue[dict[str, Any]] = queue.Queue()
        monitor_busy = False
        last_monitor = 0.0
        active_tasks: list[dict[str, Any]] = []

        def scan_sessions(rows: list[dict[str, Any]], fallback: list[dict[str, Any]]) -> None:
            try:
                monitor_results.put(monitor.poll(rows))
            except Exception:
                monitor_results.put({"activeTasks": fallback, "changed": False})

        def fetch_limits() -> None:
            try:
                limit_results.put(rpc.call("account/rateLimits/read", timeout=10))
            except Exception:
                limit_results.put(None)

        def fetch_threads() -> None:
            try:
                rows: list[dict[str, Any]] = []
                cursor = None
                for _ in range(100):
                    params: dict[str, Any] = {"limit": 100}
                    if cursor:
                        params["cursor"] = cursor
                    result = rpc.call("thread/list", params, timeout=8)
                    rows.extend(result.get("data") or [])
                    cursor = result.get("nextCursor")
                    if not cursor:
                        break
                thread_results.put(unique_thread_rows(rows))
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
                    apply(app, request)
                except Exception as exc:
                    app.notice = str(exc)
                completed_request_id = request.get("requestId")
                app.dirty = True
            app.drain()
            if not thread_fetching and time.monotonic() - last_refresh >= 5:
                thread_fetching = True
                threading.Thread(target=fetch_threads, daemon=True).start()
            try:
                refreshed = thread_results.get_nowait()
            except queue.Empty:
                pass
            else:
                thread_fetching = False
                last_refresh = time.monotonic()
                if refreshed is not None:
                    app.set_threads(refreshed)
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
                if combined != active_tasks:
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
                state["completedRequestId"] = completed_request_id
                previous_files = files
                encoded = json.dumps({"type": "state", **state}, ensure_ascii=False, separators=(",", ":"))
                if encoded != previous:
                    print(encoded, flush=True)
                    previous = encoded
                app.dirty = False
    finally:
        rpc.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

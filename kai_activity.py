"""Read Codex's local session event stream without loading entire conversations."""

from __future__ import annotations

import json
from datetime import datetime
from pathlib import Path
import re
import time
from typing import Any


TOOL_SCENES = {
    "CommandExecution": "terminal", "commandExecution": "terminal",
    "WebSearch": "search", "webSearch": "search",
    "FileChange": "files", "fileChange": "files",
    "ImageView": "image", "imageView": "image",
    "McpToolCall": "tool", "mcpToolCall": "tool",
    "DynamicToolCall": "tool", "dynamicToolCall": "tool",
}

FINISH_EVENTS = {"task_complete", "task_completed", "turn_aborted", "task_aborted", "task_failed"}


def _final_message(payload: dict[str, Any]) -> bool:
    return payload.get("type") == "message" and payload.get("role") == "assistant" and payload.get("phase") == "final_answer"


def _activity_time(record: dict[str, Any], now: float) -> float | None:
    payload = record.get("payload") or {}
    # Opening a chat or changing settings can touch an abandoned rollout.
    # Only execution/message events renew its activity, never file mtime.
    if not isinstance(payload, dict) or not (
        payload.get("type") in {"task_started", "item_started", "item_completed", "custom_tool_call",
                                "function_call", "function_call_output", "custom_tool_call_output",
                                "agent_message", "agent_reasoning", "token_count"} | FINISH_EVENTS
        or record.get("type") == "response_item" and payload.get("role") != "developer"
    ):
        return None
    try:
        stamp = datetime.fromisoformat(record["timestamp"].replace("Z", "+00:00")).timestamp()
        return min(stamp, now)
    except (KeyError, ValueError, TypeError, AttributeError, OverflowError):
        return now


def _label(value: Any, fallback: str) -> str:
    if isinstance(value, list):
        value = value[-1] if value else ""
    return " ".join(str(value or fallback).split())[:140]


def _command_in_call(source: str) -> str | None:
    match = re.search(r"\bcmd\s*:\s*(\"(?:\\.|[^\"\\])*\"|'(?:\\.|[^'\\])*')", source)
    if not match:
        return None
    literal = match.group(1)
    try:
        value = json.loads(literal) if literal.startswith('"') else literal[1:-1].replace("\\'", "'")
    except ValueError:
        return None
    return _label(value, "Running command")


def _event(payload: dict[str, Any], state: dict[str, Any]) -> bool:
    kind = payload.get("type")
    if kind == "task_started":
        state.update(running=True, scene="thinking", text="Working", turn=payload.get("turn_id"),
                     eventTime="", sequence=0)
        return True
    if kind in FINISH_EVENTS or _final_message(payload):
        if not payload.get("turn_id") or not state.get("turn") or payload.get("turn_id") == state.get("turn"):
            state.update(running=False, scene="off", text="Ready", turn=None)
            return True
    if not state.get("running"):
        return False
    if kind == "agent_message" or kind == "message" and payload.get("role") == "assistant":
        content = payload.get("message") or payload.get("text") or "\n".join(
            part.get("text", "") for part in (payload.get("content") or []) if isinstance(part, dict))
        lines = str(content).strip().splitlines()
        if lines:
            state.update(scene="pencil", text=_label(lines[-1], "Writing"))
            return True
    if kind in {"item_started", "item_completed"}:
        item = payload.get("item") or {}
        scene = TOOL_SCENES.get(item.get("type"))
        if scene:
            fallback = {"terminal": "Running command", "search": "Searching", "files": "Editing files",
                        "image": "Viewing image", "tool": "Using tool"}[scene]
            tool_label = (str(item.get("server", "")) + "/" + str(item.get("tool", ""))) if item.get("tool") else None
            state.update(scene=scene, text=_label(item.get("command") or item.get("query") or tool_label, fallback))
            return True
    if kind in {"custom_tool_call", "function_call"}:
        name = payload.get("name") or ""
        arguments = payload.get("input") or payload.get("arguments") or ""
        if not isinstance(arguments, str):
            arguments = ""
        if "tools.apply_patch" in arguments or name == "apply_patch":
            scene, label = "files", "Editing files"
        elif "tools.web__run" in arguments or name in {"web__run", "web.run"}:
            scene, label = "search", "Searching the web"
        elif "tools.view_image" in arguments or "tools.image_gen" in arguments:
            scene, label = "image", "Viewing image"
        elif "tools.exec_command" in arguments or name in {"exec_command", "write_stdin"}:
            scene, label = "terminal", _command_in_call(arguments) or "Running command"
        else:
            integration = re.search(r"mcp__[A-Za-z0-9_]+", arguments)
            scene, label = "tool", _label(integration.group(0) if integration else name, "Using tool")
        state.update(scene=scene, text=label)
        return True
    return False


class SessionActivityMonitor:
    def __init__(self, stale_after: float = 600, clock=time.time) -> None:
        self.offsets: dict[str, int] = {}
        self.paths: dict[str, str] = {}
        self.identities: dict[str, tuple[int, int]] = {}
        self.states: dict[str, dict[str, Any]] = {}
        self.sequence = 0
        self.stale_after = stale_after
        self.clock = clock
        self.active_ids: set[str] = set()

    def _initial_lines(self, path: Path, size: int) -> list[bytes]:
        # Walk backward to the actual lifecycle event, even when a single turn
        # is very large. Keep only that turn's lines, never the whole history.
        reversed_lines: list[bytes] = []
        partial = b""
        position = size
        with path.open("rb") as stream:
            while position:
                step = min(position, 65536)
                position -= step
                stream.seek(position)
                pieces = (stream.read(step) + partial).split(b"\n")
                partial = pieces[0]
                for line in reversed(pieces[1:]):
                    if not line:
                        continue
                    reversed_lines.append(line)
                    if not any(marker in line for marker in (b"task_started", b"task_complete", b"turn_aborted", b"task_aborted", b"task_failed", b"final_answer")):
                        continue
                    try:
                        record = json.loads(line)
                    except (ValueError, UnicodeDecodeError):
                        continue
                    payload = record.get("payload") or {}
                    if isinstance(payload, dict) and (payload.get("type") in FINISH_EVENTS | {"task_started"} or _final_message(payload)):
                        return list(reversed(reversed_lines))
        if partial:
            reversed_lines.append(partial)
        return list(reversed(reversed_lines))

    def poll(self, rows: list[dict[str, Any]]) -> dict[str, Any]:
        now = self.clock()
        live_ids = {row.get("id") for row in rows if (row.get("status") or {}).get("type") == "active"}
        listed = {row.get("id") for row in rows}
        # A recent-page refresh can omit a running chat. Keep tailing its known
        # file so its completion is consumed even before it reappears in a list.
        rows = rows + [{"id": key, "path": self.paths[key], "name": state.get("name")}
                       for key, state in self.states.items()
                       if key not in listed and state.get("running") and key in self.paths]
        present: set[str] = set()
        changed = False
        finished = []
        for row in rows:
            thread_id, raw_path = row.get("id"), row.get("path")
            if not isinstance(thread_id, str) or not isinstance(raw_path, str):
                continue
            path = Path(raw_path)
            try:
                stat = path.stat()
                size = stat.st_size
            except OSError:
                continue
            present.add(thread_id)
            if thread_id in self.states:
                self.states[thread_id]["name"] = row.get("name") or row.get("preview") or "Chat"
            identity = (stat.st_dev, stat.st_ino)
            previous = self.offsets.get(thread_id) if self.paths.get(thread_id) == raw_path and self.identities.get(thread_id) == identity else None
            if previous == size:
                continue
            if previous is None or previous > size:
                try:
                    # A writer may be between writes of a JSON line. Do not
                    # consume its partial tail or the completion event is lost.
                    with path.open("rb") as stream:
                        stream.seek(max(0, size - 65536))
                        tail = stream.read()
                    boundary = tail.rfind(b"\n")
                    size = max(0, size - len(tail) + boundary + 1) if boundary >= 0 else 0
                    lines = self._initial_lines(path, size)
                except OSError:
                    continue
                state: dict[str, Any] = {"running": False, "scene": "off", "text": "Ready", "turn": None, "sequence": 0, "eventTime": "",
                                         "name": row.get("name") or row.get("preview") or "Chat"}
            else:
                try:
                    with path.open("rb") as stream:
                        stream.seek(previous)
                        data = stream.read(size - previous)
                        complete = data.rfind(b"\n") + 1
                        lines = data[:complete].splitlines()
                        size = previous + complete
                except OSError:
                    continue
                state = self.states.get(thread_id, {"running": False, "scene": "off", "text": "Ready", "turn": None, "sequence": 0, "eventTime": "",
                                                    "name": row.get("name") or row.get("preview") or "Chat"}).copy()
            was_running = self.states.get(thread_id, {}).get("running", False)
            previous_turn = self.states.get(thread_id, {}).get("turn")
            for line in lines:
                try:
                    record = json.loads(line)
                except (ValueError, UnicodeDecodeError):
                    continue
                payload = record.get("payload") or {}
                event_changed = isinstance(payload, dict) and _event(payload, state)
                if was_running and event_changed and not state.get("running"):
                    finished.append({"id":thread_id,"runId":previous_turn})
                    was_running = False
                stamp = _activity_time(record, now)
                if stamp is not None and (payload.get("type") not in FINISH_EVENTS or event_changed):
                    state["lastActivity"] = stamp
                if event_changed:
                    if payload.get("type") in {"item_started", "custom_tool_call", "function_call"}:
                        self.sequence += 1
                        state["sequence"] = self.sequence
                        state["eventTime"] = record.get("timestamp") or ""
            if state != self.states.get(thread_id):
                changed = True
            self.states[thread_id] = state
            self.offsets[thread_id] = size
            self.paths[thread_id] = raw_path
            self.identities[thread_id] = identity
        for thread_id in set(self.states) - present:
            if self.states[thread_id].get("running"):
                continue
            self.states.pop(thread_id, None)
            self.offsets.pop(thread_id, None)
            self.paths.pop(thread_id, None)
            self.identities.pop(thread_id, None)
            changed = True
        active = [{"id": thread_id, "name": state.get("name") or "Chat",
                   "scene": state["scene"], "text": state["text"],
                   "runId":state.get("turn"),
                   "sequence": state["sequence"], "eventTime": state["eventTime"]}
                  for thread_id, state in self.states.items() if state["running"] and
                  (thread_id in live_ids or now - state.get("lastActivity", now) < self.stale_after)]
        active_ids = {task["id"] for task in active}
        changed = changed or active_ids != self.active_ids
        self.active_ids = active_ids
        active.sort(key=lambda task: (task["eventTime"], task["sequence"]), reverse=True)
        return {"activeTasks": active, "focusThreadId": active[0]["id"] if active else None,
                "finishedTasks":finished,"changed": changed}

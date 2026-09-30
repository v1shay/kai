"""Read Codex's local session event stream without loading entire conversations."""

from __future__ import annotations

import json
from pathlib import Path
import re
from typing import Any


TOOL_SCENES = {
    "CommandExecution": "terminal", "commandExecution": "terminal",
    "WebSearch": "search", "webSearch": "search",
    "FileChange": "files", "fileChange": "files",
    "ImageView": "image", "imageView": "image",
    "McpToolCall": "tool", "mcpToolCall": "tool",
    "DynamicToolCall": "tool", "dynamicToolCall": "tool",
}


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
    if kind in {"task_complete", "turn_aborted", "task_failed"}:
        if not state.get("turn") or payload.get("turn_id") == state.get("turn"):
            state.update(running=False, scene="off", text="Ready", turn=None)
            return True
    if not state.get("running"):
        return False
    if kind in {"item_started", "item_completed"}:
        item = payload.get("item") or {}
        scene = TOOL_SCENES.get(item.get("type"))
        if scene:
            fallback = {"terminal": "Running command", "search": "Searching", "files": "Editing files",
                        "image": "Viewing image", "tool": "Using tool"}[scene]
            state.update(scene=scene, text=_label(item.get("command") or item.get("query")
                                                   or item.get("tool"), fallback))
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
            scene, label = "tool", _label(name, "Using tool")
        state.update(scene=scene, text=label)
        return True
    return False


class SessionActivityMonitor:
    def __init__(self) -> None:
        self.offsets: dict[str, int] = {}
        self.paths: dict[str, str] = {}
        self.states: dict[str, dict[str, Any]] = {}
        self.sequence = 0

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
                    if not any(marker in line for marker in (b"task_started", b"task_complete", b"turn_aborted", b"task_failed")):
                        continue
                    try:
                        record = json.loads(line)
                    except (ValueError, UnicodeDecodeError):
                        continue
                    if (record.get("payload") or {}).get("type") in {"task_started", "task_complete", "turn_aborted", "task_failed"}:
                        return list(reversed(reversed_lines))
        if partial:
            reversed_lines.append(partial)
        return list(reversed(reversed_lines))

    def poll(self, rows: list[dict[str, Any]]) -> dict[str, Any]:
        present: set[str] = set()
        changed = False
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
            previous = self.offsets.get(thread_id) if self.paths.get(thread_id) == raw_path else None
            if previous == size:
                continue
            if previous is None or previous > size:
                try:
                    lines = self._initial_lines(path, size)
                except OSError:
                    continue
                state: dict[str, Any] = {"running": False, "scene": "off", "text": "Ready", "turn": None, "sequence": 0, "eventTime": "",
                                         "name": row.get("name") or row.get("preview") or "Chat"}
            else:
                try:
                    with path.open("rb") as stream:
                        stream.seek(previous)
                        lines = stream.read(size - previous).splitlines()
                except OSError:
                    continue
                state = self.states.get(thread_id, {"running": False, "scene": "off", "text": "Ready", "turn": None, "sequence": 0, "eventTime": "",
                                                    "name": row.get("name") or row.get("preview") or "Chat"}).copy()
            for line in lines:
                try:
                    record = json.loads(line)
                except (ValueError, UnicodeDecodeError):
                    continue
                payload = record.get("payload") or {}
                if isinstance(payload, dict) and _event(payload, state):
                    if payload.get("type") in {"item_started", "custom_tool_call", "function_call"}:
                        self.sequence += 1
                        state["sequence"] = self.sequence
                        state["eventTime"] = record.get("timestamp") or ""
            if state != self.states.get(thread_id):
                changed = True
            self.states[thread_id] = state
            self.offsets[thread_id] = size
            self.paths[thread_id] = raw_path
        for thread_id in set(self.states) - present:
            if self.states[thread_id].get("running"):
                continue
            self.states.pop(thread_id, None)
            self.offsets.pop(thread_id, None)
            self.paths.pop(thread_id, None)
            changed = True
        active = [{"id": thread_id, "name": state.get("name") or "Chat",
                   "scene": state["scene"], "text": state["text"],
                   "sequence": state["sequence"], "eventTime": state["eventTime"]}
                  for thread_id, state in self.states.items() if state["running"]]
        active.sort(key=lambda task: (task["eventTime"], task["sequence"]), reverse=True)
        return {"activeTasks": active, "focusThreadId": active[0]["id"] if active else None,
                "changed": changed}

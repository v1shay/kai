"""Normalized activity events sent to Kai by an OpenAI dot through MCP."""

from __future__ import annotations

from datetime import datetime, timezone
import json
import os
from pathlib import Path
import threading
import time
from typing import Any


SCENES = {
    "thinking", "search", "read", "terminal", "files", "image", "document",
    "code", "git", "permission", "waiting", "tool", "agents", "plan",
    "review", "compact", "warning", "question",
}
FINAL_SCENES = {"success", "failure", "declined"}


def default_event_path() -> Path:
    configured = os.environ.get("KAI_DOT_EVENTS")
    if configured:
        return Path(configured).expanduser()
    return Path.home() / "Library/Application Support/Kai/dot-events.jsonl"


class DotEventStore:
    """Append-only store shared by the MCP endpoint and the running Kai app."""

    def __init__(self, path: Path | None = None) -> None:
        self.path = path or default_event_path()
        self._lock = threading.Lock()
        self._sequence = 0

    def append(self, session_id: str, title: str, scene: str, text: str) -> dict[str, Any]:
        if scene not in SCENES | FINAL_SCENES | {"off"}:
            raise ValueError(f"Unsupported Kai scene: {scene}")
        session_id = " ".join(session_id.split())[:160]
        if not session_id:
            raise ValueError("session_id is required")
        with self._lock:
            self.path.parent.mkdir(parents=True, exist_ok=True)
            self._sequence += 1
            event = {
                "version": 1,
                "sequence": self._sequence,
                "timestamp": datetime.now(timezone.utc).isoformat(),
                "source": "dot",
                "sessionId": session_id,
                "title": " ".join((title or "Dot").split())[:120],
                "scene": scene,
                "text": " ".join((text or scene.title()).split())[:180],
            }
            with self.path.open("a", encoding="utf-8") as stream:
                stream.write(json.dumps(event, ensure_ascii=False, separators=(",", ":")) + "\n")
            return event


class DotActivityMonitor:
    """Tail Dot events without retaining prompts, transcripts, or tool arguments."""

    def __init__(self, path: Path | None = None, final_hold: float = 1.4) -> None:
        self.path = path or default_event_path()
        self.final_hold = final_hold
        self.offset = 0
        self.identity: tuple[int, int] | None = None
        self.sessions: dict[str, dict[str, Any]] = {}
        self.sequence = 0

    def _consume(self, event: dict[str, Any], now: float) -> None:
        if event.get("source") != "dot" or event.get("version") != 1:
            return
        session_id, scene = event.get("sessionId"), event.get("scene")
        if not isinstance(session_id, str) or scene not in SCENES | FINAL_SCENES | {"off"}:
            return
        if scene == "off":
            self.sessions.pop(session_id, None)
            return
        self.sequence += 1
        self.sessions[session_id] = {
            "id": "dot:" + session_id,
            "name": str(event.get("title") or "Dot")[:120],
            "scene": scene,
            "text": str(event.get("text") or scene.title())[:180],
            "sequence": self.sequence,
            "eventTime": str(event.get("timestamp") or ""),
            "expires": now + self.final_hold if scene in FINAL_SCENES else None,
        }

    def poll(self) -> dict[str, Any]:
        now = time.monotonic()
        try:
            stat = self.path.stat()
            identity = (stat.st_dev, stat.st_ino)
            if identity != self.identity or stat.st_size < self.offset:
                self.identity, self.offset = identity, 0
            if stat.st_size > self.offset:
                with self.path.open("rb") as stream:
                    stream.seek(self.offset)
                    data = stream.read(stat.st_size - self.offset)
                complete = data.rfind(b"\n") + 1
                if complete == 0:
                    data = b""
                else:
                    data = data[:complete]
                    self.offset += complete
                for line in data.splitlines():
                    try:
                        event = json.loads(line)
                    except (ValueError, UnicodeDecodeError):
                        continue
                    if isinstance(event, dict):
                        self._consume(event, now)
        except OSError:
            pass
        for session_id, state in list(self.sessions.items()):
            if state["expires"] is not None and now >= state["expires"]:
                self.sessions.pop(session_id, None)
        active = sorted(self.sessions.values(), key=lambda row: row["sequence"], reverse=True)
        return {"activeTasks": active, "focusThreadId": active[0]["id"] if active else None}

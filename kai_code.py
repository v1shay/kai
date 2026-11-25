"""Capability-gated client for Code's local agent owner. Never launches a CLI.

Every mutation goes to the advertised Code session and epoch exactly once.
An ambiguous acknowledgement is surfaced, never retried or rerouted to Codex.
"""
from __future__ import annotations
import copy
import json
import os
from pathlib import Path
import socket
import stat
import threading
import time
import uuid
from typing import Any
import queue
import re
from types import SimpleNamespace

PREFIX = "code:"
MAX_RESPONSE = 2 * 1024 * 1024
SCENES = (('fail', 'failure'), ('error', 'failure'), ('stop', 'declined'),
          ('question', 'question'), ('permission', 'permission'), ('approval', 'permission'), ('command', 'terminal'),
          ('file', 'files'), ('search', 'search'), ('image', 'image'), ('tool', 'tool'))

_ORBIT = re.compile(r'<orbit_interaction version="1">(.*?)</orbit_interaction>', re.S)


def normalize_live_output(text: str) -> tuple[list[dict[str, Any]], str]:
    """Decode Code's private presentation envelopes; never display/read JSON.

    Fenced examples remain literal code. Unknown or incomplete envelopes are
    withheld rather than being narrated as metadata.
    """
    from kai import markdown_lines
    output = []
    spoken = []
    run = []
    fence = None

    def plain(source):
        if source.strip():
            output.extend({"text": str(line), "style": getattr(line, "style", "text"),
                           "mathEnabled": getattr(line, "math_enabled", True)} for line in markdown_lines(source))
        spoken.append(source)

    def flush():
        source = "".join(run)
        run.clear()
        # The bounded tail can start inside a serialized, oversized card.
        close = source.find("</orbit_interaction>")
        opening = source.find('<orbit_interaction version="1">')
        if close >= 0 and (opening < 0 or close < opening):
            source = source[close + len("</orbit_interaction>"):]
        cursor = 0
        for match in _ORBIT.finditer(source):
            plain(source[cursor:match.start()])
            try:
                value = json.loads(match.group(1))
                card = value["card"] if value.get("orbit_interaction") == 1 else {}
                if not isinstance(card, dict): raise ValueError()
                kind = card.get("kind")
                body = card.get("text", "") if kind == "speaker" else card.get("prompt", "") if kind == "question" else card.get("title", "")
                if not isinstance(body, str): raise ValueError()
                if kind == "speaker":
                    if card.get("speaker") not in {"user", "assistant"}: raise ValueError()
                    user = card.get("speaker") == "user"
                    output.append({"text":body,"style":"voice:voiceprompt" if user else "voice:voicetranscript","mathEnabled":False})
                    if not user: spoken.append(body)
                elif kind == "question":
                    output.append({"text":body,"style":"heading","mathEnabled":False})
                    spoken.append(body)
                elif kind in {"status", "delegation"}:
                    details=card.get("details", "")
                    shown=details if kind == "status" and isinstance(details,str) and details else body
                    output.append({"text":shown,"style":"voice:voicestatus" if kind == "status" else "activity","mathEnabled":False})
                elif kind == "answer":
                    answer=card.get("answer", "")
                    if isinstance(answer,str):output.append({"text":answer,"style":"voice:voicecomplete","mathEnabled":False})
                else: raise ValueError()
            except (ValueError, TypeError, KeyError, AttributeError):
                output.append({"text":"Interaction available in Code","style":"activity","mathEnabled":False})
            cursor = match.end()
        tail = source[cursor:]
        partial = tail.find("<orbit_interaction")
        plain(tail[:partial] if partial >= 0 else tail)

    for line in text.splitlines(keepends=True):
        marker = re.match(r"^ {0,3}(`{3,}|~{3,})", line)
        if fence:
            if marker and marker.group(1).startswith(fence) and not line[marker.end():].strip(): fence=None
            else: output.append({"text":line.rstrip("\n"),"style":"code","mathEnabled":False})
        elif marker:
            flush();fence=marker.group(1)
        else: run.append(line)
    flush()
    return output, "".join(spoken)


class CodeConnectionError(RuntimeError):
    pass

class CodeConnection:
    def __init__(self, path: Path | None = None, enabled: bool = True) -> None:
        self.path = path or Path(os.environ.get("KAI_CODE_SOCKET", str(Path.home() / "Library/Application Support/Orbit/kai/control.sock")))
        self.enabled = enabled
        self.connected = False
        restored=os.environ.get("KAI_CODE_SELECTED", "")
        self.selected_id: str | None = restored[len(PREFIX):] if restored.startswith(PREFIX) else None
        self.epoch: str | None = None
        self.state: dict[str, Any] = {"sessions": [], "history": []}
        self.notice = ""
        self.accepted_request:tuple[str,str] | None = None
        self.error = ""
        self._next_poll = 0.0
        self._backoff = 1.0
        self._lock = threading.RLock()

    def _request(self, method: str, params: dict[str, Any] | None = None) -> dict[str, Any]:
        if not self.enabled:
            raise CodeConnectionError("Code connection is disabled")
        try:
            metadata = self.path.lstat()
            parent = self.path.parent.lstat()
            if (not stat.S_ISSOCK(metadata.st_mode) or metadata.st_uid != os.getuid()
                or metadata.st_mode & 0o077 or not stat.S_ISDIR(parent.st_mode)
                or parent.st_uid != os.getuid() or parent.st_mode & 0o077):
                raise CodeConnectionError("Code socket permissions are unsafe")
            identifier = str(uuid.uuid4())
            body = {"protocol": 1, "id": identifier, "method": method, "params": params or {}}
            if method != "snapshot":
                if not self.connected or not self.epoch:
                    raise CodeConnectionError("Code disconnected; no command was sent")
                body["epoch"] = self.epoch
            encoded = json.dumps(body, ensure_ascii=False).encode() + b"\n"
            if len(encoded) > 65536:
                raise CodeConnectionError("Prompt is too large for the Code connection")
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.settimeout(1.2)
                stream.connect(str(self.path))
                stream.sendall(encoded)
                data = bytearray()
                while b"\n" not in data:
                    chunk = stream.recv(min(65536, MAX_RESPONSE + 1 - len(data)))
                    if not chunk:
                        raise CodeConnectionError("Code disconnected before acknowledging the request; no automatic retry")
                    data.extend(chunk)
                    if len(data) > MAX_RESPONSE:
                        raise CodeConnectionError("Code returned an oversized response")
            reply = json.loads(data.split(b"\n", 1)[0])
            if not isinstance(reply, dict) or reply.get("protocol") != 1 or reply.get("id") != identifier or not isinstance(reply.get("epoch"), str):
                raise CodeConnectionError("Code returned an incompatible response")
            if not reply.get("ok"):
                raise CodeConnectionError(str(reply.get("error") or "Code rejected the request"))
            if method != "snapshot" and reply["epoch"] != self.epoch:
                raise CodeConnectionError("Code restarted during the command; no automatic retry")
            result = reply.get("result")
            if not isinstance(result, dict):
                raise CodeConnectionError("Code returned invalid state")
            if method == "snapshot":
                self.epoch = reply["epoch"]
            return result
        except (OSError, ValueError, TypeError) as error:
            raise CodeConnectionError(f"Code connection unavailable: {error}") from error

    @staticmethod
    def _validated(value: dict[str, Any]) -> dict[str, Any]:
        sessions = value.get("sessions")
        history = value.get("history")
        if not isinstance(sessions, list) or len(sessions) > 50 or not isinstance(history, list) or len(history) > 100:
            raise CodeConnectionError("Code returned invalid session data")
        seen = set()
        for row in sessions:
            if (not isinstance(row, dict) or not isinstance(row.get("id"), str) or not row["id"] or row["id"] in seen
                or any(not isinstance(row.get(key), str) for key in ("title", "agent", "project", "status"))
                or not isinstance(row.get("running"), bool) or not isinstance(row.get("capabilities"), dict)
                or row.get("running") and not isinstance(row.get("runId"), str)
                or row.get("externalId") is not None and not isinstance(row.get("externalId"), str)):
                raise CodeConnectionError("Code returned an invalid session")
            seen.add(row["id"])
        for block in history:
            if (not isinstance(block, dict) or not isinstance(block.get("id"), str)
                or block.get("kind") not in {"assistant", "user", "activity"} or not isinstance(block.get("lines"), list)
                or block.get("speechText") is not None and not isinstance(block.get("speechText"), str)
                or any(not isinstance(line, dict) or not isinstance(line.get("text"), str) or not isinstance(line.get("style"), str) for line in block["lines"])):
                raise CodeConnectionError("Code returned an invalid transcript")
        if value.get("selectedId") is not None and value["selectedId"] not in seen:
            raise CodeConnectionError("Code selected an unknown session")
        return value

    def poll(self, force: bool = False) -> bool:
        with self._lock:
            if not self.enabled or not force and time.monotonic() < self._next_poll:
                return False
            prior = (self.connected, self.state, self.epoch)
            try:
                state = self._validated(self._request("snapshot"))
                # Follow Code's native selection only while viewing a Code chat.
                if self.selected_id and state.get("selectedId"):
                    self.selected_id = state["selectedId"]
                self.state = state
                if self.notice == "Turn started": self.notice = ""
                self.connected = True
                self.error = ""
                self._backoff = 1.0
                self._next_poll = time.monotonic() + 0.5
            except CodeConnectionError as error:
                self.error = str(error)
                self.connected = False
                self._next_poll = time.monotonic() + self._backoff
                self._backoff = min(15.0, self._backoff * 2)
            return prior != (self.connected, self.state, self.epoch)

    def session(self) -> dict[str, Any] | None:
        return next((row for row in self.state["sessions"] if row["id"] == self.selected_id), None)

    def select(self, identifier: str) -> None:
        with self._lock:
            if not identifier.startswith(PREFIX):
                raise CodeConnectionError("Invalid Code session ID")
            self.poll(force=True)
            native_id = identifier[len(PREFIX):]
            row = next((row for row in self.state["sessions"] if row["id"] == native_id), None)
            if not self.connected or not row or row["capabilities"].get("open") is not True:
                raise CodeConnectionError("Code chat is unavailable; open Code and try again")
            self._request("open", {"sessionId": native_id})
            self.selected_id = native_id
            self.notice = "Loading Code chat…"
            self._next_poll = 0

    def mutate(self, method: str, text: str = "") -> None:
        with self._lock:
            row = self.session()
            if not self.connected or not row or row["capabilities"].get(method) is not True:
                raise CodeConnectionError("Code cannot perform this action right now; its current draft, run, or loading state is preserved")
            params = {"sessionId": row["id"]}
            if method == "send":
                params["text"] = text
                params["sendToken"] = row.get("sendToken")
            elif method == "cancel":
                params["runId"] = row.get("runId")
            result = self._request(method, params)
            if result.get("accepted") is not True: raise CodeConnectionError("Code did not accept the command")
            self.notice = ("Turn started" if result.get("started") else "Prompt staged in Code — " + str(result.get("notice") or "finish its confirmation there")) if method == "send" else "Stopping Code agent"
            self._next_poll = 0

    def owned_thread_ids(self) -> set[str]:
        """Code's live snapshot overrides rollout guesses, including idle chats."""
        with self._lock:
            return {row["externalId"] for row in self.state["sessions"] if row.get("externalId")} if self.connected and self.enabled else set()

    def tasks(self, include_disconnected: bool = False) -> list[dict[str, Any]]:
        with self._lock:
            if not self.enabled or not self.connected and not include_disconnected:
                return []
            return [{"id": PREFIX + row["id"], "name": row["title"], "scene": self.scene(row),
                     "text": f"{row['agent']} · {row['status']}", "externalId": row.get("externalId"),"runId":row.get("runId"),
                     "sequence": index, "eventTime": ""} for index, row in enumerate(self.state["sessions"]) if row["running"]]

    @staticmethod
    def scene(row: dict[str, Any]) -> str:
        if not row["running"]:
            return "off"
        return next((scene for word, scene in SCENES if word in row["status"].lower()), "thinking")

    def finished_tasks(self, previous: list[dict[str, Any]]) -> list[dict[str, Any]]:
        with self._lock:
            if not self.connected or not self.enabled:
                return []
            idle={PREFIX+row["id"] for row in self.state["sessions"] if not row["running"]}
            return [{"id":task["id"],"runId":task.get("runId")} for task in previous if task["id"] in idle]

    def overlay(self, state: dict[str, Any]) -> dict[str, Any]:
        with self._lock:
            state["codeConnection"] = {"connected": self.connected, "enabled": self.enabled,"error":self.error}
            rows = self.state["sessions"] if self.connected else []
            # Keep Codex navigation and projects intact; Code gets a separate
            # labelled section, never a fake project folder or Codex thread.
            state["codeChats"] = [{"id": PREFIX + row["id"], "name": f"{row['agent']} · {row['title']}", "status": "active" if row["running"] else "idle"} for row in rows]
            state["codeAcceptedRequestId"]=self.accepted_request[1] if self.accepted_request and self.accepted_request[0] == self.selected_id else None
            if not self.selected_id:
                return state
            row = self.session()
            if not row:
                state.update(threadId=PREFIX+self.selected_id, threadName="Code chat unavailable",history=[],readOnly=True,activeTurnId=None,approval=False,sessionActivity={"scene":"off","text":""},notice="Code chat was removed or is unavailable")
                return state
            matching = self.state.get("selectedId") == self.selected_id
            running = self.connected and row["running"]
            message = self.notice if self.notice and self.notice != "Loading Code chat…" else ("Code: " + str(self.state["notice"]) if self.state.get("notice") else "Code · " + row["agent"])
            if not self.connected:
                message = "Code disconnected — reconnecting. Commands are disabled."
            elif not matching or self.state.get("loading"):
                message = "Loading Code chat…"
            selected_activity={"scene":self.scene(row) if self.connected else "off","text":row["status"] if self.connected else ""}
            global_activity=selected_activity if running else state.get("activity", selected_activity)
            history=copy.deepcopy(self.state["history"]) if matching and not self.state.get("loading") else []
            for block in history:
                if block["id"].startswith("live-"):
                    raw="\n".join(line["text"] for line in block["lines"])
                    block["lines"],block["speechText"]=normalize_live_output(raw)
            state.update(sessionActivity=selected_activity,codeSessionActivity=selected_activity,project=row["project"],standalone=True,chats=[],threadId=PREFIX+row["id"],threadName=row["title"],history=history,historyHasMore=False,
                         readOnly=not self.connected or row["capabilities"].get("send") is not True,
                         activeTurnId=row.get("runId") if running else None,approval=False,approvalThreadId=None,
                         models=[],selectedModel=row["agent"],reasoningEffort="",canGoBack=False,canGoForward=False,
                         files=[],attachments=[],notice=message,focusThreadId=PREFIX+row["id"] if running else state.get("focusThreadId"),activity=global_activity)
            return state

    def leave(self) -> None:
        with self._lock:
            self.selected_id = None
            self.accepted_request = None
            self.notice = ""


class UnavailableCodex:
    """Keep Code's independent connection alive while Codex is unavailable."""
    def __init__(self, error: Exception) -> None:
        self.events = queue.Queue()
        self.proc = SimpleNamespace(poll=lambda: None)
        self.error = str(error)
    def call(self, method, params=None, timeout=25):
        if method == "thread/list": return {"data": [], "nextCursor": None}
        if method == "model/list": return {"data": [], "nextCursor": None}
        raise CodeConnectionError("Codex is unavailable; Code sessions remain independent")
    def close(self): pass

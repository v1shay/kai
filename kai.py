#!/usr/bin/env python3
"""Kai: a small terminal client for local Codex tasks."""

from __future__ import annotations

import argparse
import collections
import curses
import json
import locale
import os
from pathlib import Path
import queue
import re
import shlex
import shutil
import subprocess
import sys
import textwrap
import threading
import time
from typing import Any
from urllib.parse import unquote, urlparse


class DisplayLine(str):
    """Rendered text plus an optional presentation hint for the curses view."""

    def __new__(cls, value: str, style: str = "", math_enabled: bool | None = None) -> DisplayLine:
        line = super().__new__(cls, value)
        line.style = style
        line.math_enabled = getattr(value, "math_enabled", True) if math_enabled is None else math_enabled
        return line


class RPCError(RuntimeError):
    pass


def resolve_codex_binary(binary: str = "codex", candidates: list[str] | None = None) -> str:
    """Avoid broken PATH shims when a working local Codex executable exists."""
    if binary != "codex":
        return binary
    if candidates is None:
        home = Path.home()
        releases = sorted((home / ".codex/packages/app-server-daemon/releases").glob("*/bin/codex"), reverse=True)
        candidates = [
            os.environ.get("KAI_CODEX_BINARY", ""),
            shutil.which("codex") or "",
            "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
            str(home / "Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex"),
            str(home / ".codex/plugins/.plugin-appserver/codex-cli/bin/codex"),
            *(str(path) for path in releases),
        ]
    for candidate in dict.fromkeys(candidates):
        if not candidate or not Path(candidate).is_file():
            continue
        try:
            result = subprocess.run([candidate, "--version"], capture_output=True,
                                    text=True, timeout=3, check=False)
        except (OSError, subprocess.TimeoutExpired):
            continue
        if result.returncode == 0 and result.stdout.strip().startswith("codex-cli "):
            return candidate
    raise RuntimeError("No working Codex CLI found. Check the Codex installation.")


class AppServer:
    """One stdio JSONL connection. The reader never writes to the terminal."""

    def __init__(self, binary: str = "codex") -> None:
        self.proc = subprocess.Popen(
            [resolve_codex_binary(binary), "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True, bufsize=1,
        )
        self.events: queue.Queue[dict[str, Any]] = queue.Queue()
        self.pending: dict[int, tuple[threading.Event, dict[str, Any]]] = {}
        self.lock = threading.Lock()
        self.write_lock = threading.Lock()
        self.next_id = 1
        self.errors: collections.deque[str] = collections.deque(maxlen=12)
        threading.Thread(target=self._read, daemon=True).start()
        threading.Thread(target=self._stderr, daemon=True).start()
        self.call("initialize", {"clientInfo": {"name": "kai", "title": "Kai TUI", "version": "0.1.0"}})
        self.notify("initialized", {})

    def _send(self, message: dict[str, Any]) -> None:
        line = json.dumps(message, separators=(",", ":")) + "\n"
        with self.write_lock:
            if self.proc.poll() is not None:
                raise RPCError("Codex App Server stopped: " + (self.errors[-1] if self.errors else "unknown error"))
            assert self.proc.stdin is not None
            self.proc.stdin.write(line)
            self.proc.stdin.flush()

    def call(self, method: str, params: dict[str, Any] | None = None, timeout: float = 25) -> dict[str, Any]:
        done = threading.Event()
        box: dict[str, Any] = {}
        with self.lock:
            request_id = self.next_id
            self.next_id += 1
            self.pending[request_id] = (done, box)
        try:
            self._send({"id": request_id, "method": method, "params": params or {}})
            if not done.wait(timeout):
                raise RPCError(f"{method} timed out")
            if "error" in box:
                error = box["error"]
                raise RPCError(f"{method}: {error.get('message', error)}")
            return box.get("result") or {}
        finally:
            with self.lock:
                self.pending.pop(request_id, None)

    def notify(self, method: str, params: dict[str, Any] | None = None) -> None:
        self._send({"method": method, "params": params or {}})

    def respond(self, request_id: int | str, result: dict[str, Any]) -> None:
        self._send({"id": request_id, "result": result})

    def _read(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if "id" in message and "method" not in message:
                with self.lock:
                    entry = self.pending.get(message["id"])
                if entry:
                    done, box = entry
                    box.update(message)
                    done.set()
            else:
                self.events.put(message)
        self.events.put({"method": "kai/disconnected", "params": {}})
        with self.lock:
            for done, box in self.pending.values():
                box["error"] = {"message": self.errors[-1] if self.errors else "App Server disconnected"}
                done.set()

    def _stderr(self) -> None:
        assert self.proc.stderr is not None
        for line in self.proc.stderr:
            self.errors.append(line.strip())

    def close(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                self.proc.kill()


def display_text(value: Any, limit: int | None = 2500) -> str:
    if isinstance(value, str):
        result = value
    elif value is None:
        result = ""
    else:
        result = json.dumps(value, ensure_ascii=False, indent=2)
    return result if limit is None or len(result) <= limit else result[:limit] + "\n... [truncated]"


def _markdown_inline(text: str) -> str:
    """Remove inline Markdown notation without changing its human-readable content."""
    if text.strip() in {r"\[", r"\]", r"\(", r"\)"}:
        return text
    contains_inline_code = bool(re.search(r"(`+).*?\1", text))
    # Keep TeX delimiters and commands intact for the native app's optional
    # KaTeX renderer. Ordinary Markdown stripping must not eat \(, \[, or _.
    math_chunks: list[str] = []
    def save_math(match: re.Match[str]) -> str:
        math_chunks.append(match.group(0))
        return f"\ue000{len(math_chunks) - 1}\ue001"
    text = re.sub(r"\$\$.+?\$\$|\\\((?:\\.|[^\\])*?\\\)|\\\[(?:\\.|[^\\])*?\\\]", save_math, text)
    text = re.sub(r"!\[([^]]*)\]\([^)]*\)", lambda match: "[image: " + (match.group(1) or "image") + "]", text)
    text = re.sub(r"\[([^]]+)\]\(([^)]+)\)",
                  lambda match: match.group(1) if match.group(1) == match.group(2)
                  else f"{match.group(1)} ({match.group(2)})", text)
    text = re.sub(r"<((?:https?://|mailto:)[^>]+)>", r"\1", text)
    text = re.sub(r"(`+)(.*?)\1", r"\2", text)
    text = re.sub(r"(\*\*|__)(.+?)\1", r"\2", text)
    text = re.sub(r"~~(.+?)~~", r"\1", text)
    text = re.sub(r"(?<!\w)([*_])([^\n]+?)\1(?!\w)", r"\2", text)
    text = re.sub(r"\\([\\`*{}\[\]()#+.!_>~-])", r"\1", text)
    text = re.sub(r"\ue000(\d+)\ue001", lambda match: math_chunks[int(match.group(1))], text)
    return DisplayLine(text, math_enabled=not contains_inline_code)


def _table_cells(line: str) -> list[str]:
    """Split a Markdown table row while keeping escaped pipes in cells."""
    cells = re.split(r"(?<!\\)\|", line.strip())
    if cells and not cells[0].strip():
        cells.pop(0)
    if cells and not cells[-1].strip():
        cells.pop()
    return [_markdown_inline(cell.strip().replace(r"\|", "|")) for cell in cells]


def markdown_lines(text: str) -> list[DisplayLine]:
    """Render common Markdown as clean terminal lines, leaving the source untouched."""
    rendered: list[DisplayLine] = []
    in_code = False
    math_closer: str | None = None
    fence = re.compile(r"^\s*(`{3,}|~{3,})")
    table_rule = re.compile(r"^\s*\|?\s*:?-{3,}:?\s*(?:\|\s*:?-{3,}:?\s*)+\|?\s*$")
    source_lines = text.splitlines()
    skipped: set[int] = set()
    for index, raw in enumerate(source_lines):
        if index in skipped:
            continue
        if math_closer is not None:
            rendered.append(DisplayLine(raw))
            if raw.strip() == math_closer:
                math_closer = None
            continue
        if fence.match(raw):
            in_code = not in_code
            continue
        if in_code:
            rendered.append(DisplayLine(raw, "code", math_enabled=False))
            continue
        if raw.strip() in {"$$", r"\["}:
            math_closer = "$$" if raw.strip() == "$$" else r"\]"
            rendered.append(DisplayLine(raw))
            continue

        if (index + 1 < len(source_lines) and "|" in raw
                and table_rule.match(source_lines[index + 1])):
            headers = _table_cells(raw)
            columns = _table_cells(source_lines[index + 1])
            if len(headers) == len(columns) and len(headers) >= 2:
                rendered.append(DisplayLine("  ·  ".join(headers), "heading", math_enabled="`" not in raw))
                skipped.add(index + 1)
                row = index + 2
                while row < len(source_lines) and "|" in source_lines[row]:
                    cells = _table_cells(source_lines[row])
                    if len(cells) != len(headers):
                        break
                    rendered.append(DisplayLine(cells[0], "table", math_enabled="`" not in source_lines[row]))
                    rendered.extend(DisplayLine(f"{headers[column]}: {cells[column]}", "table", math_enabled="`" not in source_lines[row])
                                    for column in range(1, len(headers)))
                    rendered.append(DisplayLine(""))
                    skipped.add(row)
                    row += 1
                continue

        heading = re.match(r"^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$", raw)
        if heading:
            rendered.append(DisplayLine(_markdown_inline(heading.group(1)), "heading"))
            continue
        if re.match(r"^\s{0,3}((\*\s*){3,}|(-\s*){3,}|(_\s*){3,})$", raw):
            rendered.append(DisplayLine("─" * 24, "rule"))
            continue
        if table_rule.match(raw):
            continue

        quote = re.match(r"^(\s*)>\s?(.*)$", raw)
        if quote:
            rendered.append(DisplayLine(quote.group(1) + "│ " + _markdown_inline(quote.group(2)), "quote", math_enabled="`" not in raw))
            continue
        task = re.match(r"^(\s*)[-+*]\s+\[([ xX])\]\s+(.*)$", raw)
        if task:
            mark = "☑" if task.group(2).lower() == "x" else "☐"
            rendered.append(DisplayLine(f"{task.group(1)}{mark} {_markdown_inline(task.group(3))}", "list", math_enabled="`" not in raw))
            continue
        bullet = re.match(r"^(\s*)[-+*]\s+(.*)$", raw)
        if bullet:
            rendered.append(DisplayLine(bullet.group(1) + "• " + _markdown_inline(bullet.group(2)), "list", math_enabled="`" not in raw))
            continue
        if raw.strip().startswith("|") and raw.strip().endswith("|"):
            cells = [_markdown_inline(cell.strip()) for cell in raw.strip().strip("|").split("|")]
            rendered.append(DisplayLine("  │  ".join(cells), "table", math_enabled="`" not in raw))
            continue
        rendered.append(DisplayLine(_markdown_inline(raw)))
    return rendered or [DisplayLine("")]


def wrap_display_lines(lines: list[str], width: int) -> list[DisplayLine]:
    """Turn arbitrary multiline text into physical, styled terminal rows."""
    wrapped: list[DisplayLine] = []
    ansi_escape = re.compile(r"\x1b(?:\[[0-?]*[ -/]*[@-~]|\][^\x07]*(?:\x07|\x1b\\))")
    for line in lines:
        style = getattr(line, "style", "")
        for raw in re.split(r"\r\n|\r|\n", str(line)):
            physical = ansi_escape.sub("", raw).expandtabs(4)
            physical = "".join(char if ord(char) >= 32 and ord(char) != 127 else "�"
                               for char in physical)
            parts = textwrap.wrap(physical, width=max(1, width), expand_tabs=False,
                                  replace_whitespace=False, drop_whitespace=False,
                                  break_long_words=True, break_on_hyphens=False) or [""]
            wrapped.extend(DisplayLine(part, style) for part in parts)
    return wrapped


def voice_transcript_lines(text: str) -> list[DisplayLine] | None:
    """Present a complete realtime voice handoff without changing its stored text."""
    handoff = re.fullmatch(
        r"\s*(?:<realtime_delegation>\s*)?"
        r"<source>transcript_tail_flush</source>\s*"
        r"<input>.*?</input>\s*"
        r"<transcript_delta>(.*?)</transcript_delta>\s*"
        r"</realtime_delegation>\s*",
        text, re.DOTALL,
    )
    if handoff is None:
        return None

    turns: list[tuple[str, list[str]]] = []
    for line in handoff.group(1).strip().splitlines():
        speaker = re.match(r"^(user|assistant):[ \t]*(.*)$", line)
        if speaker:
            turns.append((speaker.group(1), [speaker.group(2)]))
        elif turns:
            turns[-1][1].append(line)
        elif line.strip():
            return None
    if not turns:
        return None

    rendered = [DisplayLine("Voice transcript", "heading"), DisplayLine("")]
    for speaker, content in turns:
        spoken = "\n".join(content).strip()
        if not spoken:
            continue
        if speaker == "user":
            rendered.append(DisplayLine("You (voice)", "prompt"))
            rendered.extend(DisplayLine(line, "prompt") for line in markdown_lines(spoken))
        else:
            rendered.append(DisplayLine("ChatGPT (voice)", "heading"))
            rendered.extend(markdown_lines(spoken))
        rendered.append(DisplayLine(""))
    return rendered if len(rendered) > 2 else None


def prompt_lines(item: dict[str, Any]) -> list[DisplayLine]:
    """Render a user prompt as Markdown inside the prompt-band presentation."""
    text = user_message_text(item)
    voice_lines = voice_transcript_lines(text)
    if voice_lines is not None:
        return voice_lines
    return [DisplayLine(str(line), "prompt") for line in markdown_lines(text)]


def item_lines(item: dict[str, Any], mode: str = "history") -> list[str]:
    kind = item.get("type", "unknown")
    if mode == "history":
        if kind == "userMessage":
            return [*prompt_lines(item), ""]
        if kind == "agentMessage":
            return [*markdown_lines(display_text(item.get("text", ""), None)), ""]
        return []
    if mode == "terminal" and kind == "commandExecution":
        command = display_text(item.get("command", ""), 1000)
        status = item.get("status", "")
        code = item.get("exitCode")
        output = display_text(item.get("aggregatedOutput", ""), None)
        return [f"$ {command}  [{status}{'' if code is None else ', exit ' + str(code)}]",
                *(DisplayLine(line, "code") for line in output.splitlines()), ""]
    if mode == "activity" and kind not in {"userMessage", "agentMessage"}:
        label = kind
        if kind == "mcpToolCall":
            label += " / " + str(item.get("server", "")) + "/" + str(item.get("tool", ""))
        elif kind == "commandExecution":
            label += " / " + display_text(item.get("command", ""), 140)
        elif kind == "webSearch":
            label += " / " + str(item.get("query", ""))
        details = next((item[key] for key in ("result", "error", "changes", "summary",
                                              "text", "aggregatedOutput", "content", "plan")
                        if item.get(key) not in (None, "", [], {})), "")
        return [f"{label}  [{item.get('status', 'done')}]", display_text(details, None), ""]
    return []


def user_message_text(item: dict[str, Any]) -> str:
    parts = item.get("content") or []
    text = "\n".join(part.get("text") or "" for part in parts if part.get("type") == "text")
    rendered: list[str] = []
    image_number = 0
    for part in parts:
        if part.get("type") == "text":
            rendered.append(part.get("text") or "")
        elif part.get("type") in {"image", "localImage"}:
            image_number += 1
            label = f"[Image {image_number}]"
            if label not in text:
                # Keep a stable placeholder today. A future rich chatbox can replace this
                # line with the image carried by the structured part without changing data.
                rendered.append(label)
    return "\n".join(rendered)


def merge_turns(older: list[dict[str, Any]], newer: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Keep read history if a resume response contains only some turns."""
    merged = list(older)
    positions = {turn.get("id"): i for i, turn in enumerate(merged) if turn.get("id")}
    for turn in newer:
        turn_id = turn.get("id")
        if turn_id in positions:
            previous = merged[positions[turn_id]]
            items = list(previous.get("items") or [])
            item_positions = {item.get("id"): i for i, item in enumerate(items) if item.get("id")}
            for item in turn.get("items") or []:
                item_id = item.get("id")
                if item_id in item_positions:
                    items[item_positions[item_id]] = item
                else:
                    items.append(item)
            merged[positions[turn_id]] = {**previous, **turn, "items": items}
        else:
            merged.append(turn)
    return merged


def all_items(turns: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [item for turn in turns for item in (turn.get("items") or [])]


def unique_thread_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Choose the freshest list record for each task ID."""
    def rank(row: dict[str, Any]) -> tuple[float, int]:
        updated = row.get("updatedAt")
        updated_at = float(updated) if isinstance(updated, (int, float)) else -1.0
        try:
            size = Path(row["path"]).stat().st_size if row.get("path") else -1
        except (OSError, TypeError, ValueError):
            size = -1
        return updated_at, size

    chosen: dict[str, tuple[tuple[float, int], dict[str, Any]]] = {}
    for row in rows:
        thread_id = row.get("id")
        if not isinstance(thread_id, str) or not thread_id:
            continue
        row_rank = rank(row)
        if thread_id not in chosen or row_rank > chosen[thread_id][0]:
            chosen[thread_id] = row_rank, row
    return [row for _, row in sorted(chosen.values(), key=lambda entry: entry[0], reverse=True)]


def git(cwd: Path, *args: str, timeout: int = 12) -> tuple[int, str]:
    try:
        result = subprocess.run(["git", *args], cwd=cwd, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True, timeout=timeout)
        return result.returncode, result.stdout if result.returncode == 0 else result.stderr.strip()
    except (OSError, subprocess.TimeoutExpired) as exc:
        return 1, str(exc)


def image_file(path: Path) -> bool:
    try:
        with path.open("rb") as stream:
            head = stream.read(16)
    except OSError:
        return False
    return (head.startswith(b"\x89PNG\r\n\x1a\n") or head.startswith(b"\xff\xd8\xff")
            or head[:6] in {b"GIF87a", b"GIF89a"} or
            (head.startswith(b"RIFF") and head[8:12] == b"WEBP") or
            head.startswith((b"II*\x00", b"MM\x00*")))


def _dropped_image_path(token: str, cwd: Path) -> Path | None:
    """Resolve one terminal-pasted path without accepting arbitrary non-image files."""
    if token.startswith("file://"):
        parsed = urlparse(token)
        if parsed.netloc not in {"", "localhost"}:
            return None
        token = unquote(parsed.path)
    path = Path(token).expanduser()
    if not path.is_absolute():
        path = cwd / path
    try:
        path = path.resolve(strict=True)
    except OSError:
        return None
    return path if path.is_file() and image_file(path) else None


def consume_dropped_images(value: str, cwd: Path) -> tuple[str, list[Path]]:
    """Replace an image-path suffix pasted by a terminal with compact prompt labels."""
    starts = [0] + [index + 1 for index, char in enumerate(value) if char.isspace()]
    for start in starts:
        suffix = value[start:]
        # Warp/Finder may paste a single absolute path with literal spaces and no shell
        # quoting. Resolve the whole suffix first, before asking shlex to split it.
        direct = _dropped_image_path(suffix.strip(), cwd)
        if direct is not None:
            return value[:start].rstrip(), [direct]
        try:
            tokens = shlex.split(suffix)
        except ValueError:
            continue
        if not tokens:
            continue
        paths = [_dropped_image_path(token, cwd) for token in tokens]
        if any(path is None for path in paths):
            continue
        prefix = value[:start].rstrip()
        return prefix, [path for path in paths if path is not None]
    return value, []


class Kai:
    COMMANDS = ("projects", "chats", "chat", "new", "history", "prompts", "activity", "terminal",
                "files", "file", "diff", "git", "attach", "approve", "reject",
                "interrupt", "status", "help", "quit")

    def __init__(self, rpc: AppServer, cwd: Path, service_name: str = "kai_tui",
                 standalone_root: Path | None = None) -> None:
        self.rpc = rpc
        self.service_name = service_name
        self.cwd = cwd.resolve()
        self.standalone_root = (standalone_root or Path.home() / "Library" / "Application Support" / "Kai" / "Chats").resolve()
        self.project = self.cwd
        self.threads: list[dict[str, Any]] = []
        self.project_paths: list[Path] = []
        self.thread_id: str | None = None
        self.thread: dict[str, Any] = {}
        self.turns: list[dict[str, Any]] = []
        self.active_turn: str | None = None
        self.read_only = False
        self.last_poll = 0.0
        self.approvals: collections.deque[dict[str, Any]] = collections.deque()
        self.attachments: list[Path] = []
        self.view = "projects"
        self.notice = "Select a project"
        self.scroll = 0
        self.menu_selection = {"projects": 0, "chats": 0, "files": 0}
        self.menu_offset = {"projects": 0, "chats": 0, "files": 0}
        self.file_tree_root: Path | None = None
        self.file_tree_entries: list[tuple[Path, int, bool]] = []
        self.file_visible_entries: list[tuple[Path, int, bool]] = []
        self.file_collapsed: set[Path] = set()
        self.file_origin = "history"
        self.body: list[str] = []
        self.dirty = True
        self.rate_limits_changed = False
        self.running = True
        self.refresh_threads()
        self.menu_selection["projects"] = self.project_paths.index(self.cwd)

    def refresh_threads(self) -> None:
        rows: list[dict[str, Any]] = []
        cursor = None
        for _ in range(100):
            params: dict[str, Any] = {"limit": 100}
            if cursor:
                params["cursor"] = cursor
            result = self.rpc.call("thread/list", params)
            rows.extend(result.get("data") or [])
            cursor = result.get("nextCursor")
            if not cursor:
                break
        self.set_threads(unique_thread_rows(rows))

    def set_threads(self, rows: list[dict[str, Any]]) -> None:
        previous = [(row.get("id"), row.get("name"), row.get("cwd"), row.get("path"))
                    for row in self.threads]
        self.threads = rows
        paths = {self.cwd}
        if self.project != self.standalone_root:
            paths.add(self.project)
        for row in self.threads:
            raw = row.get("cwd")
            if isinstance(raw, str) and raw.startswith("/"):
                path = Path(raw).resolve()
                if path != self.standalone_root:
                    paths.add(path)
        self.project_paths = sorted(paths, key=lambda p: str(p).lower())
        self.menu_selection["projects"] = min(self.menu_selection["projects"], len(self.project_paths) - 1)
        if previous != [(row.get("id"), row.get("name"), row.get("cwd"), row.get("path"))
                        for row in rows]:
            self.dirty = True

    def open_project(self, index: int) -> None:
        if not 0 <= index < len(self.project_paths):
            raise ValueError("Project not found")
        self.leave_chat()
        self.project = self.project_paths[index]
        self.file_tree_root = None
        self.menu_selection["projects"] = index
        self.menu_selection["chats"] = 0
        self.menu_offset["chats"] = 0
        self.view = "chats"
        self.notice = "Select a chat or choose New chat"
        self.dirty = True

    def open_chat(self, index: int) -> None:
        rows = self._project_threads()
        if not 0 <= index < len(rows):
            raise ValueError("Chat not found")
        self.menu_selection["chats"] = index
        self.select_thread(rows[index]["id"])

    def leave_chat(self) -> None:
        if self.thread_id and not self.read_only:
            try:
                self.rpc.call("thread/unsubscribe", {"threadId": self.thread_id}, timeout=5)
            except RPCError:
                pass
        self.thread_id = None
        self.thread = {}
        self.turns = []
        self.active_turn = None
        self.read_only = False
        self.attachments.clear()

    def back(self) -> None:
        if self.view == "projects":
            return
        if self.view == "chats":
            self.view = "projects"
            self.notice = "Select a project"
        elif self.view == "file" and self.file_origin == "files":
            self.view = "files"
        elif self.view in {"files", "file"}:
            self.view = "history" if self.thread_id else "chats"
        elif self.thread_id:
            self.leave_chat()
            self.view = "chats"
            self.notice = "Select a chat or choose New chat"
        else:
            self.view = "projects"
            self.notice = "Select a project"
        self.scroll = 0
        self.dirty = True

    def open_selected(self) -> None:
        if self.view == "projects":
            self.open_project(self.menu_selection["projects"])
        elif self.view == "chats":
            self.open_chat(self.menu_selection["chats"])
        elif self.view == "files":
            entries = self.file_visible_entries
            index = self.menu_selection["files"]
            if 0 <= index < len(entries):
                path, _, is_dir = entries[index]
                if is_dir:
                    self.toggle_selected_directory()
                else:
                    self.file_path = self._resolve_file(str(path))
                    self.file_origin = "files"
                    self.view = "file"
                    self.scroll = 0
                    self.dirty = True

    def refresh_file_tree(self, preserve_collapsed: bool = False) -> None:
        """Scan the selected project once; navigation uses the cached rows."""
        entries: list[tuple[Path, int, bool]] = []
        skip = {".git", "node_modules", ".build", "__pycache__", ".venv"}
        stack: list[tuple[Path, int, bool]] = [(self.project, -1, True)]
        while stack:
            path, depth, is_dir = stack.pop()
            if depth >= 0:
                entries.append((path, depth, is_dir))
            if not is_dir:
                continue
            try:
                with os.scandir(path) as directory:
                    children = [(Path(entry.path), entry.is_dir(follow_symlinks=False))
                                for entry in directory if entry.name not in skip]
            except OSError:
                continue
            children.sort(key=lambda child: (not child[1], child[0].name.casefold()))
            stack.extend((child_path, depth + 1, child_is_dir)
                         for child_path, child_is_dir in reversed(children))
        collapsed = self.file_collapsed.intersection(path for path, _, is_dir in entries if is_dir) if preserve_collapsed and self.file_tree_root == self.project else set()
        self.file_tree_root = self.project
        self.file_tree_entries = entries
        self.file_collapsed = collapsed
        self._rebuild_visible_files()
        self.menu_selection["files"] = 0
        self.menu_offset["files"] = 0
        self.dirty = True

    def toggle_selected_directory(self, collapse: bool | None = None) -> None:
        index = self.menu_selection["files"]
        if not 0 <= index < len(self.file_visible_entries):
            return
        path, _, is_dir = self.file_visible_entries[index]
        if not is_dir:
            return
        should_collapse = path not in self.file_collapsed if collapse is None else collapse
        if should_collapse:
            self.file_collapsed.add(path)
        else:
            self.file_collapsed.discard(path)
        self._rebuild_visible_files()
        self.menu_selection["files"] = next(
            (i for i, entry in enumerate(self.file_visible_entries) if entry[0] == path), 0)
        self.dirty = True

    def _rebuild_visible_files(self) -> None:
        visible: list[tuple[Path, int, bool]] = []
        hidden_depth: int | None = None
        for entry in self.file_tree_entries:
            entry_path, depth, entry_is_dir = entry
            if hidden_depth is not None:
                if depth > hidden_depth:
                    continue
                hidden_depth = None
            visible.append(entry)
            if entry_is_dir and entry_path in self.file_collapsed:
                hidden_depth = depth
        self.file_visible_entries = visible

    def select_thread(self, thread_id: str) -> None:
        old = self.thread_id
        result = self.rpc.call("thread/read", {"threadId": thread_id, "includeTurns": True})
        self.thread = result.get("thread") or {}
        self.turns = self.thread.get("turns") or []
        self.thread_id = thread_id
        if old and old != thread_id:
            try:
                self.rpc.call("thread/unsubscribe", {"threadId": old}, timeout=5)
            except RPCError:
                pass
        try:
            resumed = self.rpc.call("thread/resume", {"threadId": thread_id})
            self.read_only = False
        except RPCError as exc:
            resumed = {}
            self.read_only = True
            self.notice = "Read-only: " + str(exc).split(": ", 1)[-1]
        live_thread = resumed.get("thread") or {}
        if live_thread.get("turns"):
            self.turns = merge_turns(self.turns, live_thread["turns"])
        self.thread.update({k: v for k, v in live_thread.items() if k != "turns"})
        self.active_turn = next((t.get("id") for t in reversed(self.turns)
                                 if t.get("status") == "inProgress"), None)
        raw_cwd = self.thread.get("cwd")
        if isinstance(raw_cwd, str) and raw_cwd.startswith("/"):
            self.project = Path(raw_cwd).resolve()
        self.view = "history"
        self.scroll = 0
        if not self.read_only:
            self.notice = "Selected " + thread_id[:12]
        self.dirty = True

    def _find_thread(self, token: str) -> str:
        matches = [x["id"] for x in self.threads if x.get("id") == token or x.get("id", "").startswith(token)]
        if token.isdigit():
            visible = self._project_threads()
            index = int(token) - 1
            if 0 <= index < len(visible):
                return visible[index]["id"]
        if len(matches) == 1:
            return matches[0]
        if len(matches) > 1:
            raise ValueError("Ambiguous task ID; use more characters")
        raise ValueError("Task not found; run /chats")

    def _project_threads(self) -> list[dict[str, Any]]:
        return [x for x in self.threads if isinstance(x.get("cwd"), str)
                and Path(x["cwd"]).resolve() == self.project]

    def send_prompt(self, prompt: str) -> None:
        if not self.thread_id:
            self.new_chat()
        assert self.thread_id
        if self.read_only:
            resumed = self.rpc.call("thread/resume", {"threadId": self.thread_id})
            self.thread.update({k: v for k, v in (resumed.get("thread") or {}).items() if k != "turns"})
            self.read_only = False
        parts: list[dict[str, str]] = [{"type": "text", "text": prompt}]
        # Keep the exact path in a structured localImage part. A future non-curses UI can
        # render this part inline without changing prompt text or migrating stored chats.
        parts.extend({"type": "localImage", "path": str(path)} for path in self.attachments)
        if self.active_turn:
            if self.attachments:
                self.notice = "Finish or interrupt this turn before sending images"
                return
            result = self.rpc.call("turn/steer", {"threadId": self.thread_id,
                                                   "expectedTurnId": self.active_turn,
                                                   "input": parts})
            self.notice = "Steered active turn " + str(result.get("turnId", ""))[:12]
        else:
            result = self.rpc.call("turn/start", {"threadId": self.thread_id, "input": parts})
            turn = result.get("turn") or {}
            self.active_turn = turn.get("id")
            if turn and all(x.get("id") != turn.get("id") for x in self.turns):
                self.turns.append(turn)
            self.notice = "Turn started"
        self.attachments.clear()
        self.view = "history"
        self.scroll = 0
        self.dirty = True

    def new_chat(self, projectless: bool = False) -> None:
        directory = self.standalone_root if projectless else self.project
        if projectless:
            directory.mkdir(parents=True, exist_ok=True)
        result = self.rpc.call("thread/start", {"cwd": str(directory), "serviceName": self.service_name})
        thread = result.get("thread") or {}
        thread_id = thread.get("id")
        if not thread_id:
            raise RPCError("thread/start returned no task ID")
        self.project = directory
        self.file_tree_root = None
        self.file_tree_entries.clear()
        self.file_visible_entries.clear()
        self.file_collapsed.clear()
        self.refresh_threads()
        if not any(row.get("id") == thread_id for row in self.threads):
            self.threads.insert(0, thread)
        self.thread_id = thread_id
        self.thread = thread
        self.turns = []
        self.active_turn = None
        self.read_only = False
        self.view = "history"
        self.notice = "New task " + thread_id[:12]
        self.dirty = True

    def _approval_for_thread(self, thread_id: str | None) -> dict[str, Any] | None:
        if not thread_id:
            return None
        for approval in self.approvals:
            if approval.get("params", {}).get("threadId") == thread_id:
                return approval
        return None

    def _approval_for_selected(self) -> dict[str, Any] | None:
        for approval in self.approvals:
            if approval.get("params", {}).get("threadId") == self.thread_id:
                return approval
        return None

    def decide(self, accept: bool, thread_id: str | None = None) -> None:
        approval = self._approval_for_thread(thread_id) if thread_id else self._approval_for_selected()
        if not approval and not thread_id:
            approval = next(iter(self.approvals), None)
        if not approval:
            self.notice = "No pending approval for this task"
            return
        method = approval.get("method")
        params = approval.get("params") or {}
        if method in {"item/commandExecution/requestApproval", "item/fileChange/requestApproval"}:
            answer = {"decision": "accept" if accept else "decline"}
        elif method == "item/permissions/requestApproval":
            answer = {"permissions": params.get("permissions", {}) if accept else {}}
        elif method in {"execCommandApproval", "applyPatchApproval"}:
            answer = {"decision": "approved" if accept else {"denied": {"rejection": "Rejected in Kai"}}}
        elif method == "mcpServer/elicitation/request" and not accept:
            answer = {"action": "decline", "content": None}
        else:
            self.notice = "This prompt needs the desktop app: " + str(method)
            return
        self.rpc.respond(approval["id"], answer)
        self.approvals.remove(approval)
        self.notice = "Approved" if accept else "Rejected"
        self.dirty = True

    def command(self, line: str) -> None:
        if line.startswith("//"):
            if not self.thread_id:
                raise ValueError("Open a chat first")
            self.send_prompt(line[1:])
            return
        if not line.startswith("/"):
            if not self.thread_id:
                raise ValueError("Open a chat first")
            self.send_prompt(line)
            return
        try:
            tokens = shlex.split(line[1:])
        except ValueError as exc:
            raise ValueError(str(exc)) from exc
        if not tokens:
            return
        cmd, *args = tokens
        if cmd not in self.COMMANDS:
            raise ValueError("Unknown command. Use /help")
        argument = " ".join(args)
        if cmd == "quit":
            self.running = False
        elif cmd == "help":
            self.view = "help"
        elif cmd == "projects":
            self.refresh_threads()
            if argument:
                if argument.isdigit() and 1 <= int(argument) <= len(self.project_paths):
                    index = int(argument) - 1
                else:
                    path = Path(argument).expanduser().resolve()
                    if not path.is_dir():
                        raise ValueError("Project folder does not exist")
                    if path not in self.project_paths:
                        self.project_paths.append(path)
                    index = self.project_paths.index(path)
                self.open_project(index)
            else:
                self.view = "projects"
        elif cmd == "chats":
            self.refresh_threads()
            self.view = "chats"
        elif cmd == "chat":
            if not argument:
                raise ValueError("Use /chat <id or list number>")
            self.select_thread(self._find_thread(argument))
        elif cmd == "new":
            self.new_chat()
        elif cmd == "files":
            self.refresh_file_tree()
            self.view = "files"
        elif cmd in {"history", "prompts", "activity", "terminal", "diff", "git", "status"}:
            self.view = cmd
        elif cmd == "file":
            if not argument:
                raise ValueError("Use /file <path>")
            self.file_path = self._resolve_file(argument)
            self.file_origin = "files" if self.view == "files" else "history"
            self.view = "file"
        elif cmd == "attach":
            if not argument:
                raise ValueError("Use /attach <image path>")
            path = self._resolve_file(argument)
            if not image_file(path):
                raise ValueError("Image must be PNG, JPEG, GIF, WebP or TIFF")
            self.attachments.append(path)
            self.notice = "Attached " + path.name + "; send a prompt"
        elif cmd == "approve":
            self.decide(True)
        elif cmd == "reject":
            self.decide(False)
        elif cmd == "interrupt":
            if not self.thread_id or not self.active_turn:
                self.notice = "No active turn"
            else:
                self.rpc.call("turn/interrupt", {"threadId": self.thread_id, "turnId": self.active_turn})
                self.notice = "Interrupt requested"
        self.scroll = 0
        self.dirty = True

    def _resolve_file(self, raw: str) -> Path:
        path = Path(raw).expanduser()
        if not path.is_absolute():
            path = self.project / path
        path = path.resolve(strict=True)
        if not path.is_file():
            raise ValueError("Not a file")
        return path

    def drain(self) -> None:
        changed = False
        pending_text: dict[tuple[str, str], tuple[dict[str, Any], list[str]]] = {}

        def flush_text(item_id: str | None = None) -> None:
            for identity in list(pending_text):
                if item_id is not None and identity[0] != item_id:
                    continue
                item, chunks = pending_text.pop(identity)
                previous = item.get(identity[1])
                item[identity[1]] = (previous if isinstance(previous, str) else "") + "".join(chunks)

        if self.read_only and self.thread_id and time.monotonic() - self.last_poll >= 2:
            self.last_poll = time.monotonic()
            try:
                result = self.rpc.call("thread/read", {"threadId": self.thread_id, "includeTurns": True}, timeout=8)
                thread = result.get("thread") or {}
                self.thread.update({k: v for k, v in thread.items() if k != "turns"})
                self.turns = thread.get("turns") or []
                self.active_turn = next((t.get("id") for t in reversed(self.turns)
                                         if t.get("status") == "inProgress"), None)
                changed = True
            except RPCError:
                pass
        for _ in range(1000):
            try:
                event = self.rpc.events.get_nowait()
            except queue.Empty:
                break
            method = event.get("method", "")
            params = event.get("params") or {}
            if method == "kai/disconnected":
                self.notice = "App Server disconnected; restart Kai"
                changed = True
                continue
            if method in {"account/rateLimits/updated", "account/updated"}:
                self.rate_limits_changed = True
                continue
            if "id" in event and method:
                self.approvals.append(event)
                self.notice = "Approval needed: /approve or /reject"
                changed = True
                continue
            if method == "serverRequest/resolved":
                request_id = params.get("requestId")
                self.approvals = collections.deque(a for a in self.approvals if a.get("id") != request_id)
                changed = True
            if params.get("threadId") != self.thread_id and (params.get("thread") or {}).get("id") != self.thread_id:
                if method in {"thread/started", "thread/archived", "thread/name/updated"}:
                    self.notice = "Task list changed; /chats to refresh"
                    changed = True
                continue
            if method == "turn/started":
                turn = params.get("turn") or {}
                self.active_turn = turn.get("id")
                if self.active_turn and all(t.get("id") != self.active_turn for t in self.turns):
                    self.turns.append(turn)
                changed = True
            elif method == "turn/completed":
                turn = params.get("turn") or {}
                current = self._turn(turn.get("id"))
                if current is not None:
                    current.update({k: v for k, v in turn.items() if k != "items"})
                if turn.get("id") == self.active_turn:
                    self.active_turn = None
                self.notice = "Turn " + str(turn.get("status", "completed"))
                changed = True
            elif method in {"item/started", "item/completed"}:
                turn = self._turn(params.get("turnId"))
                item = params.get("item") or {}
                if turn is not None and item.get("id"):
                    flush_text(item["id"])
                    items = turn.get("items")
                    if not isinstance(items, list):
                        items = []
                        turn["items"] = items
                    for i, existing in enumerate(items):
                        if existing.get("id") == item["id"]:
                            # Started/completed snapshots can leave streamed fields null.
                            # Keep any text/output already assembled from deltas.
                            items[i] = {**existing, **{key: value for key, value in item.items()
                                                      if value is not None}}
                            break
                    else:
                        items.append(item)
                    changed = True
            elif method in {"item/agentMessage/delta", "item/commandExecution/outputDelta"}:
                item = self._item(params.get("itemId"))
                delta = params.get("delta")
                if item is not None and isinstance(delta, str):
                    key = "text" if "agentMessage" in method else "aggregatedOutput"
                    identity = (params["itemId"], key)
                    if identity not in pending_text:
                        pending_text[identity] = item, []
                    pending_text[identity][1].append(delta)
                    changed = True
            elif method == "thread/status/changed":
                self.thread["status"] = params.get("status")
                changed = True
            elif method == "error":
                self.notice = display_text((params.get("error") or {}).get("message"), 160)
                changed = True
        flush_text()
        if changed:
            self.dirty = True

    def _turn(self, turn_id: str | None) -> dict[str, Any] | None:
        return next((turn for turn in self.turns if turn.get("id") == turn_id), None)

    def _item(self, item_id: str | None) -> dict[str, Any] | None:
        return next((item for item in all_items(self.turns) if item.get("id") == item_id), None)

    def lines(self) -> list[str]:
        if self.view == "help":
            return [
                "PROJECTS", "/projects                 list local folders", "/projects <number|path>   select folder",
                "", "TASKS", "/chats                    list tasks in folder", "/chat <id|number>         open task",
                "/new                      create task", "", "CONVERSATION", "/history  /prompts  /activity  /terminal",
                "/attach <image>           add image to next prompt", "/approve  /reject         decide pending request",
                "/interrupt                stop active turn", "", "WORKSPACE", "/files  /file <path>  /diff  /git",
                "", "OTHER", "/status  /help  /quit", "", "Type text to send. Type // to send text beginning with /.",
                "Up/Down scroll | PgUp/PgDn page | Enter submit | Ctrl-C quit",
            ]
        if self.view == "projects":
            return ["LOCAL PROJECTS", ""] + [
                f"{i:>2}  {'* ' if path == self.project else '  '}{path}"
                for i, path in enumerate(self.project_paths, 1)
            ] + ["", "Select with /projects <number|path>"]
        if self.view == "chats":
            rows = self._project_threads()
            return ["TASKS / " + str(self.project), ""] + [
                f"{i:>2}  {str(row.get('id', ''))[:12]}  {display_text(row.get('name') or row.get('preview') or 'Untitled', 70).splitlines()[0]}  / {(row.get('status') or {}).get('type', '?')}"
                for i, row in enumerate(rows, 1)
            ] + (["", "No tasks. Use /new."] if not rows else ["", "Open with /chat <number|id>"])
        if self.view == "prompts":
            if not self.thread_id:
                return ["No task selected. Use /chats or /new."]
            prompts = [item for item in all_items(self.turns) if item.get("type") == "userMessage"]
            lines = ["PROMPTS / " + (self.thread.get("name") or self.thread_id[:12]), ""]
            for index, item in enumerate(prompts, 1):
                prompt = prompt_lines(item)
                lines.extend([DisplayLine(f"{index}. {prompt[0]}", "prompt"), *prompt[1:], ""])
            return lines if prompts else lines + ["No prompts yet."]
        if self.view in {"history", "activity", "terminal"}:
            if not self.thread_id:
                return ["No task selected. Use /chats or /new."]
            lines = [self.view.upper() + " / " + (self.thread.get("name") or self.thread_id[:12]), ""]
            for turn in self.turns:
                for item in turn.get("items") or []:
                    lines.extend(item_lines(item, self.view))
            return lines if len(lines) > 2 else lines + ["Nothing here yet."]
        if self.view == "status":
            status = self.thread.get("status") or {}
            approval = self._approval_for_selected()
            lines = ["STATUS", "", "project  " + str(self.project),
                     "task     " + (self.thread_id or "none"),
                     "state    " + str(status.get("type", "idle")),
                     "access   " + ("read-only; desktop writer active" if self.read_only else "read/write"),
                     "turn     " + (self.active_turn or "none"),
                     "images   " + str(len(self.attachments))]
            if status.get("activeFlags"):
                lines.append("flags    " + ", ".join(status["activeFlags"]))
            if approval:
                params = approval.get("params") or {}
                lines += ["", "APPROVAL", str(approval.get("method", "")),
                          display_text(params.get("reason") or params.get("command") or params.get("permissions") or params, 900),
                          "Use /approve or /reject"]
            return lines
        if self.view == "files":
            if self.file_tree_root != self.project:
                self.refresh_file_tree()
            lines = ["FILES / " + str(self.project), ""]
            for path, depth, is_dir in self.file_visible_entries:
                marker = ("▶ " if path in self.file_collapsed else "▼ ") if is_dir else "  "
                lines.append("  " * depth + marker + path.name)
            return lines + (["", "No files found."] if not self.file_visible_entries else [])
        if self.view == "file":
            path = self.file_path
            try:
                data = path.read_bytes()[:262145]
                if b"\x00" in data[:4096]:
                    content = "Binary file; preview unavailable"
                else:
                    content = data[:262144].decode("utf-8", "replace")
                    if len(data) > 262144:
                        content += "\n... [preview truncated]"
                return [str(path), ""] + content.splitlines()
            except OSError as exc:
                return [str(path), "", str(exc)]
        if self.view == "git":
            code, root = git(self.project, "rev-parse", "--show-toplevel")
            if code:
                return ["GIT", "", root or "Not a Git repository"]
            _, branch = git(self.project, "branch", "--show-current")
            if not branch.strip():
                _, branch = git(self.project, "rev-parse", "--short", "HEAD")
                branch = "detached @ " + branch.strip()
            _, status = git(self.project, "status", "--short", "--untracked-files=normal")
            _, worktrees = git(self.project, "worktree", "list", "--porcelain")
            return ["GIT", "", "root  " + root.strip(), "branch  " + branch.strip(), "", "CHANGES",
                    status.strip() or "Clean", "", "WORKTREES"] + worktrees.strip().splitlines()
        if self.view == "diff":
            code, unstaged = git(self.project, "diff", "--no-ext-diff", "--")
            if code:
                return ["DIFF", "", unstaged or "Not a Git repository"]
            _, staged = git(self.project, "diff", "--cached", "--no-ext-diff", "--")
            _, untracked = git(self.project, "ls-files", "--others", "--exclude-standard")
            return ["DIFF", "", "UNSTAGED", unstaged.strip() or "No changes", "", "STAGED",
                    staged.strip() or "No changes", "", "UNTRACKED", untracked.strip() or "None"]
        return []


def _safe_add(stdscr: Any, y: int, x: int, value: str, width: int, attr: int = 0) -> None:
    if width <= 0:
        return
    try:
        stdscr.addnstr(y, x, value.replace("\t", "    "), width, attr)
    except curses.error:
        pass


def run_ui(stdscr: Any, app: Kai) -> None:
    curses.curs_set(1)
    prompt_attr = curses.A_REVERSE | curses.A_DIM
    if curses.has_colors():
        try:
            curses.start_color()
            curses.use_default_colors()
            if curses.COLORS >= 256:
                curses.init_pair(1, 15, 238)
                prompt_attr = curses.color_pair(1)
        except curses.error:
            pass
    stdscr.timeout(100)
    stdscr.keypad(True)
    curses.mousemask(curses.ALL_MOUSE_EVENTS)
    curses.mouseinterval(200)
    input_text = ""
    last_size = (0, 0)
    wrapped_body: list[DisplayLine] = []
    wrapped_width = 0
    wrapped_view = ""
    row_targets: dict[int, int] = {}
    nav_targets: list[tuple[int, int, str]] = []
    while app.running:
        app.drain()
        height, width = stdscr.getmaxyx()
        if (height, width) != last_size:
            app.dirty = True
            last_size = (height, width)
        if height < 10 or width < 36:
            stdscr.erase()
            _safe_add(stdscr, 0, 0, "Kai needs a larger terminal", max(0, width - 1))
            stdscr.refresh()
        else:
            menu_view = app.view in {"projects", "chats", "files"}
            if app.dirty and not menu_view:
                app.body = app.lines()
                app.dirty = False
                wrapped_width = 0
            stdscr.erase()
            if app.view == "projects":
                title = " KAI  /  Projects"
            elif app.view == "chats":
                title = " KAI  /  " + app.project.name + "  /  Chats"
            elif app.view == "files":
                title = " KAI  /  " + app.project.name + "  /  Files"
            else:
                name = app.thread.get("name") or (app.thread_id or app.view)
                title = " KAI  /  " + app.project.name + "  /  " + str(name).replace("\n", " ")
            _safe_add(stdscr, 0, 0, title, width - 1, curses.A_REVERSE | curses.A_BOLD)
            row_targets = {}
            if menu_view:
                entries = (app.project_paths if app.view == "projects" else
                           app._project_threads() if app.view == "chats" else app.file_visible_entries)
                heading = {"projects": "Projects", "chats": "Chats", "files": "Files"}[app.view]
                _safe_add(stdscr, 2, 2, heading, width - 4, curses.A_BOLD)
                hint = ("↑↓ select  ·  Enter/click open  ·  → fold  ·  ← unfold"
                        if app.view == "files" else "Click, or use arrows and Enter")
                _safe_add(stdscr, 3, 2, hint, width - 4, curses.A_DIM)
                first_y = 5
                capacity = max(1, height - 5 - first_y)
                selected = app.menu_selection[app.view]
                if entries:
                    selected = max(0, min(selected, len(entries) - 1))
                    app.menu_selection[app.view] = selected
                    offset = app.menu_offset[app.view]
                    if selected < offset:
                        offset = selected
                    if selected >= offset + capacity:
                        offset = selected - capacity + 1
                    offset = max(0, min(offset, max(0, len(entries) - capacity)))
                    app.menu_offset[app.view] = offset
                    for visible_index, index in enumerate(range(offset, min(len(entries), offset + capacity))):
                        y = first_y + visible_index
                        if app.view == "projects":
                            path = entries[index]
                            count = sum(1 for row in app.threads if row.get("cwd") == str(path))
                            duplicate = sum(1 for candidate in entries if candidate.name == path.name) > 1
                            name = (path.name or "/") + (" / " + path.parent.name if duplicate else "")
                            label = f" {index + 1:>2}  {name}    {count} {'chat' if count == 1 else 'chats'} "
                        elif app.view == "chats":
                            row = entries[index]
                            name = str(row.get("name") or row.get("preview") or "Untitled").replace("\n", " ")
                            label = f" {index + 1:>2}  {name}    {str(row.get('id', ''))[:8]} "
                        else:
                            path, depth, is_dir = entries[index]
                            marker = ("▶ " if path in app.file_collapsed else "▼ ") if is_dir else "  "
                            label = " " + "  " * depth + marker + path.name
                        _safe_add(stdscr, y, 1, label, width - 2,
                                  curses.A_REVERSE if index == selected else 0)
                        row_targets[y] = index
                else:
                    empty = ("No chats yet. Choose New chat." if app.view == "chats" else
                             "No files found." if app.view == "files" else "No projects found.")
                    _safe_add(stdscr, first_y, 2, empty, width - 4, curses.A_DIM)
            else:
                body_top, body_height = 2, height - 7
                if wrapped_width != width or wrapped_view != app.view:
                    wrapped_body = wrap_display_lines(app.body, width - 3)
                    wrapped_width = width
                    wrapped_view = app.view
                max_scroll = max(0, len(wrapped_body) - body_height)
                app.scroll = min(app.scroll, max_scroll)
                start = max_scroll - app.scroll
                for i, line in enumerate(wrapped_body[start:start + body_height]):
                    styles = {
                        "heading": curses.A_BOLD | curses.A_UNDERLINE,
                        "code": curses.A_DIM,
                        "quote": curses.A_DIM,
                        "rule": curses.A_DIM,
                        "table": curses.A_BOLD,
                        "prompt": prompt_attr,
                    }
                    shown = line.ljust(width - 3) if getattr(line, "style", "") == "prompt" else line
                    _safe_add(stdscr, body_top + i, 1, shown, width - 3,
                              styles.get(getattr(line, "style", ""), 0))
            nav_targets = []
            nav: list[tuple[str, str]] = []
            if app.view == "chats":
                nav = ([("Projects", "back"), ("New", "new"), ("Quit", "quit")]
                       if width < 58 else
                       [("Back to projects", "back"), ("New chat", "new"), ("Quit", "quit")])
            elif app.view == "projects":
                nav = [("Quit", "quit")]
            elif app.view in {"files", "file"}:
                destination = "files" if app.view == "file" and app.file_origin == "files" else "chat"
                nav = ([("Back", "back"), ("Quit", "quit")] if width < 58 else
                       [("Back to " + destination, "back"), ("Quit", "quit")])
            else:
                nav = ([("Chats", "back"), ("Quit", "quit")]
                       if width < 58 else [("Back to chats", "back"), ("Quit", "quit")])
            x = 1
            for label, action in nav:
                button = "[ " + label + " ]"
                if x + len(button) >= width - 1:
                    break
                _safe_add(stdscr, height - 4, x, button, len(button), curses.A_REVERSE)
                nav_targets.append((x, x + len(button), action))
                x += len(button) + 2
            status = (str(app.project_paths[app.menu_selection["projects"]])
                      if app.view == "projects" and app.project_paths else app.notice)
            if app.view == "files" and app.file_visible_entries:
                status = str(app.file_visible_entries[app.menu_selection["files"]][0])
            if app.attachments:
                status += f"  |  {len(app.attachments)} image(s) queued"
            if app._approval_for_selected():
                status += "  |  APPROVAL NEEDED"
            _safe_add(stdscr, height - 3, 1, status, width - 3, curses.A_DIM)
            _safe_add(stdscr, height - 2, 0, "-" * (width - 1), width - 1, curses.A_DIM)
            prefix = "> "
            visible = input_text[-max(0, width - len(prefix) - 3):]
            _safe_add(stdscr, height - 1, 1, prefix + visible, width - 3, curses.A_BOLD)
            try:
                stdscr.move(height - 1, min(width - 2, 1 + len(prefix) + len(visible)))
            except curses.error:
                pass
            stdscr.refresh()
        try:
            key = stdscr.get_wch()
        except curses.error:
            continue
        if key == curses.KEY_MOUSE:
            try:
                _, mx, my, _, state = curses.getmouse()
            except curses.error:
                continue
            if state & getattr(curses, "BUTTON4_PRESSED", 0):
                if app.view in {"projects", "chats", "files"}:
                    app.menu_selection[app.view] = max(0, app.menu_selection[app.view] - 3)
                else:
                    app.scroll += 3
                continue
            if state & getattr(curses, "BUTTON5_PRESSED", 0):
                if app.view in {"projects", "chats", "files"}:
                    size = (len(app.project_paths) if app.view == "projects" else
                            len(app._project_threads()) if app.view == "chats" else
                            len(app.file_visible_entries))
                    app.menu_selection[app.view] = min(max(0, size - 1), app.menu_selection[app.view] + 3)
                else:
                    app.scroll = max(0, app.scroll - 3)
                continue
            if not state & (curses.BUTTON1_CLICKED | curses.BUTTON1_DOUBLE_CLICKED):
                continue
            try:
                if my == height - 4:
                    action = next((name for start, end, name in nav_targets if start <= mx < end), None)
                    if action == "back":
                        app.back()
                    elif action == "new":
                        app.new_chat()
                    elif action == "quit":
                        app.running = False
                elif my in row_targets and app.view in {"projects", "chats", "files"}:
                    app.menu_selection[app.view] = row_targets[my]
                    app.open_selected()
            except (RPCError, ValueError, OSError) as exc:
                app.notice = str(exc)
                app.dirty = True
            continue
        if key in ("\n", "\r", curses.KEY_ENTER):
            line = input_text.strip()
            input_text = ""
            if line:
                try:
                    app.command(line)
                except (RPCError, ValueError, OSError) as exc:
                    app.notice = str(exc)
                    app.dirty = True
                    if not line.startswith("/"):
                        input_text = line
            elif app.view in {"projects", "chats", "files"}:
                try:
                    if app.view == "chats" and not app._project_threads():
                        app.new_chat()
                    else:
                        app.open_selected()
                except (RPCError, ValueError, OSError) as exc:
                    app.notice = str(exc)
                    app.dirty = True
        elif key in ("\x7f", "\b", curses.KEY_BACKSPACE):
            if input_text:
                input_text = input_text[:-1]
            else:
                app.back()
        elif key == "\x1b":
            app.back()
        elif key == "\x03":
            app.running = False
        elif key == curses.KEY_UP:
            if app.view in {"projects", "chats", "files"}:
                app.menu_selection[app.view] = max(0, app.menu_selection[app.view] - 1)
            else:
                app.scroll += 1
        elif key == curses.KEY_DOWN:
            if app.view in {"projects", "chats", "files"}:
                size = (len(app.project_paths) if app.view == "projects" else
                        len(app._project_threads()) if app.view == "chats" else
                        len(app.file_visible_entries))
                app.menu_selection[app.view] = min(max(0, size - 1), app.menu_selection[app.view] + 1)
            else:
                app.scroll = max(0, app.scroll - 1)
        elif key == curses.KEY_PPAGE:
            if app.view in {"projects", "chats", "files"}:
                app.menu_selection[app.view] = max(0, app.menu_selection[app.view] - max(1, height - 10))
            else:
                app.scroll += max(1, height - 7)
        elif key == curses.KEY_NPAGE:
            if app.view in {"projects", "chats", "files"}:
                size = (len(app.project_paths) if app.view == "projects" else
                        len(app._project_threads()) if app.view == "chats" else
                        len(app.file_visible_entries))
                app.menu_selection[app.view] = min(max(0, size - 1), app.menu_selection[app.view] + max(1, height - 10))
            else:
                app.scroll = max(0, app.scroll - max(1, height - 7))
        elif key == curses.KEY_RIGHT and app.view == "files":
            app.toggle_selected_directory(collapse=True)
        elif key == curses.KEY_LEFT and app.view == "files":
            app.toggle_selected_directory(collapse=False)
        elif isinstance(key, str) and key.isprintable():
            input_text += key
            visible, dropped = consume_dropped_images(input_text, app.project)
            if dropped:
                labels = []
                for path in dropped:
                    app.attachments.append(path)
                    labels.append(f"[Image {len(app.attachments)}]")
                # These labels are deliberately human-readable references. The exact paths
                # live in app.attachments/localImage so this can become a real image widget
                # when Kai moves beyond curses, without touching chat persistence.
                input_text = " ".join(part for part in (visible, *labels) if part)
                app.notice = f"Attached {len(dropped)} image(s); send when ready"
                app.dirty = True


def main() -> int:
    try:
        locale.setlocale(locale.LC_ALL, "")
    except locale.Error:
        pass
    parser = argparse.ArgumentParser(description="Kai: Codex tasks in a clean terminal UI")
    parser.add_argument("--cwd", type=Path, default=Path.cwd(), help="starting project folder")
    parser.add_argument("--codex", default="codex", help="Codex executable")
    args = parser.parse_args()
    cwd = args.cwd.expanduser().resolve()
    if not cwd.is_dir():
        parser.error("--cwd must be an existing folder")
    rpc = None
    try:
        rpc = AppServer(args.codex)
        app = Kai(rpc, cwd)
        curses.wrapper(run_ui, app)
    except (OSError, RPCError) as exc:
        print(f"Kai: {exc}", file=sys.stderr)
        return 1
    finally:
        if rpc is not None:
            rpc.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

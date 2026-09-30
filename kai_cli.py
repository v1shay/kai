#!/usr/bin/env python3
"""Non-curses command-line frontend for Kai.

This process talks directly to its own Codex App Server instance.  It does not
start, inspect, send keystrokes to, or otherwise control the Kai TUI.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys
import time
from typing import Any, Iterable

from kai import AppServer, Kai, RPCError


def snapshot(app: Kai) -> dict[str, Any]:
    """Return a stable, machine-readable representation of the current view."""
    approval = app._approval_for_selected()
    return {
        "view": app.view,
        "notice": app.notice,
        "project": str(app.project),
        "threadId": app.thread_id,
        "activeTurnId": app.active_turn,
        "readOnly": app.read_only,
        "pendingApproval": ({
            "id": approval.get("id"),
            "method": approval.get("method"),
            "params": approval.get("params") or {},
        } if approval else None),
        "attachments": [str(path) for path in app.attachments],
        "lines": [str(line) for line in app.lines()],
    }


def wait_for_turn(app: Kai, timeout: float, poll_interval: float = 0.02) -> str:
    """Drain App Server events until the selected turn ends or needs a decision."""
    if timeout <= 0 or not app.active_turn:
        app.drain()
        return "not_waited"
    deadline = time.monotonic() + timeout
    while app.active_turn and time.monotonic() < deadline:
        app.drain()
        if app._approval_for_selected():
            return "approval_needed"
        if app.active_turn:
            time.sleep(poll_interval)
    app.drain()
    return "completed" if not app.active_turn else "timed_out"


def execute(app: Kai, command: str, wait: float = 0) -> tuple[dict[str, Any], str]:
    """Run one command through the same state machine used by the TUI."""
    app.drain()
    app.command(command)
    wait_result = wait_for_turn(app, wait)
    return snapshot(app), wait_result


def _text_frame(state: dict[str, Any]) -> str:
    return "\n".join(state["lines"])


def _emit(state: dict[str, Any], output_format: str, command: str | None = None,
          wait_result: str | None = None) -> None:
    if output_format == "json":
        value = dict(state)
        if command is not None:
            value["command"] = command
        if wait_result is not None:
            value["wait"] = wait_result
        print(json.dumps(value, ensure_ascii=False, separators=(",", ":")))
    else:
        print(_text_frame(state))


def _commands(args: argparse.Namespace) -> Iterable[str]:
    if args.command:
        yield from args.command
        return
    if not sys.stdin.isatty():
        for raw in sys.stdin:
            command = raw.rstrip("\r\n")
            if command:
                yield command


def _repl(app: Kai, output_format: str, wait: float) -> int:
    _emit(snapshot(app), output_format)
    while app.running:
        try:
            command = input("kai> ")
        except EOFError:
            print()
            break
        except KeyboardInterrupt:
            print()
            return 130
        if not command:
            app.drain()
            _emit(snapshot(app), output_format)
            continue
        try:
            state, wait_result = execute(app, command, wait)
        except (RPCError, ValueError, OSError) as exc:
            print(f"Kai CLI: {exc}", file=sys.stderr)
            continue
        _emit(state, output_format, command, wait_result)
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Kai without curses: script the exact Kai command/state layer",
        epilog=("Commands are the same as the TUI: /projects, /chats, /chat, /new, "
                "/history, /prompts, /activity, /terminal, /files, /file, /diff, "
                "/git, /attach, /approve, /reject, /interrupt, /status, /help, /quit."),
    )
    parser.add_argument("--cwd", type=Path, default=Path.cwd(), help="starting project folder")
    parser.add_argument("--codex", default="codex", help="Codex executable")
    parser.add_argument("-c", "--command", action="append",
                        help="command or prompt to run; repeat to keep one Kai session")
    parser.add_argument("--format", choices=("text", "json"), default="text",
                        help="human view text or one compact JSON object per emitted view")
    parser.add_argument("--emit", choices=("final", "each"), default="final",
                        help="print the final view or the view after every command")
    parser.add_argument("--wait", type=float, default=0, metavar="SECONDS",
                        help="after a prompt, wait this long for completion or approval")
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    cwd = args.cwd.expanduser().resolve()
    if not cwd.is_dir():
        parser.error("--cwd must be an existing folder")
    if args.wait < 0:
        parser.error("--wait must be zero or greater")

    rpc: AppServer | None = None
    try:
        rpc = AppServer(args.codex)
        app = Kai(rpc, cwd, service_name="kai_cli")
        commands = list(_commands(args))
        if not commands and sys.stdin.isatty():
            return _repl(app, args.format, args.wait)
        if not commands:
            _emit(snapshot(app), args.format)
            return 0

        final_state: dict[str, Any] | None = None
        final_wait = "not_waited"
        for command in commands:
            final_state, final_wait = execute(app, command, args.wait)
            if args.emit == "each":
                _emit(final_state, args.format, command, final_wait)
        if args.emit == "final" and final_state is not None:
            _emit(final_state, args.format, commands[-1], final_wait)
        return 0
    except (RPCError, ValueError, OSError) as exc:
        print(f"Kai CLI: {exc}", file=sys.stderr)
        return 1
    finally:
        if rpc is not None:
            rpc.close()


if __name__ == "__main__":
    raise SystemExit(main())

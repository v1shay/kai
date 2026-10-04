#!/usr/bin/env python3
"""Small Streamable HTTP/stdio MCP bridge from OpenAI dots to Kai."""

from __future__ import annotations

import argparse
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import sys
from typing import Any

from kai_dot import DotEventStore, FINAL_SCENES, SCENES


PROTOCOL_VERSION = "2025-06-18"

TOOLS = [
    {
        "name": "kai_activity",
        "description": (
            "Show the paired OpenAI dot's current work in the user's Kai notch companion. "
            "Call at the beginning of assigned work and whenever the kind of work changes. "
            "Send only a short status label; never send prompts, secrets, or full tool arguments."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "description": "Stable id for this dot task."},
                "title": {"type": "string", "description": "Short task name."},
                "state": {"type": "string", "enum": sorted(SCENES)},
                "detail": {"type": "string", "description": "Brief user-visible activity, ideally under 80 characters."},
            },
            "required": ["session_id", "title", "state"],
            "additionalProperties": False,
        },
    },
    {
        "name": "kai_complete",
        "description": (
            "Finish a previously reported Kai dot task. Always call once when the task succeeds, "
            "fails, or is declined so the notch does not remain active."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": {"type": "string", "description": "The id used with kai_activity."},
                "title": {"type": "string", "description": "Short task name."},
                "outcome": {"type": "string", "enum": sorted(FINAL_SCENES)},
                "detail": {"type": "string", "description": "Brief final status."},
            },
            "required": ["session_id", "outcome"],
            "additionalProperties": False,
        },
    },
]


def result(request: dict[str, Any], store: DotEventStore) -> dict[str, Any] | None:
    request_id, method = request.get("id"), request.get("method")
    if request_id is None:
        return None
    response: dict[str, Any] = {"jsonrpc": "2.0", "id": request_id}
    try:
        if method == "initialize":
            response["result"] = {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {"name": "kai-dot-bridge", "version": "1.0.0"},
                "instructions": (
                    "Use kai_activity for meaningful task-state changes and kai_complete exactly once "
                    "when work ends. Do not send conversation text or secrets."
                ),
            }
        elif method == "ping":
            response["result"] = {}
        elif method == "tools/list":
            response["result"] = {"tools": TOOLS}
        elif method == "tools/call":
            params = request.get("params") or {}
            args = params.get("arguments") or {}
            name = params.get("name")
            if not isinstance(args, dict):
                raise ValueError("Tool arguments must be an object")
            if name == "kai_activity":
                event = store.append(str(args.get("session_id") or ""), str(args.get("title") or "Dot"),
                                     str(args.get("state") or ""), str(args.get("detail") or "Working"))
            elif name == "kai_complete":
                event = store.append(str(args.get("session_id") or ""), str(args.get("title") or "Dot"),
                                     str(args.get("outcome") or "success"), str(args.get("detail") or "Complete"))
            else:
                raise ValueError("Unknown tool")
            response["result"] = {
                "content": [{"type": "text", "text": f"Kai updated: {event['scene']}"}],
                "structuredContent": {"accepted": True, "scene": event["scene"]},
            }
        else:
            response["error"] = {"code": -32601, "message": "Method not found"}
    except (TypeError, ValueError) as exc:
        response["error"] = {"code": -32602, "message": str(exc)}
    return response


def serve_stdio(store: DotEventStore) -> int:
    for line in sys.stdin:
        try:
            request = json.loads(line)
            response = result(request, store) if isinstance(request, dict) else None
        except ValueError as exc:
            response = {"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": str(exc)}}
        if response is not None:
            print(json.dumps(response, separators=(",", ":")), flush=True)
    return 0


def handler(store: DotEventStore, token: str) -> type[BaseHTTPRequestHandler]:
    class MCPHandler(BaseHTTPRequestHandler):
        server_version = "KaiMCP/1.0"

        def log_message(self, format: str, *args: Any) -> None:
            print(format % args, file=sys.stderr)

        def _authorized(self) -> bool:
            bearer = secrets.compare_digest(self.headers.get("Authorization", ""), "Bearer " + token)
            developer_path = secrets.compare_digest(self.path.rstrip("/"), f"/{token}/mcp")
            return bearer or developer_path

        def do_GET(self) -> None:
            if self.path == "/health":
                body = b'{"ok":true}'
                self.send_response(HTTPStatus.OK)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers(); self.wfile.write(body)
                return
            self.send_error(HTTPStatus.METHOD_NOT_ALLOWED)

        def do_POST(self) -> None:
            if self.path.rstrip("/") not in {"/mcp", f"/{token}/mcp"}:
                self.send_error(HTTPStatus.NOT_FOUND); return
            if not self._authorized():
                self.send_error(HTTPStatus.UNAUTHORIZED); return
            try:
                size = int(self.headers.get("Content-Length", "0"))
                if size <= 0 or size > 1_000_000:
                    raise ValueError("Invalid request size")
                request = json.loads(self.rfile.read(size))
                if not isinstance(request, dict):
                    raise ValueError("Expected a JSON-RPC object")
                response = result(request, store)
            except (ValueError, UnicodeDecodeError) as exc:
                response = {"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": str(exc)}}
            if response is None:
                self.send_response(HTTPStatus.ACCEPTED); self.end_headers(); return
            body = json.dumps(response, separators=(",", ":")).encode()
            self.send_response(HTTPStatus.OK)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers(); self.wfile.write(body)

    return MCPHandler


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stdio", action="store_true", help="serve MCP over standard input/output")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8766)
    parser.add_argument("--token", default=os.environ.get("KAI_DOT_TOKEN"))
    parser.add_argument("--events", type=Path, default=None)
    args = parser.parse_args()
    store = DotEventStore(args.events)
    if args.stdio:
        return serve_stdio(store)
    if not args.token:
        parser.error("set KAI_DOT_TOKEN or pass --token for HTTP mode")
    server = ThreadingHTTPServer((args.host, args.port), handler(store, args.token))
    print(f"Kai Dot MCP listening on http://{args.host}:{args.port}/{args.token}/mcp", file=sys.stderr)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

from pathlib import Path
import json
import os
import time
import queue
import subprocess
import tempfile
import unittest

from kai import (DisplayLine, Kai, consume_dropped_images, image_file, item_lines,
                 markdown_lines, resolve_codex_binary, user_message_text,
                 voice_transcript_lines, wrap_display_lines)
from kai_cli import execute, snapshot, wait_for_turn
from kai_bridge import (activity as bridge_activity, apply as bridge_apply,
                        remaining_limit_percent, snapshot as bridge_snapshot)
from kai_activity import SessionActivityMonitor


class FakeServer:
    def __init__(self, cwd):
        self.cwd = str(cwd)
        self.events = queue.Queue()
        self.calls = []
        self.answers = []
        self.writer_busy = False
        self.resume_partial = False
        self.rows = [{"id": "task-123", "cwd": self.cwd, "name": "Existing task",
                      "status": {"type": "idle"}}]

    def call(self, method, params=None, timeout=25):
        params = params or {}
        self.calls.append((method, params))
        if method == "thread/list":
            return {"data": self.rows, "nextCursor": None}
        if method == "thread/read":
            return {"thread": {**self.rows[0], "turns": [
                {"id": "old-turn", "status": "completed", "items": [
                    {"id": "u1", "type": "userMessage", "content": [{"type": "text", "text": "Hello"}]},
                    {"id": "a1", "type": "agentMessage", "phase": "final_answer", "text": "Hi"},
                    {"id": "c1", "type": "commandExecution", "command": "pwd", "status": "completed",
                     "aggregatedOutput": self.cwd},
                ]}]}}
        if method == "thread/resume":
            if self.writer_busy:
                from kai import RPCError
                raise RPCError("thread/resume: active writer")
            if self.resume_partial:
                return {"thread": {**self.rows[0], "turns": [
                    {"id": "old-turn", "status": "completed", "items": [
                        {"id": "u2", "type": "userMessage", "content": [{"type": "text", "text": "Later"}]},
                    ]},
                ]}}
            return {"thread": self.rows[0]}
        if method == "thread/start":
            row = {"id": "new-task", "cwd": params["cwd"], "name": "New task", "status": {"type": "idle"}}
            self.rows.insert(0, row)
            return {"thread": row}
        if method == "turn/start":
            return {"turn": {"id": "new-turn", "status": "inProgress", "items": []}}
        if method == "turn/steer":
            return {"turnId": "new-turn"}
        return {}

    def respond(self, request_id, answer):
        self.answers.append((request_id, answer))


class KaiTests(unittest.TestCase):
    def test_codex_binary_skips_broken_path_launcher(self):
        with tempfile.TemporaryDirectory() as directory:
            broken = Path(directory) / 'broken-codex'
            working = Path(directory) / 'working-codex'
            broken.write_text('#!/bin/sh\nexit 1\n')
            working.write_text('#!/bin/sh\nprintf "codex-cli 0.158.0\\n"\n')
            broken.chmod(0o755)
            working.chmod(0o755)
            self.assertEqual(resolve_codex_binary(candidates=[str(broken), str(working)]), str(working))

    def test_markdown_preserves_tex_for_native_math_renderer(self):
        lines = markdown_lines('Inline \\(x_1 + \\alpha\\), $y^2$, and **bold**.\n'
                               '$$\n\\frac{a}{b}\n$$\n'
                               '\\[\n\\sum_{i=1}^{n} i\n\\]\n'
                               '```text\n$literal$\n```')
        self.assertEqual([str(line) for line in lines[:7]], [
            'Inline \\(x_1 + \\alpha\\), $y^2$, and bold.',
            '$$', '\\frac{a}{b}', '$$', '\\[', '\\sum_{i=1}^{n} i', '\\]'])
        self.assertEqual((str(lines[7]), lines[7].style), ('$literal$', 'code'))
        self.assertFalse(lines[7].math_enabled)
        self.assertFalse(markdown_lines('Use `$x$` literally.')[0].math_enabled)
        self.assertTrue(markdown_lines('Render $x$ normally.')[0].math_enabled)
        self.assertEqual(str(markdown_lines('$$\na &= b \\\\ c &= d\n$$')[1]), r'a &= b \\ c &= d')

    def test_concurrent_session_activity_switches_on_new_tool_and_finishes(self):
        with tempfile.TemporaryDirectory() as directory:
            paths = [Path(directory) / name for name in ("one.jsonl", "two.jsonl")]
            rows = [{"id": name, "name": name, "path": str(path)}
                    for name, path in zip(("one", "two"), paths)]
            def append(index, stamp, payload):
                with paths[index].open("a") as stream:
                    stream.write(json.dumps({"timestamp": stamp, "type": "event_msg", "payload": payload}) + "\n")
            monitor = SessionActivityMonitor()
            append(0, "2026-09-27T21:00:00.000Z", {"type": "task_started", "turn_id": "a"})
            self.assertEqual([t["id"] for t in monitor.poll(rows)["activeTasks"]], ["one"])
            append(1, "2026-09-27T21:00:01.000Z", {"type": "task_started", "turn_id": "b"})
            self.assertEqual(monitor.poll(rows)["focusThreadId"], "one")
            append(0, "2026-09-27T21:00:02.000Z", {"type": "item_started", "item": {"type": "CommandExecution", "command": "pwd"}})
            state = monitor.poll(rows)
            self.assertEqual(state["focusThreadId"], "one")
            self.assertEqual(state["activeTasks"][0]["scene"], "terminal")
            append(1, "2026-09-27T21:00:02.500Z", {"type": "custom_tool_call", "name": "exec", "input": "await tools.apply_patch(patch)"})
            state = monitor.poll(rows)
            self.assertEqual(state["focusThreadId"], "two")
            self.assertEqual(state["activeTasks"][0]["scene"], "files")
            append(1, "2026-09-27T21:00:02.700Z", {"type": "custom_tool_call", "name": "exec",
                                                     "input": 'await tools.exec_command({cmd:"swift test"})'})
            state = monitor.poll(rows)
            self.assertEqual(state["activeTasks"][0]["text"], "swift test")
            append(0, "2026-09-27T21:00:02.800Z", {"type": "item_started", "item": {"type": "WebSearch", "query": "docs"}})
            self.assertEqual(monitor.poll(rows)["focusThreadId"], "one")
            append(1, "2026-09-27T21:00:02.900Z", {"type": "item_completed", "item": {"type": "CommandExecution", "command": "swift test"}})
            self.assertEqual(monitor.poll(rows)["focusThreadId"], "one")
            append(0, "2026-09-27T21:00:03.000Z", {"type": "task_complete", "turn_id": "a"})
            self.assertEqual(monitor.poll(rows)["focusThreadId"], "two")
            os.utime(paths[1], (time.time()-700, time.time()-700))
            self.assertEqual(monitor.poll(rows)["focusThreadId"], "two")
            append(1, "2026-09-27T21:00:04.000Z", {"type": "task_complete", "turn_id": "b"})
            self.assertEqual(monitor.poll(rows)["activeTasks"], [])

    def test_session_scanner_ignores_lifecycle_words_inside_tool_input(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "task.jsonl"
            with path.open("w") as stream:
                stream.write(json.dumps({"timestamp": "2026-09-27T21:00:00.000Z", "payload":
                                         {"type": "task_started", "turn_id": "a"}}) + "\n")
                stream.write(json.dumps({"timestamp": "2026-09-27T21:00:01.000Z", "payload":
                                         {"type": "custom_tool_call", "name": "exec",
                                          "input": '"type":"task_complete" ' + "x" * 70000}}) + "\n")
            state = SessionActivityMonitor().poll([{"id": "task", "name": "Task", "path": str(path)}])
            self.assertEqual(state["focusThreadId"], "task")

    def test_session_scanner_recovers_active_turn_larger_than_two_megabytes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "long.jsonl"
            with path.open("w") as stream:
                stream.write(json.dumps({"timestamp": "2026-09-27T21:00:00.000Z", "payload":
                                         {"type": "task_started", "turn_id": "long"}}) + "\n")
                stream.write(json.dumps({"timestamp": "2026-09-27T21:00:01.000Z", "payload":
                                         {"type": "message", "text": "x" * 2_200_000}}) + "\n")
                stream.write(json.dumps({"timestamp": "2026-09-27T21:00:02.000Z", "payload":
                                         {"type": "custom_tool_call", "name": "exec", "input": "await tools.exec_command({cmd:\"pwd\"})"}}) + "\n")
            state = SessionActivityMonitor().poll([{"id": "task", "name": "Task", "path": str(path)}])
            self.assertEqual(state["focusThreadId"], "task")
            self.assertEqual(state["activeTasks"][0]["text"], "pwd")

    def test_running_session_survives_thread_list_gap_until_lifecycle_completion(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "task.jsonl"
            def append(payload):
                with path.open("a") as stream:
                    stream.write(json.dumps({"timestamp": "2026-09-27T21:00:00.000Z", "payload": payload}) + "\n")
            row = {"id": "task", "name": "Stable chat", "path": str(path)}
            monitor = SessionActivityMonitor()
            append({"type": "task_started", "turn_id": "turn"})
            self.assertEqual(monitor.poll([row])["focusThreadId"], "task")
            self.assertEqual(monitor.poll([])["focusThreadId"], "task")
            append({"type": "task_complete", "turn_id": "turn"})
            self.assertEqual(monitor.poll([row])["activeTasks"], [])

    def test_remaining_limit_percent_prefers_codex_bucket_and_rejects_missing_values(self):
        limits = {"rateLimits": {"primary": {"usedPercent": 90}},
                  "rateLimitsByLimitId": {"codex": {"primary": {"usedPercent": 24.6}}}}
        self.assertEqual(remaining_limit_percent(limits), 75)
        self.assertEqual(remaining_limit_percent({"rateLimits": {"primary": {"usedPercent": 90}}}), 10)
        self.assertIsNone(remaining_limit_percent({"rateLimits": {"primary": None}}))

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = Path(self.tmp.name)
        subprocess.run(["git", "init", "-q", str(self.path)], check=True)
        (self.path / "note.txt").write_text("hello world\n")
        (self.path / "pixel.png").write_bytes(b"\x89PNG\r\n\x1a\n" + b"0" * 20)
        (self.path / "photo space.png").write_bytes(b"\x89PNG\r\n\x1a\n" + b"1" * 20)
        self.rpc = FakeServer(self.path)
        self.app = Kai(self.rpc, self.path)

    def tearDown(self):
        self.tmp.cleanup()

    def test_all_requested_views_and_navigation(self):
        for command in ("/projects", "/chats", "/chat task-123", "/history",
                        "/activity", "/terminal", "/files", "/file note.txt",
                        "/diff", "/git", "/status", "/help"):
            self.app.command(command)
            self.assertIsInstance(self.app.lines(), list, command)
        self.app.command("/file note.txt")
        self.assertIn("hello world", "\n".join(self.app.lines()))
        self.app.command("/quit")
        self.assertFalse(self.app.running)

    def test_click_style_navigation_and_back(self):
        self.assertEqual(self.app.view, "projects")
        self.app.open_selected()
        self.assertEqual(self.app.view, "chats")
        self.app.open_selected()
        self.assertEqual(self.app.view, "history")
        self.assertEqual(self.app.thread_id, "task-123")
        self.app.back()
        self.assertEqual(self.app.view, "chats")
        self.assertIsNone(self.app.thread_id)
        self.app.back()
        self.assertEqual(self.app.view, "projects")
        with self.assertRaisesRegex(ValueError, "Open a chat first"):
            self.app.command("a prompt")

    def test_full_file_tree_folding_and_back_to_active_chat(self):
        nested = self.app.project / "folder" / "one" / "two" / "three" / "four"
        nested.mkdir(parents=True)
        deep_file = nested / "deep.txt"
        deep_file.write_text("deep content\n")
        for index in range(300):
            (self.path / f"entry-{index:03}.txt").write_text("x")

        self.app.command("/chat task-123")
        self.app.command("/files")
        self.assertGreater(len(self.app.file_visible_entries), 300)
        self.assertIn(deep_file, [row[0] for row in self.app.file_visible_entries])
        self.assertNotIn("... more files", "\n".join(self.app.lines()))

        folder = self.app.project / "folder"
        self.app.menu_selection["files"] = next(
            i for i, row in enumerate(self.app.file_visible_entries) if row[0] == folder)
        self.app.toggle_selected_directory(collapse=True)
        self.assertNotIn(deep_file, [row[0] for row in self.app.file_visible_entries])
        self.app.toggle_selected_directory(collapse=False)
        self.assertIn(deep_file, [row[0] for row in self.app.file_visible_entries])

        self.app.menu_selection["files"] = next(
            i for i, row in enumerate(self.app.file_visible_entries) if row[0] == deep_file)
        self.app.open_selected()
        self.assertEqual(self.app.view, "file")
        self.assertIn("deep content", "\n".join(self.app.lines()))
        self.app.back()
        self.assertEqual(self.app.view, "files")
        self.assertEqual(self.app.thread_id, "task-123")
        self.app.back()
        self.assertEqual(self.app.view, "history")
        self.assertEqual(self.app.thread_id, "task-123")
        self.app.back()
        self.assertEqual(self.app.view, "chats")

        self.app.command("/chat task-123")
        self.app.command("/file note.txt")
        self.app.back()
        self.assertEqual(self.app.view, "history")

    def test_bridge_folders_toggle_independently_and_survive_drawer_refresh(self):
        for name in ("alpha", "beta"):
            folder = self.path / name
            folder.mkdir()
            (folder / "inside.txt").write_text(name)
        bridge_apply(self.app, {"action": "files"})
        alpha, beta = (self.path / "alpha").resolve(), (self.path / "beta").resolve()
        bridge_apply(self.app, {"action": "folder", "path": str(alpha)})
        bridge_apply(self.app, {"action": "folder", "path": str(beta)})
        self.assertEqual(self.app.file_collapsed, {alpha, beta})
        bridge_apply(self.app, {"action": "files"})
        self.assertEqual(self.app.file_collapsed, {alpha, beta})
        bridge_apply(self.app, {"action": "folder", "path": str(alpha)})
        self.assertEqual(self.app.file_collapsed, {beta})
        self.assertIn(alpha / "inside.txt", [row[0] for row in self.app.file_visible_entries])
        self.assertNotIn(beta / "inside.txt", [row[0] for row in self.app.file_visible_entries])

    def test_prior_prompts_survive_partial_resume_and_have_own_view(self):
        self.rpc.resume_partial = True
        self.app.command("/chat task-123")
        self.app.command("/prompts")
        shown = "\n".join(self.app.lines())
        self.assertIn("1. Hello", shown)
        self.assertIn("2. Later", shown)
        self.assertNotIn("you", shown.lower())
        self.app.command("/history")
        self.assertIn("Hello", "\n".join(self.app.lines()))

    def test_duplicate_task_rows_show_only_latest_record(self):
        older = self.path / "older-rollout.jsonl"
        newer = self.path / "newer-rollout.jsonl"
        older.write_bytes(b"x" * 1000)
        newer.write_bytes(b"x")
        self.rpc.rows = [
            {"id": "task-123", "cwd": str(self.path), "name": "Old title",
             "updatedAt": 10, "path": str(older)},
            {"id": "task-123", "cwd": str(self.path), "name": "Current title",
             "updatedAt": 20, "path": str(newer)},
        ]
        self.app.refresh_threads()
        self.assertEqual(len(self.app.threads), 1)
        self.assertEqual(self.app.threads[0]["name"], "Current title")
        self.app.command("/chat task-123")
        self.assertEqual(self.app.thread_id, "task-123")

    def test_history_uses_prompt_bands_without_speaker_labels(self):
        self.app.command("/chat task-123")
        user = next(item for item in self.app.turns[0]["items"] if item["type"] == "userMessage")
        source = "# Files mentioned by the user:\n\n## My request:\n**Do it**"
        user["content"][0]["text"] = source
        lines = self.app.lines()
        prompt = next(line for line in lines if line == "Files mentioned by the user:")
        self.assertEqual(prompt.style, "prompt")
        self.assertIn("My request:", lines)
        self.assertIn("Do it", lines)
        self.assertNotIn("# Files mentioned by the user:", lines)
        self.assertNotIn("you", lines)
        self.assertFalse(any(str(line).startswith("kai / ") for line in lines))
        self.assertEqual(user["content"][0]["text"], source)

    def test_voice_handoff_renders_ordered_speakers_as_markdown(self):
        source = ("<source>transcript_tail_flush</source>\n"
                  "<input>The realtime session ended.</input>\n"
                  "<transcript_delta>assistant: **Ready** to help.\n"
                  "user: Can you show me\n# the files?\n"
                  "assistant: Here are the files:\n- one\n- two"
                  "</transcript_delta>\n</realtime_delegation>")
        self.app.command("/chat task-123")
        user = next(item for item in self.app.turns[0]["items"] if item["type"] == "userMessage")
        user["content"][0]["text"] = source
        lines = self.app.lines()
        shown = "\n".join(lines)
        self.assertIn("Voice transcript", shown)
        self.assertIn("ChatGPT (voice)\nReady to help.", shown)
        self.assertIn("You (voice)\nCan you show me\nthe files?", shown)
        self.assertIn("Here are the files:\n• one\n• two", shown)
        self.assertNotIn("transcript_tail_flush", shown)
        self.assertNotIn("The realtime session ended.", shown)
        self.assertEqual(user["content"][0]["text"], source)
        self.assertEqual(next(line for line in lines if line == "Can you show me").style, "prompt")

        self.app.command("/prompts")
        self.assertIn("ChatGPT (voice)", "\n".join(self.app.lines()))

    def test_voice_handoff_requires_complete_known_envelope(self):
        ordinary = "Please show `<transcript_delta>` in my file."
        incomplete = ("<source>transcript_tail_flush</source><input>done</input>"
                      "<transcript_delta>user: hello")
        self.assertIsNone(voice_transcript_lines(ordinary))
        self.assertIsNone(voice_transcript_lines(incomplete))
        self.assertIsNone(voice_transcript_lines(
            "<source>other</source><input>done</input>"
            "<transcript_delta>user: hello</transcript_delta></realtime_delegation>"))
        self.assertIn("transcript_tail_flush", "\n".join(item_lines(
            {"type": "userMessage", "content": [{"type": "text", "text": incomplete}]})))

        wrapped = ("<realtime_delegation><source>transcript_tail_flush</source>"
                   "<input>done</input><transcript_delta>user: **hello**"
                   "</transcript_delta></realtime_delegation>")
        self.assertIn("hello", voice_transcript_lines(wrapped))

    def test_new_prompt_attachment_and_interrupt(self):
        self.app.command("/new")
        self.assertEqual(self.app.thread_id, "new-task")
        self.app.command("/attach pixel.png")
        self.assertTrue(image_file(self.path / "pixel.png"))
        self.app.command("Describe the image")
        method, params = self.rpc.calls[-1]
        self.assertEqual(method, "turn/start")
        self.assertEqual([part["type"] for part in params["input"]], ["text", "localImage"])
        self.app.command("/interrupt")
        self.assertEqual(self.rpc.calls[-1][0], "turn/interrupt")

    def test_dragged_images_become_short_labels_with_exact_paths(self):
        raw = f"Compare these {self.path}/pixel.png '{self.path}/photo space.png'"
        visible, paths = consume_dropped_images(raw, self.path)
        labels = [f"[Image {index}]" for index in range(1, len(paths) + 1)]
        shown = " ".join((visible, *labels))
        self.assertEqual(shown, "Compare these [Image 1] [Image 2]")
        self.assertEqual(paths, [(self.path / "pixel.png").resolve(),
                                 (self.path / "photo space.png").resolve()])
        self.assertNotIn(str(self.path), shown)

        escaped = str(self.path / "photo space.png").replace(" ", "\\ ")
        visible, paths = consume_dropped_images(escaped, self.path)
        self.assertEqual(visible, "")
        self.assertEqual(paths, [(self.path / "photo space.png").resolve()])

        # Warp/Finder drops can contain literal spaces with no quotes or escaping.
        visible, paths = consume_dropped_images(
            f"What is this {self.path / 'photo space.png'}", self.path)
        self.assertEqual(visible, "What is this")
        self.assertEqual(paths, [(self.path / "photo space.png").resolve()])

        stored = {"content": [
            {"type": "text", "text": "Compare [Image 1]"},
            {"type": "localImage", "path": str(paths[0])},
        ]}
        self.assertEqual(user_message_text(stored), "Compare [Image 1]")
        self.assertEqual(user_message_text({"content": [{"type": "text", "text": None},
                                                        {"type": "localImage", "path": str(paths[0])}]}),
                         "\n[Image 1]")

        untouched, paths = consume_dropped_images("mention note.txt", self.path)
        self.assertEqual((untouched, paths), ("mention note.txt", []))

    def test_stream_and_approvals(self):
        self.app.command("/chat task-123")
        self.rpc.events.put({"method": "turn/started", "params": {
            "threadId": "task-123", "turn": {"id": "t2", "items": [], "status": "inProgress"}}})
        self.rpc.events.put({"method": "item/started", "params": {
            "threadId": "task-123", "turnId": "t2", "item": {"id": "a2", "type": "agentMessage", "text": ""}}})
        self.rpc.events.put({"method": "item/agentMessage/delta", "params": {
            "threadId": "task-123", "turnId": "t2", "itemId": "a2", "delta": "# Streamed\n\n**ready**"}})
        self.rpc.events.put({"id": 31, "method": "item/commandExecution/requestApproval", "params": {
            "threadId": "task-123", "turnId": "t2", "command": "pwd"}})
        self.app.drain()
        shown = "\n".join(self.app.lines())
        self.assertIn("Streamed\n\nready", shown)
        self.assertNotIn("# Streamed", shown)
        self.assertEqual(self.app._item("a2")["text"], "# Streamed\n\n**ready**")
        self.app.command("/approve")
        self.assertEqual(self.rpc.answers[-1], (31, {"decision": "accept"}))
        self.rpc.events.put({"id": 32, "method": "item/fileChange/requestApproval", "params": {
            "threadId": "task-123", "turnId": "t2"}})
        self.app.drain()
        self.app.command("/reject")
        self.assertEqual(self.rpc.answers[-1], (32, {"decision": "decline"}))
        self.rpc.events.put({"method": "turn/completed", "params": {
            "threadId": "task-123", "turn": {"id": "t2", "status": "completed", "items": []}}})
        self.app.drain()
        self.assertIsNone(self.app.active_turn)

    def test_nullable_stream_fields_and_interleaved_multiline_events(self):
        self.app.command("/chat task-123")
        events = [
            ("turn/started", {"turn": {"id": "t2", "items": None, "status": "inProgress"}}),
            ("item/started", {"turnId": "t2", "item": {"id": "a2", "type": "agentMessage", "text": None}}),
            ("item/agentMessage/delta", {"turnId": "t2", "itemId": "a2", "delta": "First line\n"}),
            ("item/agentMessage/delta", {"turnId": "t2", "itemId": "a2", "delta": None}),
            ("item/agentMessage/delta", {"turnId": "t2", "itemId": "a2", "delta": "Second line"}),
            ("item/started", {"turnId": "t2", "item": {"id": "c2", "type": "commandExecution", "command": "pwd", "aggregatedOutput": None}}),
            ("item/commandExecution/outputDelta", {"turnId": "t2", "itemId": "c2", "delta": "one\ntwo\n"}),
            ("item/started", {"turnId": "t2", "item": {"id": "a2", "type": "agentMessage", "text": None}}),
            ("item/agentMessage/delta", {"turnId": "t2", "itemId": "a2", "delta": "\nThird line"}),
            ("item/completed", {"turnId": "t2", "item": {"id": "a2", "type": "agentMessage", "text": None, "status": "completed"}}),
            ("item/completed", {"turnId": "t2", "item": {"id": "c2", "type": "commandExecution", "aggregatedOutput": None, "status": "completed"}}),
            ("item/started", {"turnId": "t2", "item": {"id": "a3", "type": "agentMessage", "text": None}}),
            ("item/agentMessage/delta", {"turnId": "t2", "itemId": "a3", "delta": "Final reply\nNext paragraph"}),
            ("turn/completed", {"turn": {"id": "t2", "status": "completed", "items": []}}),
        ]
        for method, params in events:
            self.rpc.events.put({"method": method, "params": {"threadId": "task-123", **params}})
        self.app.drain()
        self.assertEqual(self.app._item("a2")["text"], "First line\nSecond line\nThird line")
        self.assertEqual(self.app._item("c2")["aggregatedOutput"], "one\ntwo\n")
        self.assertEqual(self.app._item("a3")["text"], "Final reply\nNext paragraph")
        self.assertIsNone(self.app.active_turn)
        self.app.command("/history")
        shown = "\n".join(self.app.lines())
        self.assertIn("Third line", shown)
        self.assertIn("Final reply\nNext paragraph", shown)
        self.app.command("/terminal")
        self.assertIn("one\ntwo", "\n".join(self.app.lines()))

        self.app.command("Follow up")
        self.rpc.events.put({"method": "item/started", "params": {
            "threadId": "task-123", "turnId": "new-turn",
            "item": {"id": "a4", "type": "agentMessage", "text": None}}})
        self.rpc.events.put({"method": "item/agentMessage/delta", "params": {
            "threadId": "task-123", "turnId": "new-turn", "itemId": "a4",
            "delta": "Follow-up reply\nwith another line"}})
        self.rpc.events.put({"method": "item/completed", "params": {
            "threadId": "task-123", "turnId": "new-turn",
            "item": {"id": "a4", "type": "agentMessage", "text": "Follow-up reply\nwith another line", "status": "completed"}}})
        self.rpc.events.put({"method": "turn/completed", "params": {
            "threadId": "task-123", "turn": {"id": "new-turn", "status": "completed", "items": []}}})
        self.app.drain()
        self.app.command("/history")
        shown = "\n".join(self.app.lines())
        self.assertIn("First line\nSecond line\nThird line", shown)
        self.assertIn("Follow-up reply\nwith another line", shown)
        self.assertIsNone(self.app.active_turn)

    def test_desktop_owned_task_remains_readable(self):
        self.rpc.writer_busy = True
        self.app.command("/chat task-123")
        self.assertTrue(self.app.read_only)
        self.app.command("/status")
        self.assertIn("read-only", "\n".join(self.app.lines()))
        self.rpc.writer_busy = False
        self.app.command("Hello again")
        self.assertFalse(self.app.read_only)
        self.assertEqual(self.rpc.calls[-1][0], "turn/start")

    def test_codex_markdown_is_rendered_without_changing_source(self):
        source = ("# Result\n\n**Ready** with [details](https://example.com).\n"
                  "- first\n- [x] shipped\n\n> note\n\n```python\n# code stays code\n```")
        rendered = markdown_lines(source)
        self.assertEqual(rendered[0], "Result")
        self.assertEqual(rendered[0].style, "heading")
        self.assertIn("Ready with details (https://example.com).", rendered)
        self.assertIn("• first", rendered)
        self.assertIn("☑ shipped", rendered)
        self.assertIn("│ note", rendered)
        self.assertIn("# code stays code", rendered)

        self.app.command("/chat task-123")
        agent = next(item for item in self.app.turns[0]["items"] if item["type"] == "agentMessage")
        agent["text"] = source
        self.assertNotIn("# Result", "\n".join(self.app.lines()))
        self.assertEqual(agent["text"], source)

    def test_markdown_tables_keep_headers_and_values_in_narrow_view(self):
        source = ("Kai could show | App Server data\n"
                  "--- | ---\n"
                  "Viewing an image | imageView item with its path\n"
                  "Running a command | commandExecution with status and output\n")
        rendered = markdown_lines(source)
        self.assertEqual(rendered[:4], ["Kai could show  ·  App Server data",
                                        "Viewing an image",
                                        "App Server data: imageView item with its path", ""])
        self.assertIn("App Server data: commandExecution with status and output", rendered)
        self.assertNotIn("--- | ---", rendered)
        self.assertEqual(source.splitlines()[0], "Kai could show | App Server data")

        with_pipes = "| Name | Value |\n| :--- | ---: |\n| a \\| b | **two** |"
        shown = markdown_lines(with_pipes)
        self.assertIn("a | b", shown)
        self.assertIn("Value: two", shown)

    def test_long_and_unusual_text_is_not_cut_or_sent_to_curses_with_newlines(self):
        long_reply = "界🙂" * 16000 + "\n\nEnd of reply"
        agent = {"type": "agentMessage", "text": long_reply}
        rendered = item_lines(agent)
        self.assertNotIn("[truncated]", "\n".join(rendered))
        self.assertIn("End of reply", rendered)

        output = "first\n\n\x1b[31mred\x1b[0m\tcolumn\rnext\n" + "x" * 13000
        terminal = item_lines({"type": "commandExecution", "command": "print",
                               "status": "completed", "aggregatedOutput": output}, "terminal")
        wrapped = wrap_display_lines(terminal, 80)
        self.assertTrue(all("\n" not in line and "\r" not in line for line in wrapped))
        self.assertTrue(all(getattr(line, "style", "") == "code" for line in wrapped[1:-1]))
        shown = "\n".join(wrapped)
        self.assertIn("red column", shown)
        self.assertIn("next", shown)
        self.assertEqual(sum(len(line) for line in wrapped if line and set(line) == {"x"}), 13000)
        self.assertNotIn("\x1b", shown)

        styled = wrap_display_lines([DisplayLine("one\ntwo", "prompt")], 20)
        self.assertEqual(styled, ["one", "two"])
        self.assertTrue(all(line.style == "prompt" for line in styled))

        unknown = item_lines({"type": "futureTextItem", "status": "completed",
                              "text": "alpha\nbeta\n" + "z" * 1500}, "activity")
        self.assertIn("alpha\nbeta", "\n".join(unknown))
        self.assertNotIn("[truncated]", "\n".join(unknown))
        self.assertTrue(all("\n" not in line for line in wrap_display_lines(unknown, 80)))

    def test_stream_burst_keeps_order_and_yields_between_batches(self):
        self.app.command("/chat task-123")
        self.rpc.events.put({"method": "turn/started", "params": {
            "threadId": "task-123", "turn": {"id": "burst", "status": "inProgress", "items": []}}})
        self.rpc.events.put({"method": "item/started", "params": {
            "threadId": "task-123", "turnId": "burst",
            "item": {"id": "burst-text", "type": "agentMessage", "text": None}}})
        for index in range(1500):
            self.rpc.events.put({"method": "item/agentMessage/delta", "params": {
                "threadId": "task-123", "turnId": "burst", "itemId": "burst-text",
                "delta": f"{index},"}})
        self.app.drain()
        self.assertFalse(self.rpc.events.empty())
        self.app.drain()
        self.assertTrue(self.rpc.events.empty())
        expected = "".join(f"{index}," for index in range(1500))
        self.assertEqual(self.app._item("burst-text")["text"], expected)

    def test_cli_uses_same_commands_without_curses(self):
        state, result = execute(self.app, "/projects 1")
        self.assertEqual(result, "not_waited")
        self.assertEqual(state["view"], "chats")
        state, _ = execute(self.app, "/chat 1")
        self.assertEqual(state["view"], "history")
        self.assertEqual(state["threadId"], "task-123")
        self.assertIn("Hello", "\n".join(state["lines"]))
        self.assertEqual(snapshot(self.app)["lines"], [str(line) for line in self.app.lines()])

    def test_cli_wait_stops_for_approval(self):
        self.app.command("/chat task-123")
        self.app.active_turn = "turn-1"
        self.rpc.events.put({"id": 88, "method": "item/commandExecution/requestApproval", "params": {
            "threadId": "task-123", "turnId": "turn-1", "command": "pwd"}})
        self.assertEqual(wait_for_turn(self.app, 0.2), "approval_needed")
        state = snapshot(self.app)
        self.assertEqual(state["pendingApproval"]["id"], 88)

    def test_frontends_label_new_tasks_independently(self):
        cli = Kai(self.rpc, self.path, service_name="kai_cli")
        cli.command("/new")
        self.assertEqual(self.rpc.calls[-2][0], "thread/start")
        self.assertEqual(self.rpc.calls[-2][1]["serviceName"], "kai_cli")

    def test_notch_new_chat_is_outside_selected_project(self):
        standalone = (self.path / "standalone-chats").resolve()
        self.app.standalone_root = standalone
        bridge_apply(self.app, {"action": "new"})
        self.assertEqual(self.rpc.calls[-2], ("thread/start", {"cwd": str(standalone), "serviceName": "kai_tui"}))
        self.assertTrue(standalone.is_dir())
        self.assertNotIn(standalone, self.app.project_paths)
        state = bridge_snapshot(self.app)
        self.assertTrue(state["standalone"])
        self.assertEqual([chat["id"] for chat in state["standaloneChats"]], ["new-task"])
        self.assertEqual(state["chats"], [])
        bridge_apply(self.app, {"action": "project", "path": str(self.path)})
        bridge_apply(self.app, {"action": "new_project"})
        self.assertEqual(self.rpc.calls[-2][1]["cwd"], str(self.path.resolve()))

    def test_notch_bridge_uses_live_kai_state_and_actions(self):
        initial = bridge_snapshot(self.app)
        self.assertEqual(initial["project"], str(self.path.resolve()))
        self.assertEqual(initial["chats"][0]["id"], "task-123")
        monitored = bridge_snapshot(self.app, active_tasks=[{"id": "task-123", "name": "Existing task",
                                                            "scene": "terminal", "text": "Running command"}])
        self.assertEqual(monitored["chats"][0]["status"], "active")
        self.assertEqual(monitored["focusThreadId"], "task-123")
        self.assertEqual(monitored["activity"]["scene"], "terminal")
        bridge_apply(self.app, {"action": "chat", "id": "task-123"})
        bridge_apply(self.app, {"action": "files"})
        state = bridge_snapshot(self.app)
        self.assertEqual(state["threadId"], "task-123")
        self.assertEqual([part["kind"] for part in state["history"]], ["user", "assistant"])
        self.assertIn(str((self.path / "note.txt").resolve()), [row["path"] for row in state["files"]])

        bridge_apply(self.app, {"action": "attachments", "paths": [str(self.path / "pixel.png")]})
        bridge_apply(self.app, {"action": "send", "text": "Describe this"})
        method, params = self.rpc.calls[-1]
        self.assertEqual(method, "turn/start")
        self.assertEqual([part["type"] for part in params["input"]], ["text", "localImage"])
        self.assertEqual(bridge_snapshot(self.app, include_files=False).get("files"), None)

    def test_notch_activity_comes_from_structured_items(self):
        self.app.active_turn = "turn-1"
        self.app.turns = [{"id": "turn-1", "items": [
            {"id": "command-1", "type": "commandExecution", "status": "inProgress",
             "command": "git status --short"}]}]
        self.assertEqual(bridge_activity(self.app),
                         {"scene": "terminal", "text": "git status --short"})
        self.app.approvals.append({"id": 7, "method": "item/commandExecution/requestApproval",
                                   "params": {"threadId": self.app.thread_id, "command": "git status --short"}})
        self.assertEqual(bridge_activity(self.app)["scene"], "permission")

    def test_background_approval_controls_target_requesting_chat(self):
        self.app.approvals.append({"id": 8, "method": "item/commandExecution/requestApproval",
                                   "params": {"threadId": "background", "command": "rm temp"}})
        state = bridge_snapshot(self.app, include_files=False, active_tasks=[
            {"id": "background", "name": "Background", "scene": "terminal", "text": "rm temp"}
        ])
        self.assertTrue(state["approval"])
        self.assertEqual(state["approvalThreadId"], "background")
        self.assertEqual(state["activity"]["scene"], "permission")
        bridge_apply(self.app, {"action": "approve", "threadId": "background"})
        self.assertEqual(self.rpc.answers[-1], (8, {"decision": "accept"}))


if __name__ == "__main__":
    unittest.main()

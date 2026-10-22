import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from kai_activity import SessionActivityMonitor
from kai_bridge import apply
from kai import Kai
from test_kai import FakeServer


class ReliabilityTests(unittest.TestCase):
    def test_split_completion_event_is_not_lost(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.jsonl"
            started = json.dumps({"payload": {"type": "task_started", "turn_id": "a"}}).encode() + b"\n"
            ended = json.dumps({"payload": {"type": "task_complete", "turn_id": "a"}}).encode() + b"\n"
            rows = [{"id": "chat", "path": str(path)}]
            path.write_bytes(started)
            monitor = SessionActivityMonitor()
            self.assertEqual(len(monitor.poll(rows)["activeTasks"]), 1)
            with path.open("ab") as stream:
                stream.write(ended[:20])
            self.assertEqual(len(monitor.poll(rows)["activeTasks"]), 1)
            self.assertEqual(monitor.offsets["chat"], len(started))
            with path.open("ab") as stream:
                stream.write(ended[20:])
            self.assertEqual(monitor.poll(rows)["activeTasks"], [])

    def test_initial_partial_line_and_file_replacement(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "events.jsonl"
            record = json.dumps({"payload": {"type": "task_started", "turn_id": "a"}}).encode() + b"\n"
            rows = [{"id": "chat", "path": str(path)}]
            path.write_bytes(record[:20])
            monitor = SessionActivityMonitor()
            self.assertEqual(monitor.poll(rows)["activeTasks"], [])
            self.assertEqual(monitor.offsets["chat"], 0)
            path.write_bytes(record)
            self.assertEqual(len(monitor.poll(rows)["activeTasks"]), 1)
            replacement = Path(directory) / "replacement"
            replacement.write_bytes(record.replace(b"task_started", b"task_aborted"))
            replacement.replace(path)
            self.assertEqual(monitor.poll(rows)["activeTasks"], [])

    def test_project_navigation_does_not_refetch_entire_task_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            app = Kai(FakeServer(directory), Path(directory))
            with patch.object(app, "refresh_threads", side_effect=AssertionError("Unexpected paging")):
                apply(app, {"action": "project", "path": directory})
            self.assertEqual(app.project, Path(directory).resolve())


if __name__ == "__main__":
    unittest.main()

class HistoryPaginationTests(unittest.TestCase):
    def make_app(self, directory, count):
        app = Kai(FakeServer(directory), Path(directory))
        app.thread_id = "chat"
        app.turns = [{"id": str(i), "items": [
            {"type": "userMessage", "id": "u" + str(i), "content": [{"type": "text", "text": "Prompt " + str(i)}]},
            {"type": "agentMessage", "id": "a" + str(i), "text": "Response " + str(i)},
        ]} for i in range(count)]
        return app

    def test_only_last_two_exchanges_are_formatted_from_large_history(self):
        from kai_bridge import snapshot, item_lines
        with tempfile.TemporaryDirectory() as directory:
            app = self.make_app(directory, 10000)
            with patch("kai_bridge.item_lines", wraps=item_lines) as formatter:
                state = snapshot(app)
            self.assertEqual([m["id"] for m in state["history"]], ["u9998", "a9998", "u9999", "a9999"])
            self.assertEqual(formatter.call_count, 4)
            self.assertTrue(state["historyHasMore"])

    def test_pages_include_whole_exchanges_and_stop_at_start(self):
        from kai_bridge import snapshot
        with tempfile.TemporaryDirectory() as directory:
            app = self.make_app(directory, 5)
            app.turns[-1]["items"].insert(1, {"type": "agentMessage", "id": "commentary", "text": "Working"})
            first = snapshot(app)
            self.assertEqual(first["history"][0]["id"], "u3")
            self.assertIn("commentary", [m["id"] for m in first["history"]])
            apply(app, {"action": "history_more", "threadId": "chat"})
            second = snapshot(app)
            self.assertEqual(second["history"][0]["id"], "u1")
            self.assertTrue(second["historyHasMore"])
            apply(app, {"action": "history_more", "threadId": "chat"})
            final = snapshot(app)
            self.assertEqual(final["history"][0]["id"], "u0")
            self.assertFalse(final["historyHasMore"])
            with self.assertRaises(ValueError):
                apply(app, {"action": "history_more", "threadId": "other"})

class ToolActivityHistoryTests(unittest.TestCase):
    make_app = HistoryPaginationTests.make_app
    def test_tools_stay_in_chronological_order_inside_two_exchange_window(self):
        from kai_bridge import snapshot
        with tempfile.TemporaryDirectory() as directory:
            app = self.make_app(directory, 4)
            app.turns[1]["items"].append({"id": "oldtool", "type": "imageView"})
            app.turns[-1]["items"][1:1] = [
                {"id": "command", "type": "commandExecution", "status": "inProgress", "command": "private arguments", "aggregatedOutput": "x" * 1000000},
                {"id": "image", "type": "imageView", "status": "completed"},
                {"id": "reasoning", "type": "reasoning", "text": "not an activity row"},
                {"id": "edit", "type": "fileChange", "status": "completed"},
            ]
            state = snapshot(app)
            self.assertEqual([m["id"] for m in state["history"]], ["u2", "a2", "u3", "command", "a3"])
            self.assertTrue(state["historyHasMore"])
            rows = [m for m in state["history"] if m["kind"] == "activity"]
            self.assertEqual([m["lines"][0]["text"] for m in rows], ["Running a command…, viewed an image, edited files"])
            self.assertTrue(all(m["lines"][0]["mathEnabled"] is False and m["speechText"] is None for m in rows))
            app.turns[-1]["items"][1].update(status="completed", exitCode=0)
            completed = snapshot(app)
            self.assertEqual(next(m for m in completed["history"] if m["id"] == "command")["lines"][0]["text"], "Ran a command, viewed an image, edited files")
            self.assertEqual(rows[0]["activityIcon"], "terminal")
            self.assertLess(len(json.dumps(completed)), 10000)

    def test_failed_tools_and_mapped_dynamic_events_have_accurate_labels(self):
        from kai_bridge import tool_activity_line
        self.assertEqual(tool_activity_line({"type": "commandExecution", "status": "completed", "exitCode": 1}), "Ran a command · failed")
        self.assertEqual(tool_activity_line({"type": "dynamicToolCall", "tool": "tools.view_image", "status": "completed"}), "Viewed an image")
        self.assertEqual(tool_activity_line({"type": "webSearch", "status": "inProgress"}), "Searching the web…")
        self.assertEqual(tool_activity_line({"type": "mcpToolCall", "tool": "read_page", "status": "failed"}), "Used tool: read page · failed")
        self.assertIsNone(tool_activity_line({"type": "reasoning"}))

    def test_repeated_and_mixed_tools_fold_only_between_messages(self):
        from kai_bridge import snapshot
        with tempfile.TemporaryDirectory() as directory:
            app = self.make_app(directory, 2)
            app.turns[-1]["items"][1:1] = [
                {"id": "c1", "type": "commandExecution", "status": "completed"},
                {"id": "c2", "type": "commandExecution", "status": "completed"},
                {"id": "i1", "type": "imageView"},
                {"id": "i2", "type": "imageView"},
                {"id": "comment", "type": "agentMessage", "text": "Next paragraph"},
                {"id": "c3", "type": "commandExecution", "status": "completed"},
            ]
            rows = [m for m in snapshot(app)["history"] if m["kind"] == "activity"]
            self.assertEqual([m["id"] for m in rows], ["c1", "c3"])
            self.assertEqual([m["lines"][0]["text"] for m in rows], ["Ran commands, viewed images", "Ran a command"])

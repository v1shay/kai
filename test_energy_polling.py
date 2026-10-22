from pathlib import Path
import os
import tempfile
import unittest
from unittest.mock import patch

from kai import Kai, RPCError, list_thread_rows
from kai_bridge import snapshot, thread_refresh_interval
from test_kai import FakeServer


class EnergyPollingTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name) / "rollout.jsonl"
        self.path.write_text("initial\n")
        self.rpc = FakeServer(self.directory.name)
        self.rpc.writer_busy = True
        self.rpc.rows[0]["path"] = str(self.path)
        self.clock = patch("kai.time.monotonic", return_value=1000).start()
        self.addCleanup(patch.stopall)
        self.app = Kai(self.rpc, Path(self.directory.name))
        self.app.select_thread("task-123")
        self.app.dirty = False
        self.rpc.calls.clear()

    def tick(self, seconds=2):
        self.clock.return_value += seconds
        self.app.drain()

    def reads(self):
        return [call for call in self.rpc.calls if call[0] == "thread/read"]

    def test_unchanged_desktop_owned_history_is_not_loaded_repeatedly(self):
        for _ in range(100):
            self.tick()
        self.assertEqual(self.reads(), [])
        self.assertFalse(self.app.dirty)

    def test_appended_history_is_refreshed_once_then_quiet(self):
        with self.path.open("a") as stream:
            stream.write("new event\n")
        self.tick()
        self.assertEqual(len(self.reads()), 1)
        for _ in range(10):
            self.tick()
        self.assertEqual(len(self.reads()), 1)

    def test_same_size_rewrite_and_file_replacement_are_detected(self):
        revision = self.path.stat()
        self.path.write_text("changed\n")
        os.utime(self.path, ns=(revision.st_atime_ns, revision.st_mtime_ns + 1_000_000))
        self.tick()
        replacement = self.path.with_suffix(".replacement")
        replacement.write_text("changed\n")
        replacement.replace(self.path)
        self.tick()
        self.assertEqual(len(self.reads()), 2)

    def test_write_during_read_is_not_mistaken_for_already_loaded_history(self):
        original = self.rpc.call

        def call(method, params=None, timeout=25):
            if method == "thread/read":
                with self.path.open("a") as stream:
                    stream.write("concurrent event\n")
            return original(method, params, timeout)

        self.path.write_text("changed\n")
        with patch.object(self.rpc, "call", side_effect=call):
            self.tick()
        self.tick()
        self.assertEqual(len(self.reads()), 2)

    def test_missing_rollout_backs_off_without_losing_external_events(self):
        self.path.unlink()
        self.app.thread.pop("path", None)
        self.rpc.rows[0].pop("path", None)
        for _ in range(60):
            self.tick()
        self.assertLessEqual(len(self.reads()), 7)
        self.rpc.events.put({"method":"thread/status/changed","params":{"threadId":"task-123","status":{"type":"active"}}})
        self.app.drain()
        self.assertEqual(self.app.thread["status"]["type"], "active")
        self.assertTrue(self.app.dirty)

    def test_failed_read_retries_the_same_revision_with_backoff(self):
        self.path.write_text("changed\n")
        with patch.object(self.rpc, "call", side_effect=RPCError("unavailable")) as call:
            for _ in range(30):
                self.tick()
            self.assertLessEqual(call.call_count, 5)
        self.tick(30)
        self.assertEqual(len(self.reads()), 1)

    def test_unchanged_catalog_does_not_resolve_every_project_again(self):
        with patch.object(Path, "resolve", side_effect=AssertionError("Unnecessary project resolution")):
            self.app.set_threads(self.rpc.rows)
        self.assertFalse(self.app.dirty)
        self.rpc.rows[0] = {**self.rpc.rows[0], "status":{"type":"active"}}
        self.app.set_threads(self.rpc.rows)
        self.assertTrue(self.app.dirty)

    def test_idle_catalog_refresh_is_slower_than_active_or_initial_discovery(self):
        self.assertEqual(thread_refresh_interval(self.app, []), 30)
        self.assertEqual(thread_refresh_interval(self.app, [{"id":"active"}]), 5)
        self.app.projects_loading = True
        self.assertEqual(thread_refresh_interval(self.app, []), 2)

    def test_catalog_pages_use_the_state_index_instead_of_scanning_rollouts(self):
        class Pages:
            def __init__(self):
                self.requests = []

            def call(self, method, params, timeout=8):
                self.requests.append((method, params))
                return {"data":[{"id":"second" if params.get("cursor") else "first"}],
                        "nextCursor":None if params.get("cursor") else "next"}

        server = Pages()
        rows, more = list_thread_rows(server)
        self.assertEqual([row["id"] for row in rows], ["first", "second"])
        self.assertFalse(more)
        self.assertEqual(len(server.requests), 2)
        for method, params in server.requests:
            self.assertEqual(method, "thread/list")
            self.assertTrue(params["useStateDbOnly"])
            self.assertEqual(params["sortKey"], "updated_at")

    def test_snapshot_reuses_resolved_project_directories(self):
        # Streaming updates must not perform one realpath/lstat chain per chat.
        with patch.object(Path, "resolve", side_effect=AssertionError("Unnecessary filesystem query")):
            state = snapshot(self.app)
        self.assertEqual(state["chats"][0]["id"], "task-123")


if __name__ == "__main__":
    unittest.main()

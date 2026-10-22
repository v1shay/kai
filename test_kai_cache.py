import unittest
from unittest.mock import patch
from kai import Kai, RecentThreadCache
from test_kai import FakeServer
from pathlib import Path
import tempfile

class RecentThreadCacheTests(unittest.TestCase):
    def thread(self, identity, revision=1):
        return {"id": identity, "updatedAt": revision, "status": {"type": "idle"}}

    def test_recent_history_is_independent_revision_checked_and_bounded(self):
        cache = RecentThreadCache(max_entries=2, max_bytes=500)
        turns = [{"id": "turn", "items": []}]
        cache.put("a", self.thread("a"), turns)
        cache.put("b", self.thread("b"), turns)
        hit = cache.get(self.thread("a"))
        hit["turns"].clear()
        self.assertEqual(len(cache.get(self.thread("a"))["turns"]), 1)
        cache.put("c", self.thread("c"), turns)
        self.assertIsNone(cache.get(self.thread("b")))
        self.assertLessEqual(cache.bytes, 500)
        self.assertIsNone(cache.get(self.thread("a", revision=2)))
        cache.put("huge", self.thread("huge"), [{"text": "x" * 501}])
        self.assertNotIn("huge", cache.entries)

    def test_active_or_expired_or_unknown_revision_never_reuses_history(self):
        cache = RecentThreadCache(ttl=10)
        with patch("kai.time.monotonic", return_value=100):
            cache.put("a", self.thread("a"), [])
        with patch("kai.time.monotonic", return_value=111):
            self.assertIsNone(cache.get(self.thread("a")))
        cache.put("a", self.thread("a"), [])
        self.assertIsNone(cache.get({**self.thread("a"), "status": {"type": "active"}}))
        cache.put("unknown", {"id": "unknown"}, [])
        self.assertEqual(cache.bytes, 0)

    def test_notch_startup_fetches_one_page_and_marks_remaining_projects_loading(self):
        class PaginatedServer(FakeServer):
            def call(self, method, params=None, timeout=25):
                if method == "thread/list":
                    self.calls.append((method, params))
                    return {"data": self.rows, "nextCursor": "more" if not params.get("cursor") else None}
                return super().call(method, params, timeout)
        with tempfile.TemporaryDirectory() as directory:
            server = PaginatedServer(directory)
            app = Kai(server, Path(directory), initial_thread_pages=1)
            self.assertTrue(app.projects_loading)
            self.assertEqual(len(server.calls), 1)
            self.assertTrue(app.project_paths)
            app.refresh_threads()
            self.assertFalse(app.projects_loading)
            self.assertEqual(len(app.threads), 1)

    def test_revisit_skips_history_read_but_still_resumes_and_refreshes(self):
        with tempfile.TemporaryDirectory() as directory:
            server = FakeServer(directory)
            server.rows[0]["updatedAt"] = 1
            app = Kai(server, Path(directory))
            app.select_thread("task-123")
            app.leave_chat()
            server.calls.clear()
            app.select_thread("task-123")
            self.assertNotIn("thread/read", [call[0] for call in server.calls])
            self.assertIn("thread/resume", [call[0] for call in server.calls])
            app.leave_chat()
            server.rows[0]["updatedAt"] = 2
            app.refresh_threads()
            server.calls.clear()
            app.select_thread("task-123")
            self.assertIn("thread/read", [call[0] for call in server.calls])
            self.assertIn("thread/resume", [call[0] for call in server.calls])

if __name__ == "__main__":
    unittest.main()

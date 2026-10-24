from datetime import datetime, timezone
import json
from pathlib import Path
import tempfile
import unittest

from kai_activity import SessionActivityMonitor
from kai_bridge import activity, snapshot
from kai import Kai
from test_kai import FakeServer
from test_kai_code import FixtureConnection


class ActivityLifecycleTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name) / "rollout.jsonl"
        self.now = 2_000_000_000.0
        self.monitor = SessionActivityMonitor(clock=lambda: self.now)
        self.rows = [{"id": "chat", "path": str(self.path)}]

    def append(self, payload, age=0, record_type="event_msg"):
        stamp = datetime.fromtimestamp(self.now - age, timezone.utc).isoformat()
        with self.path.open("a") as stream:
            stream.write(json.dumps({"timestamp": stamp, "type": record_type, "payload": payload}) + "\n")

    def test_orphaned_start_expires_and_settings_do_not_revive_it(self):
        self.append({"type": "task_started", "turn_id": "old"}, age=18 * 3600)
        self.append({"type": "thread_settings_applied"})
        self.assertEqual(self.monitor.poll(self.rows)["activeTasks"], [])
        self.append({"type": "custom_tool_call", "name": "exec_command"})
        self.assertEqual(self.monitor.poll(self.rows)["focusThreadId"], "chat")
        self.now += 601
        result = self.monitor.poll(self.rows)
        self.assertEqual(result["activeTasks"], [])
        self.assertTrue(result["changed"])
        self.assertFalse(self.monitor.poll(self.rows)["changed"])

    def test_known_live_turn_does_not_expire_during_long_tool(self):
        self.append({"type": "task_started", "turn_id": "a"}, age=3600)
        self.rows[0]["status"] = {"type": "active"}
        self.assertEqual(self.monitor.poll(self.rows)["focusThreadId"], "chat")
        self.rows[0]["status"] = {"type": "notLoaded"}
        self.assertEqual(self.monitor.poll(self.rows)["activeTasks"], [])

    def test_missing_page_still_tails_completion(self):
        self.append({"type": "task_started", "turn_id": "a"})
        self.monitor.poll(self.rows)
        self.append({"type": "task_complete", "turn_id": "a"})
        self.assertEqual(self.monitor.poll([])["activeTasks"], [])

    def test_terminal_variants_and_final_answer_clear_matching_turn(self):
        for payload, record_type in [
            ({"type": "task_complete"}, "event_msg"),
            ({"type": "task_completed", "turn_id": "a"}, "event_msg"),
            ({"type": "task_aborted", "turn_id": "a"}, "event_msg"),
            ({"type": "message", "role": "assistant", "phase": "final_answer"}, "response_item"),
        ]:
            with self.subTest(payload=payload):
                self.append({"type": "task_started", "turn_id": "a"})
                self.monitor.poll(self.rows)
                self.append(payload, record_type=record_type)
                self.assertEqual(self.monitor.poll(self.rows)["activeTasks"], [])
                self.assertEqual(SessionActivityMonitor(clock=lambda: self.now).poll(self.rows)["activeTasks"], [])

    def test_old_completion_does_not_stop_new_turn(self):
        self.append({"type": "task_started", "turn_id": "new"})
        self.monitor.poll(self.rows)
        self.append({"type": "task_complete", "turn_id": "old"})
        self.assertEqual(self.monitor.poll(self.rows)["focusThreadId"], "chat")


class SelectedChatActivityTests(unittest.TestCase):
    def test_completion_feed_is_explicit_and_commentary_uses_the_last_line(self):
        import json
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/"session.jsonl";monitor=SessionActivityMonitor(clock=lambda:1000)
            rows=[{"id":"chat","path":str(path),"status":{"type":"active"}}]
            def append(payload):
                with path.open("a") as stream:stream.write(json.dumps({"type":"event_msg","payload":payload})+"\n")
            append({"type":"task_started","turn_id":"run"})
            self.assertEqual(monitor.poll(rows)["activeTasks"][0]["runId"],"run")
            append({"type":"agent_message","message":"Explaining the change\nUpdating the animation now"})
            state=monitor.poll(rows)
            self.assertEqual(state["activeTasks"][0]["text"],"Updating the animation now")
            self.assertEqual(state["activeTasks"][0]["scene"],"pencil")
            self.assertEqual(state["finishedTasks"],[])
            append({"type":"task_complete","turn_id":"old"})
            self.assertEqual(monitor.poll(rows)["finishedTasks"],[])
            append({"type":"task_complete","turn_id":"run"})
            self.assertEqual(monitor.poll(rows)["finishedTasks"],[{"id":"chat","runId":"run"}])
            self.assertEqual(monitor.poll(rows)["finishedTasks"],[])

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.app = Kai(FakeServer(self.directory.name), Path(self.directory.name))
        self.app.thread_id = "finished"
        self.app.thread = {"name": "Finished", "status": {"type": "idle"}}

    def test_viewed_finished_chat_does_not_inherit_global_work(self):
        task = {"id": "other", "name": "Other", "scene": "terminal", "text": "Running tests"}
        result = snapshot(self.app, active_tasks=[task])
        self.assertEqual(result["activity"]["scene"], "terminal")
        self.assertEqual(result["sessionActivity"]["scene"], "off")

    def test_active_task_snapshot_keeps_run_id_for_floating_pet_completion(self):
        task = {"id": "other", "name": "Other", "scene": "terminal", "text": "Running tests", "runId": "turn-2"}
        self.assertEqual(snapshot(self.app, active_tasks=[task])["activeTasks"], [task])
        task.pop("runId")
        self.assertIsNone(snapshot(self.app, active_tasks=[task])["activeTasks"][0]["runId"])

    def test_new_turn_does_not_reuse_unfinished_tool_from_old_turn(self):
        self.app.turns = [
            {"id": "old", "status": "inProgress", "items": [{"id": "cmd", "type": "commandExecution", "status": "inProgress"}]},
            {"id": "new", "status": "inProgress", "items": []},
        ]
        self.app.active_turn = "new"
        self.assertEqual(activity(self.app)["scene"], "thinking")
        self.app.turns[-1]["status"] = "completed"
        self.assertIsNone(self.app._latest_active_turn())

    def test_completion_clears_old_active_thread_status(self):
        self.app.thread["status"] = {"type": "active"}
        self.app.active_turn = "a"
        self.app.turns = [{"id": "a", "status": "inProgress", "items": []}]
        self.app.rpc.events.put({"method": "turn/completed", "params": {
            "threadId": "finished", "turn": {"id": "a", "status": "completed"}}})
        self.app.drain()
        self.assertEqual(activity(self.app)["scene"], "off")

    def test_completed_turn_does_not_render_stale_tool_as_running(self):
        item = {"id": "cmd", "type": "commandExecution", "status": "inProgress"}
        self.app.turns = [{"id": "done", "status": "completed", "items": [item]}]
        result = snapshot(self.app)
        self.assertEqual(result["history"][0]["lines"][0]["text"], "Ran a command")
        self.assertEqual(item["status"], "inProgress")

    def test_finished_code_session_remains_authoritative_for_its_rollout(self):
        code = FixtureConnection()
        code.state["sessions"][0]["running"] = False
        self.assertEqual(code.tasks(), [])
        self.assertEqual(code.owned_thread_ids(), {"native1"})
        code.selected_id = "s1"
        state = code.overlay(snapshot(self.app, active_tasks=[{"id": "other", "name": "Other", "scene": "thinking", "text": "Working"}]))
        self.assertEqual(state["sessionActivity"]["scene"], "off")
        code.connected = False
        self.assertEqual(code.owned_thread_ids(), set())

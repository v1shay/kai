import tempfile
import unittest
from pathlib import Path
from kai import Kai
from kai_bridge import apply

class RPC:
    def __init__(self): self.calls = []
    def call(self, method, params, **kwargs):
        self.calls.append((method, params))
        return {"turn": {"id": "turn", "items": []}}

class ExperimentalTests(unittest.TestCase):
    def setUp(self):
        self.rpc = RPC()
        self.app = Kai(self.rpc, Path(tempfile.gettempdir()))
        self.app.thread_id = "chat"
        self.app.models = [{"model": "dynamic", "defaultReasoningEffort": "low",
                            "supportedReasoningEfforts": [{"reasoningEffort": "low"}, {"reasoningEffort": "high"}]}]
    def test_supported_effort_and_turn_overrides(self):
        apply(self.app, {"action": "settings", "model": "dynamic", "effort": "ultra"})
        self.assertEqual(self.app.reasoning_effort, "low")
        self.app.send_prompt("hello")
        params = self.rpc.calls[-1][1]
        self.assertEqual((params["model"], params["effort"]), ("dynamic", "low"))
    def test_unknown_model_rejected(self):
        with self.assertRaises(ValueError): apply(self.app, {"action": "settings", "model": "missing"})
    def test_context_is_chat_scoped_and_consumed_once(self):
        with self.assertRaises(ValueError): apply(self.app, {"action": "context", "threadId": "other", "text": "secret"})
        apply(self.app, {"action": "context", "threadId": "chat", "text": "Selected text"})
        self.app.send_prompt("explain")
        self.assertIn("Selected text", self.rpc.calls[-1][1]["input"][0]["text"])
        self.assertEqual(self.app.context_text, "")
    def test_context_images_survive_manual_attachment_updates(self):
        with tempfile.NamedTemporaryFile(suffix=".png") as image:
            image.write(b"\x89PNG\r\n\x1a\n"); image.flush()
            apply(self.app, {"action": "context", "threadId": "chat", "paths": [image.name]})
            apply(self.app, {"action": "attachments", "paths": []})
            self.app.send_prompt("describe")
            self.assertEqual(self.rpc.calls[-1][1]["input"][-1]["path"], str(Path(image.name).resolve()))

    def test_navigation_round_trip(self):
        selected = []
        def select(thread):
            selected.append(thread)
            self.app.thread_id = thread
        self.app.select_thread = select
        self.app.navigation_back = ["previous"]
        apply(self.app, {"action": "navigate", "direction": "back"})
        self.assertEqual(self.app.navigation_forward, ["chat"])
        apply(self.app, {"action": "navigate", "direction": "forward"})
        self.assertEqual(selected, ["previous", "chat"])

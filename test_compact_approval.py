from pathlib import Path
import tempfile
import unittest

from kai import Kai
from kai_bridge import apply
from test_kai import FakeServer


class CompactApprovalTests(unittest.TestCase):
    def setUp(self):
        directory=tempfile.TemporaryDirectory();self.addCleanup(directory.cleanup)
        self.rpc=FakeServer(directory.name);self.app=Kai(self.rpc,Path(directory.name))
        self.app.thread_id="selected";self.app.active_turn="selected-turn"
        self.app.approvals.append({"id":7,"method":"item/commandExecution/requestApproval",
                                   "params":{"threadId":"background","turnId":"background-turn","command":"swift test"}})
        self.rpc.calls.clear()

    def test_stop_targets_the_approval_turn_and_declines_its_pending_command(self):
        apply(self.app,{"action":"approval_stop","threadId":"background","approvalRequestId":"7"})
        self.assertEqual(self.rpc.calls,[("turn/interrupt",{"threadId":"background","turnId":"background-turn"})])
        self.assertEqual(self.rpc.answers,[(7,{"decision":"decline"})])
        self.assertFalse(self.app.approvals)
        self.assertEqual(self.app.thread_id,"selected")

    def test_stale_or_wrong_thread_cannot_stop_any_task(self):
        for fields in [{"threadId":"background","approvalRequestId":"old"},
                       {"threadId":"selected","approvalRequestId":"7"},{},
                       {"threadId":"background"}]:
            with self.assertRaises(ValueError):apply(self.app,{"action":"approval_stop",**fields})
        self.assertFalse(self.rpc.calls);self.assertFalse(self.rpc.answers)

    def test_missing_turn_identity_does_not_interrupt_the_selected_chat(self):
        self.app.approvals[0]["params"].pop("turnId")
        with self.assertRaisesRegex(ValueError,"no turn ID"):
            apply(self.app,{"action":"approval_stop","threadId":"background","approvalRequestId":"7"})
        self.assertFalse(self.rpc.calls);self.assertFalse(self.rpc.answers)

from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest

from kai import Kai
from kai_bridge import apply
from test_kai import FakeServer


class AutomationSettingsTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.rpc = FakeServer(self.tmp.name)
        self.app = Kai(self.rpc, Path(self.tmp.name), standalone_root=Path(self.tmp.name)/'chats')
        self.app.select_thread('task-123')
        self.app.models = [{'model':'economy','defaultReasoningEffort':'low'},
                           {'model':'manual','defaultReasoningEffort':'high'}]
        self.app.selected_model, self.app.reasoning_effort = 'manual', 'high'
        self.rpc.calls.clear()

    def test_automatic_send_preserves_existing_chat_model(self):
        apply(self.app, {'action':'send','text':'Hello','useDefaultModel':True,'defaultModel':'economy'})
        params = next(p for m,p in self.rpc.calls if m == 'turn/start')
        self.assertEqual((params['model'],params['effort']),('manual','high'))
        self.assertEqual(self.app.selected_model,'manual')

    def test_manual_send_preserves_the_in_chat_model_picker(self):
        apply(self.app, {'action':'send','text':'Hello','defaultModel':'economy'})
        params = next(p for m,p in self.rpc.calls if m == 'turn/start')
        self.assertEqual((params['model'],params['effort']),('manual','high'))

    def test_new_chat_uses_saved_default_instead_of_last_opened_chat_model(self):
        apply(self.app, {'action':'new','defaultModel':'economy'})
        params = next(p for m,p in self.rpc.calls if m == 'thread/start')
        self.assertEqual(params['model'],'economy')
        self.assertEqual((self.app.selected_model,self.app.reasoning_effort),('economy','low'))

    def test_unavailable_default_fails_before_starting_a_turn_or_new_chat(self):
        for action in ['new','new_project']:
            with self.subTest(action=action),self.assertRaisesRegex(ValueError,'default model is unavailable'):
                apply(self.app, {'action':action,'text':'Hello','useDefaultModel':True,'defaultModel':'missing'})
        self.assertFalse(any(m in {'thread/start','turn/start','thread/realtime/start'} for m,_ in self.rpc.calls))
        self.assertEqual(self.app.selected_model,'manual')

    def test_running_turn_is_steered_without_attempting_to_switch_its_model(self):
        self.app.active_turn = 'running'
        apply(self.app, {'action':'send','text':'Follow up','useDefaultModel':True,'defaultModel':'economy'})
        self.assertTrue(any(m == 'turn/steer' for m,_ in self.rpc.calls))
        self.assertFalse(any(m == 'turn/start' for m,_ in self.rpc.calls))
        self.assertEqual(self.app.selected_model,'manual')

    def test_voice_backing_model_is_configured_without_changing_speech_model(self):
        self.app.configure_model_for_voice(self.app.models[0])
        self.assertEqual(self.rpc.calls[-2],('thread/unsubscribe',{'threadId':'task-123'}))
        self.assertEqual(self.rpc.calls[-1],('thread/resume',{'threadId':'task-123','model':'economy','config':{'model_reasoning_effort':'low'}}))
        self.assertEqual(self.app.selected_model,'economy')

    def test_voice_model_changes_leave_running_turns_alone(self):
        self.app.active_turn='running'
        self.app.configure_model_for_voice(self.app.models[0])
        self.assertEqual(self.rpc.calls,[])
        self.assertEqual(self.app.selected_model,'manual')

    def test_ignored_voice_model_override_restores_chat_and_fails_before_voice(self):
        original=self.rpc.call
        def ignored(method,params=None,timeout=25):
            result=original(method,params,timeout)
            if method == 'thread/resume' and (params or {}).get('model'):
                result['model']='manual'
            return result
        self.rpc.call=ignored
        with self.assertRaisesRegex(RuntimeError,'did not apply the default model'):
            self.app.configure_model_for_voice(self.app.models[0])
        self.assertEqual(self.rpc.calls[-1],('thread/resume',{'threadId':'task-123'}))
        self.assertEqual(self.app.selected_model,'manual')

    def test_code_send_keeps_its_existing_model_controls(self):
        sent=[]
        code=SimpleNamespace(selected_id='code-chat',notice='Ready',mutate=lambda action,text:sent.append((action,text)))
        apply(self.app,{'action':'send','text':'Hello','useDefaultModel':True,'defaultModel':'not-a-codex-model'},code)
        self.assertEqual(sent,[('send','Hello')])
        self.assertEqual(self.app.selected_model,'manual')


if __name__ == '__main__':
    unittest.main()

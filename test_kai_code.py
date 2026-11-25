import copy
import json
import tempfile
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from kai_code import CodeConnection, CodeConnectionError, normalize_live_output
from kai_bridge import apply


def fixture():
    return {'selectedId':'s1','loading':False,'notice':None,'sessions':[
        {'id':'s1','title':'Code session','agent':'Claude Code','project':'/tmp/project','externalId':'native1',
         'sendToken':'current:s1:0','running':True,'runId':'run1','status':'Editing files','capabilities':{'open':True,'send':False,'cancel':True}}],
        'history':[{'id':'h1','kind':'assistant','lines':[{'text':'Actual output','style':'text'}],'speechText':''}]}

class FixtureConnection(CodeConnection):
    def __init__(self):
        super().__init__(Path('/private/tmp/unused'))
        self.responses=fixture();self.connected=True;self.epoch='current';self.calls=[]
        self.state=copy.deepcopy(self.responses)
    def _request(self,method,params=None):
        self.calls.append((method,params))
        if method=='snapshot':return copy.deepcopy(self.responses)
        return {'accepted':True,'started':method=='send'}

class CodeConnectionTests(unittest.TestCase):
    def test_only_confirmed_idle_sessions_emit_completion_for_their_run(self):
        code=FixtureConnection();previous=code.tasks()
        self.assertEqual(previous[0]['runId'],'run1')
        self.assertEqual(code.finished_tasks(previous),[])
        code.connected=False
        self.assertEqual(code.tasks(),[])
        self.assertEqual(code.tasks(include_disconnected=True),previous)
        code.state['sessions'][0]['running']=False
        self.assertEqual(code.finished_tasks(previous),[])
        code.connected=True
        self.assertEqual(code.finished_tasks(previous),[{'id':'code:s1','runId':'run1'}])
        code.state['sessions']=[]
        self.assertEqual(code.finished_tasks(previous),[])

    def test_code_private_status_envelopes_are_not_displayed_or_read(self):
        marker='<orbit_interaction version="1">'+json.dumps({'orbit_interaction':1,'card':{'kind':'status','title':'Running command','details':'Internal metadata','status':'working'}})+'</orbit_interaction>'
        lines,speech=normalize_live_output('Actual response.\n\n'+marker+'\n\nMore useful prose.')
        self.assertNotIn('orbit_interaction',speech);self.assertNotIn('Running command',speech)
        self.assertIn('More useful prose',speech)
        self.assertTrue(any(line['text']=='Internal metadata' and line['style']=='voice:voicestatus' for line in lines))
        code,_=normalize_live_output('```xml\n'+marker+'\n```')
        self.assertTrue(any('orbit_interaction' in line['text'] and line['style']=='code' for line in code))
    def test_truncated_private_envelope_and_voice_speaker_are_handled(self):
        lines,speech=normalize_live_output('truncated JSON</orbit_interaction>Actual text.')
        self.assertNotIn('JSON',speech);self.assertIn('Actual text',speech)
        marker='<orbit_interaction version="1">'+json.dumps({'orbit_interaction':1,'card':{'kind':'speaker','speaker':'assistant','text':'Spoken answer'}})+'</orbit_interaction>'
        lines,speech=normalize_live_output(marker)
        self.assertEqual(lines[0]['style'],'voice:voicetranscript');self.assertIn('Spoken answer',speech)
        lines,speech=normalize_live_output('Safe text. <orbit_interaction version="1">partial JSON')
        self.assertNotIn('JSON',speech)

    def test_lost_backend_selection_never_falls_through_to_codex(self):
        code=FixtureConnection();code.selected_id=None
        app=SimpleNamespace(thread_id="unrelated-codex",dirty=False)
        with self.assertRaises(ValueError):apply(app,{'action':'send','targetThreadId':'code:s1','text':'Must remain in Code'},code)
        self.assertEqual(code.calls,[])
    def test_selection_and_ownership_no_codex_rpc(self):
        code=FixtureConnection()
        app=SimpleNamespace(dirty=False,context_text='Old context')
        apply(app,{'action':'chat','id':'code:s1'},code)
        self.assertEqual(code.selected_id,'s1');self.assertEqual(app.context_text,'')
        self.assertEqual(code.calls[-1],('open',{'sessionId':'s1'}))
        apply(app,{'action':'interrupt'},code)
        self.assertEqual(code.calls[-1],('cancel',{'sessionId':'s1','runId':'run1'}))
        with self.assertRaises(ValueError):apply(app,{'action':'settings','model':'wrong-provider'},code)
    def test_busy_draft_and_disconnected_sessions_never_send(self):
        code=FixtureConnection();code.selected_id='s1'
        with self.assertRaises(CodeConnectionError):code.mutate('send','Prompt')
        self.assertEqual(code.calls,[])
        code.state['sessions'][0]['capabilities']['send']=True;code.connected=False
        with self.assertRaises(CodeConnectionError):code.mutate('send','Prompt')
        self.assertEqual(code.calls,[])
    def test_disconnect_retains_history_and_disables_commands(self):
        code=FixtureConnection();code.selected_id='s1';code.connected=False
        state={'activeTasks':[], 'focusThreadId':None,'project':'/original','chats':[]}
        result=code.overlay(state)
        self.assertEqual(result['threadId'],'code:s1');self.assertEqual(result['history'][0]['lines'][0]['text'],'Actual output')
        self.assertTrue(result['readOnly']);self.assertIsNone(result['activeTurnId']);self.assertEqual(code.tasks(),[])
    def test_reconnect_follows_code_selection_and_epoch(self):
        code=FixtureConnection();code.selected_id='s1'
        second=copy.deepcopy(code.responses['sessions'][0]);second['id']='s2'
        code.responses['sessions'].append(second);code.responses['selectedId']='s2'
        self.assertTrue(code.poll(force=True));self.assertEqual(code.selected_id,'s2')
    def test_validation_rejects_duplicate_sessions_and_invalid_history(self):
        data=fixture();data['sessions'].append(copy.deepcopy(data['sessions'][0]))
        with self.assertRaises(CodeConnectionError):CodeConnection._validated(data)
        data=fixture();data['history'][0]['lines'][0]['text']=123
        with self.assertRaises(CodeConnectionError):CodeConnection._validated(data)
    def test_ambiguous_send_is_never_retried(self):
        code=FixtureConnection();code.selected_id='s1';code.state['sessions'][0]['capabilities']['send']=True
        attempts=[]
        def lost(method,params):attempts.append(method);raise CodeConnectionError('Lost acknowledgement')
        code._request=lost
        with self.assertRaises(CodeConnectionError):code.mutate('send','Once')
        self.assertEqual(attempts,['send'])
    def test_code_activity_ids_cannot_collide_with_codex_and_have_provider(self):
        code=FixtureConnection();tasks=code.tasks()
        self.assertEqual(tasks[0]['id'],'code:s1');self.assertEqual(tasks[0]['scene'],'files')
        self.assertIn('Claude Code',tasks[0]['text'])
    def test_unsupported_image_context_does_not_mutate_codex(self):
        code=FixtureConnection();code.selected_id='s1'
        app=SimpleNamespace(dirty=False,context_text='')
        with self.assertRaises(ValueError):apply(app,{'action':'context','threadId':'code:s1','paths':['/tmp/image.png'],'text':'context'},code)
        self.assertEqual(app.context_text,'');self.assertEqual(code.calls,[])

if __name__=='__main__':unittest.main()

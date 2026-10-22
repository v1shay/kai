import base64
import json
from pathlib import Path
import queue
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

from kai import Kai, RPCError
from kai_bridge import apply, snapshot
from kai_voice import VoiceSession, TranscriptStore, checked_audio
from test_kai import FakeServer


class VoiceServer(FakeServer):
    def __init__(self, cwd):
        super().__init__(cwd)
        self.pages = None
        self.block = None
        self.entered = threading.Event()

    def call(self, method, params=None, timeout=25):
        if method == 'thread/timeline/list':
            if self.pages is None:
                raise RPCError('thread/timeline/list is not supported yet')
            return self.pages.get((params or {}).get('cursor'), {'data': [], 'nextCursor': None})
        if method == 'thread/realtime/appendAudio' and self.block:
            self.entered.set()
            self.block.wait(2)
        return super().call(method, params, timeout)


class VoiceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.rpc = VoiceServer(self.tmp.name)
        self.app = Kai(self.rpc, Path(self.tmp.name))
        self.app.select_thread('task-123')
        self.events = []
        self.store = TranscriptStore(Path(self.tmp.name) / 'voice')
        self.voice = VoiceSession(self.app, self.events.append, self.store)
        self.addCleanup(self.voice.close)
        self.app.voice = self.voice
        self.app.event_handler = self.voice.event
        self.voice.sync()
        self.settle(lambda: not self.voice.history_loading)

    def settle(self, predicate):
        deadline = time.monotonic() + 2
        while time.monotonic() < deadline:
            self.voice.sync()
            if predicate(): return
            time.sleep(.005)
        self.fail('Voice worker did not settle')

    def start(self):
        apply(self.app, {'action': 'voice_start', 'sessionId': 'session-a', 'targetThreadId': 'task-123', 'sdp': 'v=0\r\nTEST OFFER'})
        self.event('started', realtimeSessionId='session-a')
        self.assertEqual(self.voice.phase, 'live')

    def event(self, kind, **params):
        self.rpc.events.put({'method': 'thread/realtime/' + kind, 'params': {'threadId': 'task-123', **params}})
        self.app.drain()

    def packet(self):
        return {'data': base64.b64encode(b'\x00\x01' * 2400).decode(), 'sampleRate': 24000,
                'numChannels': 1, 'samplesPerChannel': 2400}

    def test_protocol_start_uses_existing_chat_and_normal_handoffs(self):
        self.start()
        self.settle(lambda: any(c[0] == 'thread/realtime/start' for c in self.rpc.calls))
        call = next(p for m, p in self.rpc.calls if m == 'thread/realtime/start')
        self.assertEqual(call['threadId'], 'task-123')
        self.assertEqual(call['outputModality'], 'audio')
        self.assertEqual(call['transport'], {'type':'webrtc','sdp':'v=0\r\nTEST OFFER'})
        self.assertEqual(call['version'],'v3')
        self.assertEqual(call['model'],'gpt-live-1-codex')
        self.assertNotIn('clientManagedHandoffs', call)
        self.assertNotIn('voice', call)
        self.assertTrue(call['flushTranscriptTailOnSessionEnd'])
        self.assertFalse(any(m == 'thread/start' for m, _ in self.rpc.calls))

    def test_selected_native_voice_reaches_same_thread_session(self):
        apply(self.app, {'action':'voice_start', 'sessionId':'selected-voice',
                         'targetThreadId':'task-123', 'sdp':'v=0\r\nOFFER', 'voice':'ember'})
        self.settle(lambda: any(m == 'thread/realtime/start' for m, _ in self.rpc.calls))
        params = next(p for m,p in self.rpc.calls if m == 'thread/realtime/start')
        self.assertEqual(params['voice'], 'ember')
        self.assertEqual(params['threadId'], 'task-123')
        self.assertEqual(params['model'], 'gpt-live-1-codex')
        self.assertEqual(params['transport']['type'], 'webrtc')

    def test_voice_in_existing_chat_ignores_global_default_model(self):
        self.app.models = [{'model':'economy','defaultReasoningEffort':'low'}]
        self.app.selected_model = 'manual'
        apply(self.app, {'action':'voice_start','sessionId':'model-choice','targetThreadId':'task-123',
                         'sdp':'v=0\r\nOFFER','voice':'ember','defaultModel':'economy'})
        self.settle(lambda: any(m == 'thread/realtime/start' for m,_ in self.rpc.calls))
        self.assertFalse(any(m == 'thread/unsubscribe' or (m == 'thread/resume' and p.get('model')) for m,p in self.rpc.calls))
        realtime = next(p for m,p in self.rpc.calls if m == 'thread/realtime/start')
        self.assertEqual(realtime['model'],'gpt-live-1-codex')
        self.assertEqual(realtime['voice'],'ember')
        self.assertEqual(self.app.selected_model,'manual')

    def test_invalid_voice_cannot_send_appshot_or_start_session(self):
        for voice in ['', 'unknown', 'cedar', 123, [], {}]:
            with self.subTest(voice=voice), self.assertRaisesRegex(ValueError,'Unsupported realtime voice'):
                apply(self.app, {'action':'voice_start','sessionId':'invalid-voice',
                                 'sdp':'v=0\r\nOFFER', 'voice':voice,
                                 'appshot':{'text':'Editor','paths':[]}})
        self.assertEqual(self.voice.phase,'off')
        self.assertFalse(self.app.context_text)
        self.assertFalse(any(m in {'turn/start','thread/realtime/start'} for m,_ in self.rpc.calls))

    def test_old_chat_start_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'Session changed'):
            apply(self.app, {'action': 'voice_start', 'sessionId': 'one', 'targetThreadId': 'old-chat'})
        self.assertEqual(self.voice.phase, 'off')

    def test_appshot_is_persisted_as_image_turn_in_same_chat_before_native_voice(self):
        image = Path(self.tmp.name) / 'appshot.png'
        image.write_bytes(base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+a0p8AAAAASUVORK5CYII='))
        apply(self.app, {'action':'voice_start', 'sessionId':'with-appshot', 'targetThreadId':'task-123',
                         'sdp':'v=0\r\nOFFER', 'appshot':{'text':'Application: Editor', 'paths':[str(image)]}})
        self.settle(lambda: any(m == 'thread/realtime/start' for m, _ in self.rpc.calls))
        turn = next(p for m,p in self.rpc.calls if m == 'turn/start')
        self.assertEqual(turn['threadId'],'task-123')
        self.assertIn({'type':'localImage','path':str(image.resolve())},turn['input'])
        self.assertIn('Application: Editor',turn['input'][0]['text'])
        realtime = next(p for m,p in self.rpc.calls if m == 'thread/realtime/start')
        self.assertEqual(realtime['threadId'],'task-123')
        self.assertEqual(realtime['transport']['type'],'webrtc')
        self.assertIn('Application: Editor',realtime['initialItems'][0]['text'])
        self.assertFalse(self.app.context_text);self.assertFalse(self.app.context_images)
        self.assertFalse(any(m == 'thread/start' for m,_ in self.rpc.calls))

    def test_appshot_voice_rejects_busy_turn_without_sending_image_or_mutating_context(self):
        self.app.active_turn = 'busy'
        with self.assertRaisesRegex(ValueError,'turn to finish'):
            apply(self.app, {'action':'voice_start','sessionId':'busy','sdp':'v=0\r\nOFFER',
                             'appshot':{'text':'Editor','paths':[]}})
        self.assertEqual(self.voice.phase,'off')
        self.assertFalse(any(m in {'turn/start','thread/realtime/start'} for m,_ in self.rpc.calls))
        self.assertFalse(self.app.context_text)

    def test_invalid_appshot_never_sends_or_starts_voice(self):
        for context in [False,{}, {'text':42}, {'text':'Editor','paths':'image.png'},
                        {'text':'Editor','paths':[42]}, {'text':'Editor','paths':['a','b']}]:
            with self.assertRaisesRegex(ValueError,'appshot'):
                apply(self.app, {'action':'voice_start','sessionId':'invalid','sdp':'v=0\r\nOFFER','appshot':context})
        self.assertEqual(self.voice.phase,'off')
        self.assertFalse(any(m in {'turn/start','thread/realtime/start'} for m,_ in self.rpc.calls))

    def test_failed_appshot_turn_cannot_leak_context_into_later_prompts(self):
        with patch.object(self.app,'send_prompt',side_effect=RPCError('Backend failed')):
            with self.assertRaisesRegex(RPCError,'Backend failed'):
                apply(self.app, {'action':'voice_start','sessionId':'failed','sdp':'v=0\r\nOFFER',
                                 'appshot':{'text':'Private editor','paths':[]}})
        self.assertEqual(self.voice.phase,'off')
        self.assertFalse(self.app.context_text);self.assertFalse(self.app.context_images)

    def test_plain_voice_never_creates_an_appshot_turn(self):
        self.start()
        self.assertFalse(any(m == 'turn/start' for m,_ in self.rpc.calls))

    def test_compact_approval_exposes_review_details_and_only_accepts_reviewed_identity(self):
        self.app.approvals=[{'id':7,'method':'item/commandExecution/requestApproval',
                             'params':{'threadId':'task-123','command':'echo hello','reason':'Test command'}}]
        state=snapshot(self.app)
        self.assertEqual(state['approvalRequestId'],'7')
        self.assertIn('echo hello',state['approvalSummary'])
        apply(self.app,{'action':'approve','threadId':'task-123','approvalRequestId':'7'})
        self.assertEqual(self.rpc.answers,[(7,{'decision':'accept'})])

    def test_stale_compact_approval_cannot_allow_a_new_unreviewed_request(self):
        self.app.approvals=[{'id':8,'method':'item/commandExecution/requestApproval',
                             'params':{'threadId':'task-123','command':'different command'}}]
        for action in ['approve','reject']:
            with self.assertRaisesRegex(ValueError,'Approval changed'):
                apply(self.app,{'action':action,'threadId':'task-123','approvalRequestId':'7'})
        self.assertFalse(self.rpc.answers);self.assertEqual(len(self.app.approvals),1)

    def test_sdp_answer_is_separate_from_state_and_stale_answers_are_ignored(self):
        self.start()
        self.event('sdp', sdp='v=0\r\nANSWER')
        self.assertEqual(self.events[-1], {'type':'voiceSDP','sessionId':'session-a','sdp':'v=0\r\nANSWER'})
        self.assertNotIn('ANSWER', json.dumps(snapshot(self.app)))
        self.voice.stop()
        before=len(self.events)
        self.event('sdp',sdp='v=0\r\nLATE')
        self.assertEqual(len(self.events),before)

    def test_invalid_offer_is_rejected_without_starting_a_session(self):
        for sdp in [None,'',123,'not SDP','v=0'+'a'*140000]:
            with self.assertRaisesRegex(ValueError,'WebRTC offer'):
                apply(self.app,{'action':'voice_start','sessionId':'invalid','targetThreadId':'task-123','sdp':sdp})
        self.assertEqual(self.voice.phase,'off')

    def test_audio_is_separate_from_state_and_does_not_dirty_transcript(self):
        self.start()
        self.app.dirty = False
        self.event('outputAudio/delta', audio=self.packet())
        self.assertEqual(self.events[-1]['type'], 'voiceAudio')
        self.assertFalse(self.app.dirty)
        encoded = json.dumps(snapshot(self.app))
        self.assertNotIn(self.packet()['data'], encoded)

    def test_late_audio_and_stale_stop_are_ignored(self):
        self.start()
        before = len(self.rpc.calls)
        apply(self.app, {'action': 'voice_audio', 'sessionId': 'old', 'threadId': 'task-123', 'audio': self.packet()})
        apply(self.app, {'action': 'voice_stop', 'sessionId': 'old'})
        self.assertEqual(self.voice.phase, 'live')
        self.assertFalse(any(m.endswith('appendAudio') for m, _ in self.rpc.calls[before:]))

    def test_bad_packet_fails_before_transport(self):
        self.start()
        packet = self.packet(); packet['samplesPerChannel'] = 3
        with self.assertRaisesRegex(ValueError, 'frame count'):
            apply(self.app, {'action': 'voice_audio', 'sessionId': 'session-a', 'threadId': 'task-123', 'audio': packet})
        for change in [{'data': '@@@'}, {'sampleRate': True}, {'numChannels': 0}, {'data': 'A' * 300000}]:
            with self.assertRaises(ValueError): checked_audio({**self.packet(), **change})

    def test_cancellation_orders_stop_after_inflight_audio_and_drops_queue(self):
        self.start()
        self.rpc.block = threading.Event()
        request = {'action': 'voice_audio', 'sessionId': 'session-a', 'threadId': 'task-123', 'audio': self.packet()}
        apply(self.app, request)
        self.assertTrue(self.rpc.entered.wait(1))
        for _ in range(8): apply(self.app, request)
        self.voice.stop()
        self.rpc.block.set()
        self.settle(lambda: self.voice.phase == 'off')
        calls = [m for m, _ in self.rpc.calls if m.startswith('thread/realtime/')]
        self.assertEqual(calls.count('thread/realtime/appendAudio'), 1)
        self.assertEqual(calls[-1], 'thread/realtime/stop')
        self.assertEqual(self.events[-1]['type'], 'voiceReset')

    def test_auth_error_is_visible_and_stops_native_session(self):
        self.start()
        self.event('error', message='realtime conversation requires API key auth')
        self.settle(lambda: self.voice.phase == 'failed')
        self.assertIn('API key auth', snapshot(self.app)['voice']['error'])

    def test_typing_while_live_uses_same_thread_and_normal_persisted_turn(self):
        self.start()
        apply(self.app, {'action': 'send', 'text': 'Keep going'})
        params = next(p for m, p in self.rpc.calls if m == 'turn/start')
        self.assertEqual(params['threadId'], 'task-123')
        self.assertEqual(params['input'], [{'type':'text','text':'Keep going'}])
        self.assertEqual(self.voice.phase,'live')
        self.assertFalse(any(m.endswith('appendText') for m, _ in self.rpc.calls))

    def test_flat_transcripts_stream_and_restore_alongside_normal_history(self):
        self.start()
        self.event('transcript/delta', role='user', delta='Spoken ')
        self.event('transcript/delta', role='user', delta='question')
        text = [line['text'] for block in snapshot(self.app)['history'] for line in block['lines']]
        self.assertIn('Spoken question', text)
        self.event('transcript/done', role='user', text='Spoken question?')
        self.event('transcript/done', role='assistant', text='Spoken answer.')
        self.voice.close()
        self.assertEqual([e['item']['text'] for e in self.store.read('task-123')], ['Spoken question?', 'Spoken answer.'])
        restored = VoiceSession(self.app, self.events.append, self.store)
        self.addCleanup(restored.close)
        self.app.voice = restored
        restored.sync()
        deadline=time.monotonic()+2
        while restored.history_loading and time.monotonic()<deadline:
            restored.sync();time.sleep(.005)
        text = [line['text'] for block in snapshot(self.app)['history'] for line in block['lines']]
        self.assertIn('Hi', text)
        self.assertIn('Spoken question?', text)
        self.assertIn('Spoken answer.', text)
        voices=[m for m in snapshot(self.app)['history'] if m['id'].startswith('kai-voice')]
        self.assertTrue(all(m['speechText'] is None for m in voices))

    def test_final_transcript_after_stop_updates_partial_without_duplicate(self):
        self.start()
        self.event('transcript/delta', role='user', delta='Partial')
        self.voice.stop()
        self.event('transcript/done', role='user', text='Partial, completed.')
        self.voice.close()
        entries=self.store.read('task-123')
        self.assertEqual(len(entries),1)
        self.assertEqual(entries[0]['item']['text'],'Partial, completed.')

    def test_queue_overflow_stops_without_blocking_the_bridge(self):
        self.start()
        self.rpc.block=threading.Event()
        self.addCleanup(self.rpc.block.set)
        request={'action':'voice_audio','sessionId':'session-a','threadId':'task-123','audio':self.packet()}
        apply(self.app,request);self.assertTrue(self.rpc.entered.wait(1))
        for _ in range(24): apply(self.app,request)
        started=time.monotonic()
        with self.assertRaisesRegex(ValueError,'too slow'): apply(self.app,request)
        self.assertLess(time.monotonic()-started,.2)
        self.assertEqual(self.voice.phase,'stopping')
        self.rpc.block.set()
        self.settle(lambda:self.voice.phase=='failed')

    def test_agent_streaming_and_approvals_keep_using_existing_flow(self):
        self.start()
        self.rpc.events.put({'method':'turn/started','params':{'threadId':'task-123','turn':{'id':'work','status':'inProgress','items':[]}}})
        self.rpc.events.put({'method':'item/started','params':{'threadId':'task-123','turnId':'work','item':{'id':'answer','type':'agentMessage','text':''}}})
        self.rpc.events.put({'method':'item/agentMessage/delta','params':{'threadId':'task-123','itemId':'answer','delta':'Working response'}})
        self.rpc.events.put({'id':99,'method':'item/commandExecution/requestApproval','params':{'threadId':'task-123','command':'test'}})
        self.app.drain()
        self.assertEqual(self.app._item('answer')['text'],'Working response')
        self.assertTrue(snapshot(self.app)['approval'])
        apply(self.app,{'action':'approve','threadId':'task-123'})
        self.assertEqual(self.rpc.answers[0][0],99)

    def test_canonical_timeline_preserves_mixed_order_and_deduplicates(self):
        self.rpc.pages = {None: {'data': [
            {'type': 'item', 'position': 1, 'item': self.app.turns[0]['items'][0]},
            {'type': 'realtime', 'position': 2, 'item': {'id': 'spoken', 'type': 'transcriptSegment', 'role': 'assistant', 'text': 'Voice reply'}},
        ], 'nextCursor': 'page2'}, 'page2': {'data': [
            {'type': 'item', 'position': 3, 'item': self.app.turns[0]['items'][1]},
        ], 'nextCursor': None}}
        self.voice.load_history();self.settle(lambda: not self.voice.history_loading)
        self.event('item/started', item={'id': 'spoken', 'type': 'transcriptSegment', 'role': 'assistant', 'text': 'Voice reply'})
        result=self.voice.history_items()
        self.assertEqual([i['id'] for _, i in result][:3], ['u1', 'spoken', 'a1'])
        self.assertEqual(sum(i['id']=='spoken' for _,i in result), 1)

    def test_thread_switch_stops_voice_and_drops_old_history_results(self):
        self.start()
        old=self.voice.history_generation
        self.app.thread_id='other';self.app.turns=[]
        self.voice.history_results.put((old, [{'type': 'realtime', 'item': {'id': 'private'}}], None))
        self.voice.sync()
        self.assertEqual(self.voice.history_items(), [])
        self.assertIn(self.voice.phase, {'stopping', 'off'})

    def test_store_uses_safe_names_private_permissions_and_tolerates_partial_line(self):
        entry={'type':'realtime','item':{'id':'one','text':'hello'}}
        self.store.append('../../unsafe',entry)
        path=self.store.path('../../unsafe')
        self.assertEqual(path.parent, self.store.root)
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        with path.open('a') as stream: stream.write('{broken')
        self.assertEqual(self.store.read('../../unsafe'), [entry])
        second={'type':'realtime','item':{'id':'two','text':'recovered'}}
        self.store.append('../../unsafe',second)
        self.assertEqual(self.store.read('../../unsafe'), [entry,second])

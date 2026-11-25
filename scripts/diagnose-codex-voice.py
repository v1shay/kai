#!/usr/bin/env python3
"""Opt-in real Codex voice test with the existing login and generated test audio.

Creates one ephemeral test thread, negotiates an actual WebRTC peer, sends a
generated test-audio fixture to the voice model, checks inbound audio and transcript
notifications, and closes everything. Audio output stays muted. Never prints
SDP credentials, tokens, account identity, or transcript content. The default
never opens a microphone. --held-prompt obtains a disabled microphone track for
WebKit audio startup, but sends only its generated fixture, never live input.
"""
import argparse
import base64
import json
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))
from kai import AppServer, Kai
from kai_voice import VoiceSession, TranscriptStore, REALTIME_VOICES


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--media-executable', type=Path, default=ROOT / '.build/debug/Kai.app/Contents/MacOS/NotchPrototype')
    parser.add_argument('--held-prompt',action='store_true',help='Test the held-audio path; the live microphone track and speaker remain muted')
    parser.add_argument('--voice',choices=sorted(REALTIME_VOICES),help='Test a specific native Codex voice')
    parser.add_argument('--timeout',type=float,default=45,help='Diagnostic connection/audio timeout in seconds (1–180)')
    args = parser.parse_args()
    if not 1 <= args.timeout <= 180:
        parser.error('--timeout must be between 1 and 180 seconds')
    messages = queue.Queue()
    media = subprocess.Popen([str(args.media_executable), '--diagnose-codex-voice-media', *(['--held-prompt'] if args.held_prompt else [])],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.DEVNULL, text=True, bufsize=1)
    def read_media():
        for line in media.stdout:
            try: messages.put(json.loads(line))
            except ValueError: pass
        messages.put({'type':'mediaExited'})
    threading.Thread(target=read_media, daemon=True).start()
    rpc, voice = None, None
    try:
        print("Checking headless Codex login…", flush=True)
        rpc = AppServer()
        account = rpc.call('account/read', {}).get('account') or {}
        if account.get('type') not in {'chatgpt', 'chatgptAuthTokens'}:
            raise RuntimeError('This diagnostic requires an existing ChatGPT login')
        print('Creating ephemeral voice test thread…', flush=True)
        app = Kai(rpc, Path('/private/tmp'), initial_thread_pages=0)
        thread = rpc.call('thread/start', {'cwd':'/private/tmp', 'ephemeral':True,
                                         'sandbox':'read-only', 'approvalPolicy':'never'})['thread']
        app.thread_id=thread['id'];app.thread=thread;app.turns=[];app.read_only=False
        result={'auth':'existing ChatGPT login','nativeStarted':False,'sdpAnswer':False,
                'peerConnected':False,'audioBytes':0,'assistantTranscript':False}
        if args.voice:result['selectedVoice']=args.voice
        errors=[]
        events=set()
        def emit(event):
            if event.get('type') == 'voiceSDP':
                result['sdpAnswer']=True
                media.stdin.write(json.dumps({'sdp':event['sdp']})+'\n');media.stdin.flush()
        with tempfile.TemporaryDirectory(prefix='kai-voice-diagnostic-') as directory:
            input_file=Path(directory)/'test-input.aiff'
            wave_file=Path(directory)/'test-input.wav'
            subprocess.run(['/usr/bin/say','-o',str(input_file),'Please say exactly: Kai voice connected.'],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,timeout=15)
            subprocess.run(['/usr/bin/afconvert','-f','WAVE','-d','LEI16@24000','-c','1',str(input_file),str(wave_file)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,timeout=10)
            test_audio=base64.b64encode(wave_file.read_bytes()).decode()
            if args.held_prompt:
                media.stdin.write(json.dumps({'initialPrompt':test_audio})+'\n');media.stdin.flush()
            voice=VoiceSession(app, emit, TranscriptStore(directory))
            app.voice=voice;app.event_handler=voice.event;voice.sync()
            print('Negotiating WebRTC media…', flush=True)
            sent=args.held_prompt
            deadline=time.monotonic()+args.timeout
            while time.monotonic()<deadline:
                app.drain();voice.sync()
                if voice.phase == 'live': result['nativeStarted']=True
                if voice.error:
                    errors.append(voice.error);break
                try:
                    message=messages.get(timeout=.05)
                except queue.Empty:
                    message={}
                kind=message.get('type')
                if kind == 'offer':
                    print('WebRTC offer created; starting Codex voice…', flush=True)
                    request={'sessionId':str(uuid.uuid4()),'sdp':message['sdp']}
                    if args.voice:request['voice']=args.voice
                    voice.start(request)
                elif kind == 'connected':
                    result['peerConnected']=True
                    print('WebRTC peer and data channel connected.', flush=True)
                elif kind == 'event':
                    events.add(message.get('eventType','unknown'))
                    if message.get('eventType') == 'initialPromptSent':result['initialPromptSent']=True
                    if message.get('eventType') == 'diagnosticAudioTrackAttached:suspended':
                        print('Fixture ready: Start test audio button awaits the test click.',flush=True)
                elif kind == 'audioStats':result['audioBytes']=max(result['audioBytes'],message.get('bytes',0))
                elif kind in {'error','mediaExited'}:
                    errors.append(message.get('message') or 'Media process exited');break
                if result['peerConnected'] and result['nativeStarted'] and not sent:
                    print('Injecting generated test audio (no microphone)…',flush=True)
                    media.stdin.write(json.dumps({'testAudio':test_audio})+'\n');media.stdin.flush()
                    sent=True
                result['assistantTranscript']=any(item.get('_voice') and item.get('type') == 'agentMessage'
                                                   and item.get('text','').strip() for _,item in voice.history_items())
                if args.held_prompt:
                    result['userTranscript']=any(item.get('_voice') and item.get('type') == 'userMessage'
                                                   and item.get('text','').strip() for _,item in voice.history_items())
                if result['audioBytes'] > 0 and result['assistantTranscript'] and (not args.held_prompt or result.get('initialPromptSent') and result.get('userTranscript')):
                    break
            result['fixtureSent']=sent
            result['passed']=all(result[key] for key in ['nativeStarted','sdpAnswer','peerConnected','audioBytes','assistantTranscript'])
            if args.held_prompt:result['passed']=result['passed'] and result.get('initialPromptSent',False) and result.get('userTranscript',False)
            if errors:result['errors']=errors
            if not result['passed']:result['observedEvents']=sorted(events)
            print(json.dumps(result, indent=2),flush=True)
            voice.close();voice=None
            return 0 if result['passed'] else 1
    finally:
        if voice:voice.close()
        if rpc:rpc.close()
        if media.poll() is None:
            try:media.stdin.write('{"type":"stop"}\n');media.stdin.flush();media.wait(timeout=3)
            except (OSError,subprocess.TimeoutExpired):media.terminate()
        if media.poll() is None:
            try:media.wait(timeout=2)
            except subprocess.TimeoutExpired:media.kill();media.wait()


if __name__ == '__main__':
    raise SystemExit(main())

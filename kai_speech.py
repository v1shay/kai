"""Persistent local Piper worker. JSON lines in/out; models never downloaded."""
import base64
import json
import sys
import time


def main():
    from piper.voice import PiperVoice
    start = time.monotonic()
    voice = PiperVoice.load(sys.argv[1])
    print(json.dumps({'ready': True, 'loadMs': round((time.monotonic()-start)*1000)}), flush=True)
    for line in sys.stdin:
        try:
            request = json.loads(line)
            start = time.monotonic()
            for chunk in voice.synthesize(request['text']):
                print(json.dumps({'token': request['token'], 'rate': chunk.sample_rate,
                                  'audio': base64.b64encode(chunk.audio_int16_bytes).decode(),
                                  'synthesisMs': round((time.monotonic()-start)*1000)}), flush=True)
            print(json.dumps({'token': request['token'], 'done': True}), flush=True)
        except Exception as exc:
            print(json.dumps({'token': request.get('token'), 'error': str(exc)}), flush=True)


if __name__ == '__main__':
    try:
        main()
    except Exception as exc:
        print(json.dumps({'error': str(exc)}), flush=True)
        sys.exit(1)

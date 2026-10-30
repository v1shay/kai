#!/usr/bin/env python3
"""One-call Antigravity PreToolUse approval; never changes provider allow rules."""
import json,os,socket,sys
LIMIT=64*1024
try:
 payload=sys.stdin.buffer.read(2*1024*1024+1)
 if len(payload)>2*1024*1024:raise ValueError('Tool details exceed the review limit')
 value=json.loads(payload)
 value.pop("lastUserInput",None)
 packet=json.dumps({'token':os.environ['KAI_AGY_PERMISSION_TOKEN'],'payload':value},separators=(',',':')).encode()+b'\n'
 if len(packet)>LIMIT:raise ValueError('Tool details exceed the review limit')
 with socket.socket(socket.AF_UNIX)as connection:
  connection.settimeout(600);connection.connect(os.environ['KAI_AGY_PERMISSION_SOCKET']);connection.sendall(packet)
  reply=connection.makefile('rb').readline(4097)
  if len(reply)>4096:raise ValueError('Invalid approval response')
  decision=json.loads(reply)
  if decision.get('decision')not in {'allow','auto_approve','deny'}:raise ValueError('Invalid approval decision')
  print(json.dumps(decision))
except Exception:
 # A missing/closed host, malformed input, cancellation or timeout never grants a tool.
 print(json.dumps({'decision':'deny','reason':'The Kai approval connection closed before this exact action was approved.'}))

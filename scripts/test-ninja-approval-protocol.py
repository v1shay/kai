#!/usr/bin/env python3
"""Exercise the packaged worker's approval transport against an isolated CLI.
Never starts or changes the user's worker, provider config, logins, or projects.
"""
import argparse,hashlib,json,os,pathlib,socket,subprocess,tempfile,time,uuid

CLI = r'''#!/usr/bin/python3
import json,sys,time
if '--version' in sys.argv:print('codex-cli 0.166.0');sys.exit()
def send(v):print(json.dumps(v),flush=True)
def recv():
 v=json.loads(sys.stdin.readline())
 if v.get('method')=='config/read':send({'id':v['id'],'result':{'config':{'model_provider':'openai'}}});return recv()
 return v
v=recv();assert v['method']=='initialize';send({'id':v['id'],'result':{}})
assert recv()['method']=='initialized'
v=recv();assert v['method']=='thread/start';thread='12345678-1234-4234-8234-123456789012';send({'id':v['id'],'result':{'thread':{'id':thread}}})
v=recv();assert v['method']=='turn/start';send({'id':v['id'],'result':{'turn':{'id':'fixture-turn'}}})
base={'threadId':thread,'turnId':'fixture-turn','serverName':'isolated-fixture','mode':'form'}
for token,schema,expected in [('empty',{'type':'object','properties':{}},{}),('boolean',{'type':'object','properties':{'confirm':{'type':'boolean'}},'required':['confirm']},{'confirm':False})]:
 send({'id':token,'method':'mcpServer/elicitation/request','params':dict(base,requestedSchema=schema)})
 v=recv();assert v=={'id':token,'result':{'action':'accept','content':expected}},v
send({'id':'command','method':'item/commandExecution/requestApproval','params':dict(base,command='printf fixture',availableDecisions=['accept','decline','cancel'])})
v=recv();assert v=={'id':'command','result':{'decision':'decline'}},v
send({'method':'item/agentMessage/delta','params':dict(base,itemId='final',delta='KAI_APPROVAL_PROTOCOL_OK')})
send({'method':'turn/completed','params':{'threadId':thread,'turn':{'id':'fixture-turn','status':'completed'}}})
time.sleep(.1)
'''

def main():
 parser=argparse.ArgumentParser();parser.add_argument('binary',type=pathlib.Path);args=parser.parse_args()
 with tempfile.TemporaryDirectory(prefix='kai-approval-protocol-') as temporary:
  root=pathlib.Path(temporary).resolve();data=root/'data';project=root/'project';project.mkdir();bin_dir=root/'bin';bin_dir.mkdir();cli=bin_dir/'codex';cli.write_text(CLI);cli.chmod(0o700)
  address='/private/tmp/kai-ninjas-'+str(os.getuid())+'-'+hashlib.sha256(str(data).encode()).hexdigest()[:10]+'.sock'
  env=dict(os.environ,PATH=str(bin_dir)+':/usr/bin:/bin',CODEX_HOME=str(root/'codex'))
  def call(value):
   with socket.socket(socket.AF_UNIX) as s:
    s.settimeout(12);s.connect(address);s.sendall(json.dumps(value).encode()+b'\n');v=json.loads(s.makefile('rb').readline(8*1024*1024))
   if 'error'in v:raise RuntimeError(v['error'])
   return v
  with (root/'worker.log').open('w') as log:
   process=subprocess.Popen([str(args.binary.resolve()),'--ninja-worker','--data',str(data)],env=env,stdout=log,stderr=subprocess.STDOUT)
   try:
    deadline=time.monotonic()+5
    while not pathlib.Path(address).exists():
     assert process.poll() is None,(root/'worker.log').read_text()
     assert time.monotonic()<deadline,'Worker startup timeout';time.sleep(.02)
    profile=call({'action':'save_profile','profile':{'id':str(uuid.uuid4()),'name':'Fixture','persona':'Isolated approval test','style':'ice','project':str(project),'agent':'Codex','model':None,'effort':None,'ninja':None,'edits':True,'access':'workspace','approvals':'never','revision':0,'orchestrator':False}})
    task=call({'action':'submit','id':profile['id'],'prompt':'Isolated approval transport fixture','due':int(time.time()),'repeat':None});ident=task['id'];seen=set();latencies=[];deadline=time.monotonic()+20
    while time.monotonic()<deadline:
     task=call({'action':'task','id':ident})
     for request in task['requests']:
      token=request['id']
      if token in seen:continue
      kind=request['request_kind']
      if 'Questions'in kind:
       questions=kind['Questions']['questions'];answer={'Answers':{q['id']:['false'] for q in questions}}
      else:answer={'Choice':'decline'}
      started=time.monotonic();assert call({'action':'respond','id':ident,'request':token,'answer':answer})['ok'];latencies.append(time.monotonic()-started);seen.add(token)
      assert all(r['id']!=token for r in call({'action':'task','id':ident})['requests']),'Acknowledged before native resolution'
     if task['state']not in ['queued','running','approval']:break
     time.sleep(.02)
    assert task['state']=='completed',task['status'];assert len(seen)==3,len(seen)
    assert 'KAI_APPROVAL_PROTOCOL_OK'in call({'action':'conversation','id':profile['id'],'chat':task['chat']})['messages'][-1]['brief']
    print(json.dumps({'passed':True,'empty_form':True,'boolean_form':True,'command_rejection':True,'resolved_before_acknowledgement':True,'max_ack_seconds':round(max(latencies),3)}))
   finally:
    process.terminate()
    try:process.wait(timeout=5)
    except subprocess.TimeoutExpired:process.kill();process.wait()
    try:pathlib.Path(address).unlink()
    except FileNotFoundError:pass
if __name__=='__main__':main()

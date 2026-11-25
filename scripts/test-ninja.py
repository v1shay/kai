#!/usr/bin/env python3
"""Protocol/authorization tests, real loopback HTTP; no credentials or model needed."""
import io
from unittest.mock import patch
import sys, contextlib, importlib.util, json, pathlib, tempfile, threading, types, unittest, urllib.request, urllib.error
ROOT=pathlib.Path(__file__).resolve().parents[1]
def load(name):
 sys.path.insert(0,str(ROOT/'integrations/Code/native_helpers'));spec=importlib.util.spec_from_file_location(name,ROOT/'integrations/Code/native_helpers'/('kai_ninja'+('' if name=='gateway' else '_host')+'.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
G,H=load('gateway'),load('host')
class Fixture(G.http.server.BaseHTTPRequestHandler):
 def log_message(self,*a):pass
 def do_POST(self):
  self.server.received.append((self.path,dict(self.headers),json.loads(self.rfile.read(int(self.headers['Content-Length'])))))
  self.send_response(200);self.send_header('Content-Type','application/x-ndjson');self.end_headers()
  for v in self.server.events:self.wfile.write((json.dumps(v)+'\n').encode());self.wfile.flush()
class Protocol(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.upstream=G.http.server.ThreadingHTTPServer(('127.0.0.1',0),Fixture);self.upstream.received=[];self.upstream.events=[]
  self.server=G.Server(types.SimpleNamespace(routes=self.tmp.name,token='private-test-token'));self.route='a'*64
  self.selection={'engine':'ollama','model':'test-model','endpoint':'http://127.0.0.1:'+str(self.upstream.server_port),'context':8192,'thinking':'high','node':False}
  pathlib.Path(self.tmp.name,self.route+'.json').write_text(json.dumps(self.selection))
  for s in (self.server,self.upstream):threading.Thread(target=s.serve_forever,daemon=True).start()
 def tearDown(self):
  for s in (self.server,self.upstream):s.shutdown();s.server_close()
  self.tmp.cleanup()
 def call(self,body,path='responses',token='private-test-token'):
  req=urllib.request.Request('http://127.0.0.1:'+str(self.server.server_port)+'/route/'+self.route+'/v1/'+path,json.dumps(body).encode(),{'Content-Type':'application/json','Authorization':'Bearer '+token})
  with urllib.request.urlopen(req,timeout=4) as r:return [json.loads(line[6:]) for line in r.read().decode().splitlines() if line.startswith('data: ')]
 def test_codex_custom_tool_roundtrip_and_reasoning(self):
  self.upstream.events=[{'message':{'thinking':'Check file first.'}},{'message':{'tool_calls':[{'function':{'name':'apply_patch','arguments':{'input':'*** Begin Patch\n*** End Patch'}}}]}},{'done':True,'prompt_eval_count':11,'eval_count':7}]
  body={'stream':True,'input':[{'role':'user','content':'Do it'}],'tools':[{'type':'custom','name':'apply_patch','description':'Patch'}]}
  events=self.call(body);response=events[-1]['response'];self.assertEqual(response['status'],'completed');self.assertEqual(response['usage']['total_tokens'],18)
  call=response['output'][-1];self.assertEqual(call['type'],'custom_tool_call');self.assertEqual(call['input'],'*** Begin Patch\n*** End Patch')
  request=self.upstream.received[-1];self.assertNotIn('Authorization',request[1]);self.assertEqual(request[2]['think'],'high');self.assertEqual(request[2]['model'],'test-model');self.assertEqual(request[2]['tools'][0]['function']['parameters']['required'],['input'])
  converted=G.messages_for({'input':[call,{'type':'custom_tool_call_output','call_id':call['call_id'],'output':'Success'}]});self.assertEqual(converted[-1]['tool_name'],'apply_patch')
 def test_anthropic_custom_tools_keep_their_real_input_schema(self):
  tools,custom=G.tools_for({'tools':[{'type':'custom','name':'Read','input_schema':{'type':'object','properties':{'file_path':{'type':'string'}},'required':['file_path']}}]},True)
  self.assertEqual(tools[0]['function']['parameters']['required'],['file_path']);self.assertFalse(custom)
 def test_reasoning_precedes_parallel_tools_without_losing_context(self):
  messages=G.messages_for({'input':[{'type':'reasoning','summary':[{'type':'summary_text','text':'Inspect two files.'}]},{'type':'function_call','name':'Read','call_id':'a','arguments':'{}'},{'type':'function_call','name':'Read','call_id':'b','arguments':'{}'}]})
  self.assertEqual(len(messages),1);self.assertEqual(len(messages[0]['tool_calls']),2);self.assertEqual(messages[0]['thinking'],'Inspect two files.')
 def test_claude_messages_streams_native_tool_use(self):
  self.upstream.events=[{'message':{'content':'Working.'}},{'message':{'tool_calls':[{'function':{'name':'Read','arguments':{'file_path':'test.txt'}}}]}},{'done':True}]
  events=self.call({'stream':True,'system':[{'type':'text','text':'Local coding'}],'messages':[{'role':'user','content':[{'type':'text','text':'Read it'}]}],'tools':[{'name':'Read','input_schema':{'type':'object'}}]},'messages')
  self.assertEqual(events[0]['type'],'message_start');self.assertEqual(events[-2]['delta']['stop_reason'],'tool_use');self.assertTrue(any(v.get('content_block',{}).get('type')=='tool_use' for v in events));self.assertEqual(self.upstream.received[-1][2]['messages'][0]['content'],'Local coding')
 def test_truncated_generation_fails_instead_of_claiming_completion(self):
  self.upstream.events=[{'message':{'content':'Partial'}}];events=self.call({'stream':True,'input':'Hello'});self.assertEqual(events[-1]['type'],'response.failed');self.assertFalse(any(e['type']=='response.completed' for e in events))
 def test_gateway_does_not_survive_its_owner(self):
  self.server.owner_pid=-1
  with self.assertRaises(SystemExit):self.server.service_actions()
  self.server.owner_pid=G.os.getppid()
 def test_gateway_requires_private_credential(self):
  with self.assertRaises(urllib.error.HTTPError) as error:self.call({'stream':True,'input':'Hi'},token='wrong')
  self.assertEqual(error.exception.code,401);self.assertEqual(self.upstream.received,[])
 def test_tool_results_and_images_are_preserved_without_remote_fetch(self):
  result=G.messages_for({'messages':[{'role':'assistant','content':[{'type':'tool_use','id':'id1','name':'Read','input':{'path':'a'}}]},{'role':'user','content':[{'type':'tool_result','tool_use_id':'id1','content':'abc'},{'type':'image','source':{'type':'base64','media_type':'image/png','data':'abcd'}}]}]},True)
  self.assertEqual(result[1]['content'],'abc');self.assertEqual(result[1]['tool_name'],'Read');self.assertEqual(result[2]['images'],['abcd'])
  with self.assertRaises(ValueError):G.messages_for({'input':[{'role':'user','content':[{'type':'input_image','image_url':'https://example.com/a.png'}]}]})
 def test_mcp_structured_results_and_tool_images_are_preserved(self):
  content=[{'type':'text','text':'Screenshot'},{'type':'image','source':{'type':'base64','media_type':'image/png','data':'abcd'}}]
  messages=G.messages_for({'messages':[{'role':'user','content':[{'type':'tool_result','tool_use_id':'tool1','content':content}]}]},True)
  self.assertEqual(messages[0]['content'],'Screenshot');self.assertEqual(messages[1]['images'],['abcd']);self.assertIn('answer',G.text({'answer':42}))
 def test_system_updates_do_not_displace_the_latest_user_request(self):
  messages=G.messages_for({'system':'Harness instructions','messages':[{'role':'user','content':'Do this exact task'},{'role':'system','content':'Environment update'}]},True)
  self.assertEqual(messages[0]['content'],'Harness instructions\n\nEnvironment update');self.assertEqual(messages[-1]['content'],'Do this exact task')
 def test_llama_structured_output_uses_its_documented_schema_field(self):
  route=dict(self.selection,engine='llama_cpp');_,body,_=G.generation({'messages':[{'role':'user','content':'Judge'}],'output_config':{'format':{'type':'json_schema','schema':{'type':'object'}}}},route,True)
  self.assertEqual(body['response_format'],{'type':'json_schema','schema':{'type':'object'}})
 def test_engine_specific_thinking_does_not_change_to_an_invented_level(self):
  route=dict(self.selection,engine='llama_cpp',thinking='ultra',node=True);url,payload,_=G.generation({'input':'Hi'},route);self.assertEqual(payload['reasoning_effort'],'ultra');self.assertEqual(payload['kai_context'],8192);self.assertTrue(url.endswith('/infer/llama_cpp/v1/chat/completions'))
 def test_sampling_is_consistent_and_explicit_harness_settings_are_preserved(self):
  for engine in ('ollama','llama_cpp'):
   route=dict(self.selection,engine=engine)
   _,default,_=G.generation({'input':'Code'},route)
   self.assertEqual((default['options'] if engine=='ollama' else default)['temperature'],0)
   _,explicit,_=G.generation({'input':'Code','temperature':0.6,'top_p':0.9,'top_k':20},route)
   values=explicit['options'] if engine=='ollama' else explicit
   self.assertEqual([values[k] for k in ('temperature','top_p','top_k')],[0.6,0.9,20])
 def test_qwen_native_dialect_handles_omitted_opener_and_chunk_boundaries(self):
  tools=[{'type':'function','function':{'name':'Read','parameters':{'type':'object','properties':{'file_path':{'type':'string'},'offset':{'type':'integer'}},'required':['file_path']}}}]
  raw='I will inspect it.\n\n<function=Read>\n<parameter=file_path>\n/tmp/proof.txt\n</parameter>\n<parameter=offset>\n2\n</parameter>\n</function>\n</tool_call>'
  output=[];decoder=G.QwenToolText('qwen3-coder:30b',tools,output.append)
  for letter in raw:decoder.feed(letter)
  self.assertEqual(decoder.finish(False),[('Read',{'file_path':'/tmp/proof.txt','offset':2})]);self.assertEqual(''.join(output),'I will inspect it.\n\n')
 def test_qwen_decoder_never_promotes_examples_unknown_names_or_partial_calls(self):
  tools=[{'function':{'name':'Read','parameters':{'type':'object','properties':{'file_path':{'type':'string'}},'required':['file_path']}}}]
  valid='<function=Read><parameter=file_path>/tmp/proof</parameter></function></tool_call>'
  for model,raw,structured in [('other-model',valid,False),('qwen3-coder','```xml\n'+valid+'\n```',False),('qwen3-coder','Example: '+valid,False),('qwen3-coder',valid.replace('Read','Bash'),False),('qwen3-coder',valid.removesuffix('</tool_call>'),False),('qwen3-coder',valid,True),('qwen3-coder',valid+' additional text',False),('qwen3-coder',valid.replace('file_path','unexpected'),False)]:
   output=[];decoder=G.QwenToolText(model,tools,output.append)
   for letter in raw:decoder.feed(letter)
   self.assertEqual(decoder.finish(structured),[]);self.assertEqual(''.join(output),raw)
 def test_qwen_engine_text_becomes_native_tool_event_without_changing_approvals(self):
  self.selection['model']='qwen3-coder:30b';pathlib.Path(self.tmp.name,self.route+'.json').write_text(json.dumps(self.selection))
  self.upstream.events=[{'message':{'content':'<function=Read>\n<parameter=file_path>\n/tmp/proof\n</parameter>\n</function>\n</tool_call>'}},{'done':True}]
  events=self.call({'stream':True,'messages':[{'role':'user','content':'Read it'}],'tools':[{'name':'Read','input_schema':{'type':'object','properties':{'file_path':{'type':'string'}},'required':['file_path']}}]},'messages')
  self.assertTrue(any(v.get('content_block',{}).get('name')=='Read' for v in events));self.assertEqual(events[-2]['delta']['stop_reason'],'tool_use')
class ManagedWorker(unittest.TestCase):
 def test_missing_socket_wakes_installed_service_without_spawning_another_worker(self):
  with tempfile.TemporaryDirectory() as folder:
   home=pathlib.Path(folder);worker=home/'runtime';worker.touch();service=home/'Library/LaunchAgents/com.kai.ninjas.worker.plist';service.parent.mkdir(parents=True);service.touch()
   server=H.Server.__new__(H.Server);server.args=types.SimpleNamespace(worker=str(worker));server.engines=types.SimpleNamespace(lock=threading.Lock())
   attempts=[]
   class Socket:
    def __enter__(self):return self
    def __exit__(self,*args):pass
    def connect(self,address):
     attempts.append(address)
     if len(attempts)==1:raise ConnectionRefusedError()
    def settimeout(self,value):pass
    def sendall(self,value):self.request=json.loads(value)
    def makefile(self,mode):return io.BytesIO(b'{"tasks":[]}\n')
   with patch.object(H.pathlib.Path,'home',return_value=home),patch.object(H.socket,'socket',side_effect=lambda *a:Socket()),patch.object(H.subprocess,'run',return_value=types.SimpleNamespace(returncode=0)) as run,patch.object(H.subprocess,'Popen',side_effect=AssertionError('Managed worker must only be spawned by launchd')):
    self.assertEqual(server.worker_call({'action':'snapshot'}),{'tasks':[]})
    self.assertTrue(any('kickstart' in c.args[0] for c in run.call_args_list))

class HostAuth(unittest.TestCase):
 def test_host_requires_owner_identity_and_has_no_workspace_or_shell_api(self):
  s=H.Server(types.SimpleNamespace(owner='owner@example.com',port=0));threading.Thread(target=s.serve_forever,daemon=True).start()
  try:
   url='http://127.0.0.1:'+str(s.server_port)
   for headers in ({},{'Tailscale-User-Login':'other@example.com'},{'Tailscale-User-Login':'owner@example.com','Origin':'https://attacker.test'}):
    with self.assertRaises(urllib.error.HTTPError) as error:urllib.request.urlopen(urllib.request.Request(url+'/status',headers=headers));self.assertEqual(error.exception.code,403)
   with urllib.request.urlopen(urllib.request.Request(url+'/status',headers={'Tailscale-User-Login':'owner@example.com'})) as r:self.assertEqual(json.load(r)['version'],1)
   with self.assertRaises(urllib.error.HTTPError) as error:urllib.request.urlopen(urllib.request.Request(url+'/exec',data=b'{}',headers={'Tailscale-User-Login':'owner@example.com'}))
   self.assertEqual(error.exception.code,404)
  finally:s.shutdown();s.server_close()
 def test_model_file_path_and_cloud_guards(self):
  engine=H.Engines(pathlib.Path('/tmp'))
  for model in ('../../secret','model:cloud','a/../../b','/etc/passwd'):
   with self.assertRaises(ValueError):engine.blob(model)
if __name__=='__main__':unittest.main()

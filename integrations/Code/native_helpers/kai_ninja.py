#!/usr/bin/env python3
"""Private loopback protocol bridge. Only inference leaves this machine.

No tools execute here. Codex/Claude retain execution, MCP, approvals and history.
Routes are per chat, immutable, private files; upstream credentials are never
inherited from a cloud harness. No retries after sending a generation request.
"""
from kai_inference_stream import lines as inference_lines
import argparse, base64, contextlib, hmac, http.server, json, os, pathlib
import secrets, socket, threading, time, urllib.request, urllib.error, urllib.parse, uuid, re
import ssl, subprocess, sys

MAX_BODY = 32 * 1024 * 1024
MAX_LINE = 8 * 1024 * 1024
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self,*args,**kwargs):raise ValueError('Inference redirects are not followed')
TLS=ssl.create_default_context()
if sys.platform=='darwin':
    try:
        roots=subprocess.run(['/usr/bin/security','find-certificate','-a','-p','/System/Library/Keychains/SystemRootCertificates.keychain'],capture_output=True,timeout=3,check=True).stdout
        TLS.load_verify_locations(cadata=roots.decode('ascii'))
    except Exception:pass # Standard verified TLS remains enabled.
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}),urllib.request.HTTPSHandler(context=TLS),NoRedirect())

def uid(prefix): return prefix + uuid.uuid4().hex

def dumps(v): return json.dumps(v, separators=(',', ':'), ensure_ascii=False)

def text(v):
    if isinstance(v, str): return v
    if isinstance(v,dict):return v.get('text') if v.get('type')=='text' else dumps(v)
    values=[]
    for x in v or []:
        if not isinstance(x,dict):values.append(str(x))
        elif x.get('type') in ('image','input_image','image_url'):continue
        elif x.get('type')=='tool_reference':values.append('Tool now available: '+x.get('tool_name',''))
        else:values.append(str(x.get('text',x.get('output',dumps(x)))))
    return '\n'.join(values)

def tools_for(body, anthropic=False):
    tools, custom = [], set()
    pending=list(body.get('tools', []))
    for t in pending:
        if t.get('type')=='namespace':pending.extend(t.get('tools',[]));continue
        if t.get('type') in ('function', 'custom') or anthropic:
            f = t.get('function', t)
            name = f.get('name')
            if not name: continue
            schema = f.get('input_schema', f.get('parameters', {'type':'object','properties':{}}))
            if not anthropic and t.get('type') == 'custom':
                custom.add(name)
                schema = {'type':'object','properties':{'input':{'type':'string','description':'The complete freeform tool input, unchanged.'}},'required':['input']}
            tools.append({'type':'function','function':{'name':name,'description':f.get('description',''),'parameters':schema}})
    return tools, custom

class QwenToolText:
    """Decode Qwen3-Coder's native XML dialect when its engine misses an opener.

    Same documented dialect as llama.cpp/common/parsers/qwen3-coder.cpp;
    Ollama issue #16686 describes the missing <tool_call> opener. This is not a
    general text/Markdown tool interpreter. Complete closing tags, a line-start
    opener, declared names, and typed parameters are mandatory. Never execute a
    quoted example, incomplete block, unknown tool, or mixed structured call.
    """
    starts=('<tool_call>','<function=')
    def __init__(self,model,tools,emit):
        family=model.lower().rsplit('/',1)[-1].split(':',1)[0]
        self.enabled=family=='qwen3-coder' or family.startswith('qwen3-coder-')
        self.tools={t['function']['name']:t['function']['parameters'] for t in tools}
        self.emit=emit;self.pending='';self.line_nonspace=False;self.quoted=False;self.fence_tail='';self.capturing=False
    def output(self,value):
        self.quoted=self.quoted or any(fence in self.fence_tail+value for fence in ('```','~~~'))
        self.fence_tail=(self.fence_tail+value)[-2:]
        self.line_nonspace=bool(value.rsplit('\n',1)[-1].strip()) if '\n' in value else self.line_nonspace or bool(value.strip())
        self.emit(value)
    def feed(self,value):
        if not self.enabled:return self.output(value)
        self.pending+=value
        if len(self.pending)>MAX_LINE:raise ValueError('Qwen tool block exceeds 8 MiB')
        if self.capturing:return
        positions=[self.pending.find(tag) for tag in self.starts if tag in self.pending]
        if positions:
            index=min(positions);before=self.pending[:index]
            line_nonspace=bool(before.rsplit('\n',1)[-1].strip()) if '\n' in before else self.line_nonspace or bool(before.strip())
            quoted=self.quoted or any(fence in self.fence_tail+before for fence in ('```','~~~'))
            if not line_nonspace and not quoted:
                self.output(self.pending[:index]);self.pending=self.pending[index:];self.capturing=True;return
            # An example/inline marker stays ordinary text for the entire turn.
            self.enabled=False;self.output(self.pending);self.pending='';return
        keep=0
        for tag in self.starts:
            for length in range(1,len(tag)):
                if self.pending.endswith(tag[:length]):keep=max(keep,length)
        count=len(self.pending)-keep
        if count:self.output(self.pending[:count]);self.pending=self.pending[count:]
    @staticmethod
    def value(raw,schema):
        kind=schema.get('type','string')
        kinds=kind if isinstance(kind,list) else [kind]
        if 'string' in kinds:return raw
        value=json.loads(raw)
        if not QwenToolText.valid(value,schema):raise ValueError('Invalid Qwen tool parameter')
        return value
    @staticmethod
    def valid(value,schema):
        if 'enum' in schema and value not in schema['enum']:return False
        if 'const' in schema and value!=schema['const']:return False
        for key in ('anyOf','oneOf'):
            if key in schema and not any(QwenToolText.valid(value,s) for s in schema[key]):return False
        kind=schema.get('type');kinds=kind if isinstance(kind,list) else [kind]
        matches={'string':isinstance(value,str),'integer':type(value) is int,'number':type(value) in (int,float),'boolean':type(value) is bool,'object':isinstance(value,dict),'array':isinstance(value,list),'null':value is None}
        if kind and not any(matches.get(k,False) for k in kinds):return False
        if isinstance(value,dict):
            props=schema.get('properties',{})
            if any(k not in value for k in schema.get('required',[])):return False
            if schema.get('additionalProperties') is False and any(k not in props for k in value):return False
            if any(not QwenToolText.valid(v,props.get(k,{})) for k,v in value.items()):return False
        if isinstance(value,list) and isinstance(schema.get('items'),dict):
            if any(not QwenToolText.valid(v,schema['items']) for v in value):return False
        return True
    def decode(self):
        remaining=self.pending.strip();calls=[]
        # The closing wrapper is required even when the known bug omits its opener.
        while remaining:
            if remaining.startswith('<tool_call>'):remaining=remaining[len('<tool_call>'):].lstrip()
            match=re.match(r'<function=([A-Za-z0-9_./:-]+)>',remaining)
            if not match or match[1] not in self.tools:return None
            name=match[1];schema=self.tools[name];remaining=remaining[match.end():].lstrip();args={}
            while remaining.startswith('<parameter='):
                parameter=re.match(r'<parameter=([A-Za-z_][A-Za-z0-9_]*)>',remaining)
                if not parameter:return None
                key=parameter[1];remaining=remaining[parameter.end():];end=remaining.find('</parameter>')
                if end<0 or key in args or key not in schema.get('properties',{}):return None
                raw=remaining[:end].removeprefix('\n').removesuffix('\n')
                args[key]=self.value(raw,schema['properties'][key]);remaining=remaining[end+len('</parameter>'):].lstrip()
            if not remaining.startswith('</function>'):return None
            remaining=remaining[len('</function>'):].lstrip()
            if not remaining.startswith('</tool_call>') or not self.valid(args,schema):return None
            remaining=remaining[len('</tool_call>'):].lstrip();calls.append((name,args))
            if len(calls)>32:return None
        return calls or None
    def finish(self,has_structured_calls):
        calls=None
        if self.capturing and not has_structured_calls:
            try:calls=self.decode()
            except (ValueError,TypeError,KeyError):pass
        if not calls:self.output(self.pending)
        self.pending=''
        return calls or []

def image_url(part):
    url = part.get('image_url', '')
    if isinstance(url, dict): url = url.get('url', '')
    if not url.startswith('data:') or ';base64,' not in url:
        raise ValueError('Ninja images must contain embedded data; remote image URLs are not fetched')
    return url.split(';base64,',1)[1]

def messages_for(body, anthropic=False):
    messages, names, pending_reasoning = [], {}, ""
    system = body.get('system') if anthropic else body.get('instructions')
    if system: messages.append({'role':'system','content':text(system)})
    entries = body.get('messages', []) if anthropic else body.get('input', [])
    if isinstance(entries, str): entries = [{'role':'user','content':entries}]
    def tool_result(call,content):
        messages.append({'role':'tool','tool_call_id':call,'tool_name':names.get(call,''),'content':text(content)})
        images=[]
        for p in content if isinstance(content,list) else []:
            if p.get('type') in ('input_image','image_url'):images.append(image_url(p))
            elif p.get('type')=='image' and p.get('source',{}).get('type')=='base64':images.append(p['source']['data'])
        if images:messages.append({'role':'user','content':'Images returned by native tool '+names.get(call,call),'images':images})
    for e in entries:
        kind = e.get('type', 'message')
        if kind in ('function_call','custom_tool_call'):
            name, call = e['name'], e['call_id']; names[call] = name
            args = {'input':e.get('input','')} if kind == 'custom_tool_call' else json.loads(e.get('arguments','{}'))
            m={'role':'assistant','content':'','tool_calls':[{'id':call,'type':'function','function':{'name':name,'arguments':args}}]}
            if pending_reasoning:m['thinking']=pending_reasoning;pending_reasoning=''
            messages.append(m)
        elif kind in ('function_call_output','custom_tool_call_output'):
            tool_result(e['call_id'],e.get('output',''))
        elif kind == 'reasoning':
            # The visible summary is context, never an invented hidden state.
            summary = text(e.get('summary', []))
            if summary:pending_reasoning+=summary
        elif e.get('role'):
            content = e.get('content', '')
            if isinstance(content,str):
                m={'role':'system' if e['role']=='developer' else e['role'],'content':content}
                if e['role']=='assistant' and pending_reasoning:m['thinking']=pending_reasoning;pending_reasoning=''
                messages.append(m);continue
            buf, imgs = [], []
            def flush():
                if buf or imgs:
                    m={'role':e['role'],'content':'\n'.join(buf)}
                    if imgs: m['images']=list(imgs)
                    messages.append(m);buf.clear();imgs.clear()
            for p in content or []:
                typ = p.get('type')
                if typ in ('input_text','output_text','text'): buf.append(p.get('text',''))
                elif typ in ('input_image','image_url'): imgs.append(image_url(p))
                elif typ == 'image':
                    source=p.get('source',{})
                    if source.get('type')!='base64': raise ValueError('Only embedded images are supported')
                    imgs.append(source['data'])
                elif typ == 'tool_use':
                    flush();names[p['id']]=p['name']
                    messages.append({'role':'assistant','content':'','tool_calls':[{'id':p['id'],'type':'function','function':{'name':p['name'],'arguments':p.get('input',{})}}]})
                elif typ == 'tool_result':
                    flush();tool_result(p['tool_use_id'],p.get('content',''))
                elif typ == 'thinking':
                    # Unmodified model reasoning for templates which require it.
                    flush();messages.append({'role':'assistant','content':'','thinking':p.get('thinking','')})
                elif typ not in ('redacted_thinking',): raise ValueError('Unsupported input block: '+str(typ))
            flush()
    # Templates expect one assistant message containing all parallel calls,
    # with its reasoning attached before the tool results.
    merged=[]
    for m in messages:
        if merged and merged[-1]['role']==m['role']=='assistant':
            prev=merged[-1];prev['content']+=m.get('content','')
            if m.get('thinking'):prev['thinking']=prev.get('thinking','')+m['thinking']
            if m.get('tool_calls'):prev.setdefault('tool_calls',[]).extend(m['tool_calls'])
        else:merged.append(m)
    # Local templates generally accept one leading system message. Preserve
    # all harness instructions, including later environment/developer updates,
    # while leaving the latest user request in the conversation's final position.
    system=[m['content'] for m in merged if m['role'] in ('system','developer')]
    conversation=[m for m in merged if m['role'] not in ('system','developer')]
    return ([{'role':'system','content':'\n\n'.join(system)}] if system else [])+conversation

def generation(body, route, anthropic=False):
    tools, custom = tools_for(body, anthropic)
    messages = messages_for(body, anthropic)
    # Harnesses often omit sampling controls because their cloud model supplies
    # them. Use deterministic coding by default; honor explicit harness values.
    sampling={key:body[key] for key in ('temperature','top_p','top_k') if key in body}
    sampling.setdefault('temperature',0)
    payload = {'model':route['model'],'messages':messages,'stream':True}
    if tools: payload['tools']=tools
    limit = body.get('max_tokens', body.get('max_output_tokens'))
    if route['engine']=='ollama':
        payload['options']={'num_ctx':route['context'],**sampling}
        payload['keep_alive']='2m'
        if body.get('stop_sequences'):payload['options']['stop']=body['stop_sequences']
        if limit: payload['options']['num_predict']=int(limit)
        if route.get('thinking') is not None:
            v=route['thinking'];payload['think']={'true':True,'false':False}.get(v,v)
        path='/api/chat'
    else:
        payload.update(sampling)
        for m in messages:
            for c in m.get('tool_calls',[]): c['function']['arguments']=dumps(c['function']['arguments'])
            if m.get('images'):
                m['content']=[{'type':'text','text':m['content']}]+[{'type':'image_url','image_url':{'url':'data:image/png;base64,'+i}} for i in m.pop('images')]
            if 'thinking' in m: m['reasoning_content']=m.pop('thinking')
        payload['stream_options']={'include_usage':True}
        if limit: payload['max_tokens']=int(limit)
        if route.get('thinking') is not None: payload['reasoning_effort']=route['thinking']
        if route.get('node'):payload['kai_context']=route['context']
        path='/v1/chat/completions'
    output_format=body.get('text',{}).get('format') or body.get('output_config',{}).get('format')
    if output_format and output_format.get('type')=='json_schema':
        if route['engine']=='ollama':payload['format']=output_format['schema']
        else:payload['response_format']={'type':'json_schema','schema':output_format['schema']}
    if route.get('node'): path='/infer/'+route['engine']+path
    return route['endpoint'].rstrip('/')+path, payload, custom

class Stream:
    def __init__(self, handler, anthropic):
        self.h=handler;self.a=anthropic;self.id=uid('msg_' if anthropic else 'resp_');self.items=[];self.seq=0;self.active={};self.calls={};self.text='';self.thinking='';self.usage={'input_tokens':0,'output_tokens':0}
    def event(self, typ, **kw):
        value={'type':typ,**kw}
        if not self.a: value['sequence_number']=self.seq;self.seq+=1
        self.h.wfile.write(('event: '+typ+'\ndata: '+dumps(value)+'\n\n').encode());self.h.wfile.flush()
    def response(self,status): return {'id':self.id,'object':'response','created_at':int(time.time()),'status':status,'error':None,'incomplete_details':None,'output':self.items,'usage':dict(self.usage,total_tokens=sum(self.usage.values()))}
    def start(self, model):
        if self.a: self.event('message_start',message={'id':self.id,'type':'message','role':'assistant','model':model,'content':[],'stop_reason':None,'stop_sequence':None,'usage':self.usage.copy()})
        else: self.event('response.created',response=self.response('in_progress'));self.event('response.in_progress',response=self.response('in_progress'))
    def add(self, key, item, block):
        ix=len(self.items);self.items.append(item);self.active[key]=ix
        if self.a: self.event('content_block_start',index=ix,content_block=block)
        else: self.event('response.output_item.added',output_index=ix,item=dict(item))
        return ix
    def delta(self, value, reasoning=False):
        if not value:return
        key='thinking' if reasoning else 'text'
        if key not in self.active:
            if reasoning:
                ix=self.add(key,{'id':uid('rs_'),'type':'reasoning','summary':[]},{'type':'thinking','thinking':''})
                if not self.a:self.event('response.reasoning_summary_part.added',item_id=self.items[ix]['id'],output_index=ix,summary_index=0,part={'type':'summary_text','text':''})
            else:
                ix=self.add(key,{'id':uid('msg_'),'type':'message','role':'assistant','status':'in_progress','content':[]},{'type':'text','text':''})
                if not self.a:self.event('response.content_part.added',item_id=self.items[ix]['id'],output_index=ix,content_index=0,part={'type':'output_text','text':'','annotations':[]})
        ix=self.active[key]
        if reasoning:self.thinking+=value
        else:self.text+=value
        if self.a:self.event('content_block_delta',index=ix,delta={'type':'thinking_delta' if reasoning else 'text_delta',key:value})
        else:self.event('response.reasoning_summary_text.delta' if reasoning else 'response.output_text.delta',item_id=self.items[ix]['id'],output_index=ix,**({'summary_index':0} if reasoning else {'content_index':0}),delta=value)
    def tool(self, key, name, args, custom=False, call_id=None):
        if key not in self.calls:
            call=call_id or uid('call_');kind='custom_tool_call' if custom else 'function_call'
            item={'id':uid('fc_'),'type':kind,'call_id':call,'name':name,'status':'in_progress',('input' if custom else 'arguments'):''}
            ix=self.add('call:'+str(key),item,{'type':'tool_use','id':call,'name':name,'input':{}})
            self.calls[key]=(ix,custom,'')
        ix,custom,old=self.calls[key];self.calls[key]=(ix,custom,old+args)
        if self.a:self.event('content_block_delta',index=ix,delta={'type':'input_json_delta','partial_json':args})
        elif args:self.event('response.custom_tool_call_input.delta' if custom else 'response.function_call_arguments.delta',item_id=self.items[ix]['id'],output_index=ix,delta=args)
    def finish(self):
        for ix,item in enumerate(self.items):
            if self.a:self.event('content_block_stop',index=ix);continue
            typ=item['type']
            if typ=='message':
                part={'type':'output_text','text':self.text,'annotations':[]};item['content']=[part];item['status']='completed'
                self.event('response.output_text.done',item_id=item['id'],output_index=ix,content_index=0,text=self.text)
                self.event('response.content_part.done',item_id=item['id'],output_index=ix,content_index=0,part=part)
            elif typ=='reasoning':
                part={'type':'summary_text','text':self.thinking};item['summary']=[part]
                self.event('response.reasoning_summary_text.done',item_id=item['id'],output_index=ix,summary_index=0,text=self.thinking)
                self.event('response.reasoning_summary_part.done',item_id=item['id'],output_index=ix,summary_index=0,part=part)
            else:
                _,custom,args=next(v for v in self.calls.values() if v[0]==ix)
                field='input' if custom else 'arguments';item[field]=args;item['status']='completed'
                self.event('response.custom_tool_call_input.done' if custom else 'response.function_call_arguments.done',item_id=item['id'],output_index=ix,**{field:args})
            self.event('response.output_item.done',output_index=ix,item=item)
        if self.a:
            self.event('message_delta',delta={'stop_reason':'tool_use' if self.calls else 'end_turn','stop_sequence':None},usage=self.usage);self.event('message_stop')
        else:self.event('response.completed',response=self.response('completed'))

class Server(http.server.ThreadingHTTPServer):
    daemon_threads=True;allow_reuse_address=False
    def __init__(self, args):
        self.args=args;self.slots=threading.BoundedSemaphore(8);self.owner_pid=os.getppid()
        super().__init__(('127.0.0.1',0),Handler)
    def service_actions(self):
        if os.getppid()!=self.owner_pid:raise SystemExit(0)

class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def log_message(self,*args): pass # Prompts, paths and tokens must never be logged.
    def json(self,status,value):
        data=dumps(value).encode();self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data)
    def do_POST(self):
        begun=False
        self.close_connection=True
        path=urllib.parse.urlsplit(self.path).path
        try:
            auth=self.headers.get('Authorization','').removeprefix('Bearer ') or self.headers.get('x-api-key','')
            if not hmac.compare_digest(auth,self.server.args.token):return self.json(401,{'error':{'message':'Invalid local Ninja credential','type':'authentication_error'}})
            parts=path.split('/')
            if len(parts)<5 or parts[1]!='route' or not all(c in '0123456789abcdef' for c in parts[2]) or len(parts[2])!=64: return self.json(404,{'error':'Unknown route'})
            route=json.loads((pathlib.Path(self.server.args.routes)/(parts[2]+'.json')).read_text())
            a=path.endswith('/messages')
            if not a and not path.endswith('/responses'):return self.json(404,{'error':'Unsupported inference path'})
            n=int(self.headers.get('Content-Length','0'))
            if n<=0 or n>MAX_BODY: return self.json(413,{'error':'Inference request exceeds 32 MiB'})
            body=json.loads(self.rfile.read(n))
            streaming=bool(body.get('stream',False))
            if not self.server.slots.acquire(timeout=120): return self.json(429,{'error':'Ninja inference queue timed out'})
            try:
                url,payload,custom=generation(body,route,a)
                req=urllib.request.Request(url,dumps(payload).encode(),{'Content-Type':'application/json'},method='POST')
                if streaming:
                    self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Cache-Control','no-cache');self.send_header('Connection','close');self.end_headers();self.close_connection=True;begun=True
                stream=Stream(self,a)
                if not streaming:stream.event=lambda *args,**kwargs:None
                stream.start(route['model']);calls={};done=False
                tool_text=QwenToolText(route['model'],payload.get('tools',[]),stream.delta)
                for line in inference_lines(OPENER,req):
                    if not line.strip() or line.startswith(b':'):
                        if streaming:self.wfile.write(b': inference keepalive\n\n');self.wfile.flush()
                        continue
                    if len(line)>MAX_LINE:raise ValueError('Inference event exceeds 8 MiB')
                    if route['engine']=='ollama':
                        v=json.loads(line);m=v.get('message',{})
                        if v.get('error'):raise ValueError(str(v['error']))
                        stream.delta(m.get('thinking',''),True);tool_text.feed(m.get('content',''))
                        for i,c in enumerate(m.get('tool_calls',[])):
                            f=c['function'];name=f['name'];args=f.get('arguments',{})
                            if isinstance(args,str):args=json.loads(args)
                            stream.tool(len(stream.calls),name,str(args.get('input','')) if name in custom and not a else dumps(args),name in custom and not a,c.get('id'))
                        if v.get('done'):
                            stream.usage={'input_tokens':v.get('prompt_eval_count',0),'output_tokens':v.get('eval_count',0)};done=True
                    else:
                        if not line.startswith(b'data:'):continue
                        raw=line[5:].strip()
                        if raw==b'[DONE]':done=True;break
                        v=json.loads(raw)
                        if v.get('error'):raise ValueError(str(v['error']))
                        if v.get('usage'):stream.usage={'input_tokens':v['usage'].get('prompt_tokens',0),'output_tokens':v['usage'].get('completion_tokens',0)}
                        for choice in v.get('choices',[]):
                            d=choice.get('delta',{});stream.delta(d.get('reasoning_content') or d.get('reasoning') or '',True);tool_text.feed(d.get('content') or '')
                            for c in d.get('tool_calls',[]):
                                i=c.get('index',0);old=calls.setdefault(i,{'name':'','args':'','id':None})
                                old['name']+=c.get('function',{}).get('name','');old['args']+=c.get('function',{}).get('arguments','');old['id']=c.get('id') or old['id']
                            if choice.get('finish_reason'):done=True
                if not done: raise ValueError('Inference stream ended without a completion event')
                for name,args in tool_text.finish(bool(stream.calls or calls)):
                    is_custom=name in custom and not a
                    stream.tool(len(stream.calls),name,args.get('input','') if is_custom else dumps(args),is_custom)
                for i,c in calls.items():
                    args=c['args'];is_custom=c['name'] in custom and not a
                    if is_custom:args=json.loads(args).get('input','')
                    stream.tool(i,c['name'],args,is_custom,c['id'])
                stream.finish()
                if not streaming:
                    if a:
                        blocks=[]
                        for ix,item in enumerate(stream.items):
                            if item['type']=='reasoning':blocks.append({'type':'thinking','thinking':stream.thinking})
                            elif item['type']=='message':blocks.append({'type':'text','text':stream.text})
                            else:
                                _,_,args=next(v for v in stream.calls.values() if v[0]==ix)
                                blocks.append({'type':'tool_use','id':item['call_id'],'name':item['name'],'input':json.loads(args)})
                        self.json(200,{'id':stream.id,'type':'message','role':'assistant','model':route['model'],'content':blocks,'stop_reason':'tool_use' if stream.calls else 'end_turn','stop_sequence':None,'usage':stream.usage})
                    else:self.json(200,stream.response('completed'))
            finally:self.server.slots.release()
        except (BrokenPipeError,ConnectionResetError):pass
        except urllib.error.HTTPError as e:
            try:
                value=json.loads(e.read(8192));error=value.get('error',value);message=error.get('message',str(error)) if isinstance(error,dict) else str(error)
            except Exception:message='Inference server rejected the request'
            with contextlib.suppress(Exception):
                if begun:
                    if a:stream.event('error',error={'type':'api_error','message':message[:2000]})
                    else:stream.event('response.failed',response=dict(stream.response('failed'),error={'code':'inference_error','message':message[:2000]}))
                else:self.json(e.code,{'error':{'type':'api_error','message':message[:2000]}})
        except Exception as e:
            # Safe messages only: never echo a request, token or cloud response.
            message='Ninja inference failed: '+(str(e) if isinstance(e,(ValueError,urllib.error.URLError)) else type(e).__name__)
            if begun:
                with contextlib.suppress(Exception):
                    if a:stream.event('error',error={'type':'api_error','message':message})
                    else:stream.event('response.failed',response=dict(stream.response('failed'),error={'code':'server_error','message':message}))
            else:
                with contextlib.suppress(Exception):self.json(502,{'error':{'type':'api_error','message':message}})

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--routes',required=True);p.add_argument('--ready',required=True);p.add_argument('--token',required=True);args=p.parse_args()
    # The token is passed through a private stdin pipe, never the process list.
    if args.token=='stdin':args.token=input().strip()
    server=Server(args)
    ready=pathlib.Path(args.ready);ready.write_text(str(server.server_port));ready.chmod(0o600)
    try:server.serve_forever(poll_interval=1)
    finally:server.server_close()

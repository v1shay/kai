#!/usr/bin/env python3
"""Private Ninja inference host with an optional explicit native-agent worker.
Run behind Tailscale Serve, bound to loopback; authorize its verified login header.
Only explicit model downloads/engine selections start processes owned by this host.
"""
import argparse, contextlib, http.server, json, os, pathlib, re, shutil, socket, ipaddress
import subprocess, threading, time, urllib.request, urllib.error, signal, hashlib
from kai_inference_stream import lines as inference_lines
OPENER=urllib.request.build_opener(urllib.request.ProxyHandler({}))
LIMIT=32*1024*1024
MODEL=re.compile(r'^[A-Za-z0-9][A-Za-z0-9_./:-]{0,255}$')

def request(url,body=None,timeout=4):
    return OPENER.open(urllib.request.Request(url,None if body is None else json.dumps(body).encode(),{'Content-Type':'application/json'}),timeout=timeout)
def read_json(url,body=None,timeout=4):
    with request(url,body,timeout) as r:
        data=r.read(8*1024*1024+1)
        if len(data)>8*1024*1024:raise ValueError('Metadata size limit')
        return json.loads(data)
def available(name):
    return shutil.which(name) or next((str(p/name) for p in [pathlib.Path('/opt/homebrew/bin'),pathlib.Path('/usr/local/bin'),pathlib.Path('/Applications/Ollama.app/Contents/Resources')] if (p/name).is_file()),None)

class Engines:
    def __init__(self,root):self.lock=threading.RLock();self.ollama_lock=threading.RLock();self.llama_request_lock=threading.RLock();self.ollama=None;self.llama=None;self.llama_model=None;self.llama_port=None;self.root=root;self.active=0;self.inference=threading.BoundedSemaphore(1);self.llama_last_use=time.monotonic()
    def close(self):
        for child in (self.ollama,self.llama):
            if child and child.poll() is None:
                child.terminate()
                try:child.wait(3)
                except subprocess.TimeoutExpired:child.kill();child.wait()
    def ollama_url(self):
        with self.ollama_lock:
            try:read_json('http://127.0.0.1:11434/api/version');return 'http://127.0.0.1:11434'
            except Exception:pass
            cli=available('ollama')
            if not cli:raise ValueError('Install Ollama on this inference machine first')
            if self.ollama is None or self.ollama.poll() is not None:
                env=os.environ.copy();env['OLLAMA_HOST']='127.0.0.1:11434';env['OLLAMA_NO_CLOUD']='1';env['OLLAMA_NUM_PARALLEL']='1';env['OLLAMA_MAX_LOADED_MODELS']='1';env['OLLAMA_KEEP_ALIVE']='2m'
                self.ollama=subprocess.Popen([cli,'serve'],env=env,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            until=time.monotonic()+12
            while time.monotonic()<until:
                try:read_json('http://127.0.0.1:11434/api/version');return 'http://127.0.0.1:11434'
                except Exception:time.sleep(.1)
            raise ValueError('Ollama did not become ready')
    def blob(self,model):
        if not MODEL.fullmatch(model) or '..' in model or model.endswith((':cloud','-cloud')):raise ValueError('Select a local GGUF model')
        name,sep,tag=model.rpartition(':')
        if not sep:name,tag=model,'latest'
        parts=name.split('/')
        if len(parts)==1:parts=['library']+parts
        if len(parts)!=2:raise ValueError('Only installed Ollama registry GGUF models can be shared with llama.cpp')
        root=pathlib.Path(os.environ.get('OLLAMA_MODELS',str(pathlib.Path.home()/'.ollama/models'))).resolve()
        manifest=root/'manifests/registry.ollama.ai'/parts[0]/parts[1]/tag
        meta=json.loads(manifest.read_text())
        layer=next((l for l in meta.get('layers',[]) if l.get('mediaType')=='application/vnd.ollama.image.model'),None)
        digest=(layer or {}).get('digest','')
        if not re.fullmatch(r'sha256:[a-f0-9]{64}',digest):raise ValueError('This model has no supported GGUF model layer')
        file=root/'blobs'/digest.replace(':','-')
        with file.open('rb') as f:
            if f.read(4)!=b'GGUF':raise ValueError('The selected model is not GGUF')
        return file
    def llama_url(self,model,context):
        with self.llama_request_lock:
            self.llama_last_use=time.monotonic()
            if self.llama and self.llama.poll() is None and self.llama_model and self.llama_model[0]==model and self.llama_model[1]>=context:return 'http://127.0.0.1:'+str(self.llama_port)
            cli=available('llama-server')
            if not cli:raise ValueError('Install llama.cpp on this inference machine first')
            file=self.blob(model)
            if self.llama and self.llama.poll() is None:self.llama.terminate();self.llama.wait(8)
            with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
            self.llama=subprocess.Popen([cli,'--model',str(file),'--alias',model,'--host','127.0.0.1','--port',str(port),'--jinja','--ctx-size',str(context),'--parallel','1'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            self.llama_model=(model,context);self.llama_port=port;until=time.monotonic()+180
            while time.monotonic()<until:
                if self.llama.poll() is not None:raise ValueError('llama.cpp exited while loading this model')
                try:read_json('http://127.0.0.1:'+str(port)+'/health');return 'http://127.0.0.1:'+str(port)
                except Exception:time.sleep(.2)
            raise ValueError('llama.cpp model loading timed out')

class Server(http.server.ThreadingHTTPServer):
    daemon_threads=True
    def __init__(self,args):
        self.args=args;self.engines=Engines(pathlib.Path.home()/'.kai-ninja');self.slots=threading.BoundedSemaphore(8);self.identities={};self.identity_lock=threading.Lock()
        super().__init__((getattr(args,'bind','127.0.0.1'),args.port),Handler)
    def service_actions(self):
        if not self.engines.llama_request_lock.acquire(False):return
        try:
            if self.engines.llama and time.monotonic()-self.engines.llama_last_use>120:
                child=self.engines.llama;self.engines.llama=None;self.engines.llama_model=None
                def retire():
                    if child.poll() is None:
                        child.terminate()
                        try:child.wait(3)
                        except subprocess.TimeoutExpired:child.kill();child.wait()
                threading.Thread(target=retire,daemon=True).start()
        finally:self.engines.llama_request_lock.release()
    def worker_call(self,body):
        worker=getattr(self.args,'worker',None)
        if not worker or not pathlib.Path(worker).is_file():raise ValueError('Install the persistent Ninja worker on this machine first')
        root=pathlib.Path.home()/'Library/Application Support/Kai Unified/ninjas'
        digest=hashlib.sha256(str(root).encode()).hexdigest()[:10]
        address='/private/tmp/kai-ninjas-'+str(os.getuid())+'-'+digest+'.sock'
        with self.engines.lock:
            def reachable():
                try:
                    with socket.socket(socket.AF_UNIX) as s:s.connect(address)
                    return True
                except OSError:return False
            if not reachable():
                args=[worker,'--ninja-worker','--data',str(root)]
                if getattr(self.args,'keep_reachable',False):args.append('--keep-reachable')
                subprocess.Popen(args,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
                until=time.monotonic()+6
                while not reachable() and time.monotonic()<until:time.sleep(.03)
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(125 if body.get('action')=='import_handoff' else 35);client.connect(address);client.sendall(json.dumps(body).encode()+b'\n')
            with client.makefile('rb') as file:line=file.readline(8*1024*1024+1)
        if len(line)>8*1024*1024:raise ValueError('Worker reply exceeds 8 MiB')
        return json.loads(line)
    def authorized(self,address,headers):
        if getattr(self.args,'identity','serve')=='serve':return headers.get('Tailscale-User-Login','').casefold()==self.args.owner.casefold()
        try:
            ip=ipaddress.ip_address(address)
            if ip not in ipaddress.ip_network('100.64.0.0/10') and ip not in ipaddress.ip_network('fd7a:115c:a1e0::/48'):return False
            with self.identity_lock:
                cached=self.identities.get(address)
                if cached and time.monotonic()-cached[0]<30:return cached[1]
            cli=getattr(self.args,'tailscale',None) or available('tailscale')
            profile=json.loads(subprocess.run([cli,'whois','--json',address],capture_output=True,timeout=4,check=True).stdout)
            authorized=profile.get('UserProfile',{}).get('LoginName','').casefold()==self.args.owner.casefold()
            with self.identity_lock:
                if len(self.identities)>256:self.identities.clear()
                self.identities[address]=(time.monotonic(),authorized)
            return authorized
        except Exception:return False
class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def log_message(self,*args):pass
    def result(self,status,data):
        b=json.dumps(data).encode();self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(b)));self.end_headers();self.wfile.write(b)
    def do_GET(self):self.handle_request(False)
    def do_POST(self):self.handle_request(True)
    def handle_request(self,post):
        # Serve removes incoming identity headers and injects the verified user.
        if not self.server.authorized(self.client_address[0],self.headers):return self.result(403,{'error':'This inference host is restricted to its configured Tailscale owner'})
        if self.headers.get('Origin'):return self.result(403,{'error':'Browser requests are not accepted'})
        path=self.path.removeprefix('/ninja');begun=False;llama_reserved=False;inference_reserved=False
        try:
            if '?' in path:raise ValueError('Query parameters are not accepted')
            n=int(self.headers.get('Content-Length','0'))
            if n<0 or n>LIMIT:return self.result(413,{'error':'Request too large'})
            body=json.loads(self.rfile.read(n)) if post and n else None
            if path=='/agents':
                return self.result(200,self.server.worker_call(body or {'action':'snapshot'}))
            if path.startswith('/infer/llama_cpp/') or (path=='/model_info' and (body or {}).get('engine')=='llama_cpp'):
                if not self.server.engines.llama_request_lock.acquire(timeout=180):raise ValueError('llama.cpp inference queue timed out')
                llama_reserved=True
            if path=='/status' and not post:return self.result(200,{'version':1,'active':self.server.engines.active,'worker':bool(getattr(self.server.args,'worker',None)),'engines':{'ollama':bool(available('ollama')),'llama_cpp':bool(available('llama-server'))}})
            if path in ('/api/tags','/api/version') and not post:url=self.server.engines.ollama_url()+path
            elif path in ('/api/show','/api/pull') and post:
                if not MODEL.fullmatch((body or {}).get('model','')) or '..' in body['model'] or body['model'].endswith((':cloud','-cloud')):raise ValueError('Choose a local model tag')
                url=self.server.engines.ollama_url()+path
            elif path=='/model_info' and post:
                model=body['model'];ctx=int(body.get('context',65536))
                if not 2048<=ctx<=2097152:raise ValueError('Invalid context window')
                if body['engine']=='llama_cpp':return self.result(200,read_json(self.server.engines.llama_url(model,ctx)+'/props'))
                return self.result(200,read_json(self.server.engines.ollama_url()+'/api/show',{'model':model}))
            elif path=='/infer/ollama/api/chat' and post:
                if not MODEL.fullmatch(body.get('model','')) or body['model'].endswith((':cloud','-cloud')):raise ValueError('Cloud models are not a Ninja inference target')
                url=self.server.engines.ollama_url()+'/api/chat'
            elif path=='/infer/llama_cpp/v1/chat/completions' and post:
                ctx=int(body.pop('kai_context',65536))
                if not 2048<=ctx<=2097152:raise ValueError('Invalid context window')
                url=self.server.engines.llama_url(body['model'],ctx)+'/v1/chat/completions'
            else:return self.result(404,{'error':'No such inference operation'})
            if not self.server.slots.acquire(timeout=30):return self.result(429,{'error':'Inference host queue timed out'})
            try:
                with self.server.engines.lock:self.server.engines.active+=1
                if path.startswith('/infer/'):
                    self.send_response(200);self.send_header('Content-Type','text/event-stream' if '/llama_cpp/' in path else 'application/x-ndjson');self.send_header('Connection','close');self.end_headers();self.close_connection=True;begun=True
                    until=time.monotonic()+900
                    while not self.server.engines.inference.acquire(timeout=10):
                        if time.monotonic()>until:raise TimeoutError('Inference queue exceeded fifteen minutes')
                        self.wfile.write(b'\n');self.wfile.flush()
                    inference_reserved=True
                    req=urllib.request.Request(url,json.dumps(body).encode(),{'Content-Type':'application/json'})
                    for line in inference_lines(OPENER,req):self.wfile.write(line);self.wfile.flush()
                else:
                    with request(url,body,3600 if path=='/api/pull' else 10) as upstream:
                        self.send_response(200);self.send_header('Content-Type',upstream.headers.get('Content-Type','application/json'));self.send_header('Connection','close');self.end_headers();self.close_connection=True;begun=True
                        while True:
                            line=upstream.readline(8*1024*1024+1)
                            if not line:break
                            if len(line)>8*1024*1024:raise ValueError('Event too large')
                            self.wfile.write(line);self.wfile.flush()
            finally:
                if inference_reserved:self.server.engines.inference.release();inference_reserved=False
                with self.server.engines.lock:
                    self.server.engines.active-=1
                    if path.startswith('/infer/llama_cpp/'):self.server.engines.llama_last_use=time.monotonic()
                self.server.slots.release()
        except (BrokenPipeError,ConnectionResetError):pass
        except urllib.error.HTTPError as e:
            if begun:
                with contextlib.suppress(Exception):self.wfile.write((('data: ' if '/llama_cpp/' in path else '')+json.dumps({'error':'Inference engine rejected request ('+str(e.code)+')'})+'\n\n').encode());self.wfile.flush()
            else:
                try:value=json.loads(e.read(8192))
                except Exception:value={'error':'Inference engine rejected the request'}
                with contextlib.suppress(Exception):self.result(e.code,value)
        except Exception as e:
            if begun:
                with contextlib.suppress(Exception):self.wfile.write((('data: ' if '/llama_cpp/' in path else '')+json.dumps({'error':str(e) if isinstance(e,ValueError) else type(e).__name__})+'\n\n').encode());self.wfile.flush()
            else:
                with contextlib.suppress(Exception):self.result(502,{'error':str(e) if isinstance(e,(ValueError,urllib.error.URLError)) else type(e).__name__})
        finally:
            if llama_reserved:self.server.engines.llama_request_lock.release()

def install(args):
    """Explicit host-local setup. Preserve other Serve paths and never use Funnel."""
    if args.worker and not pathlib.Path(args.worker).is_file():raise SystemExit('Choose an installed Ninja worker executable')
    root=pathlib.Path.home()/'.kai-ninja';root.mkdir(mode=0o700,exist_ok=True)
    target=root/'kai_ninja_host.py'
    if pathlib.Path(__file__).resolve()!=target.resolve():shutil.copyfile(__file__,target)
    target.chmod(0o700)
    helper=pathlib.Path(__file__).with_name('kai_inference_stream.py')
    if helper.resolve()!=(root/helper.name).resolve():shutil.copyfile(helper,root/helper.name)
    cli=available('tailscale')
    if not cli:raise SystemExit('Install and sign in to Tailscale first')
    status=json.loads(subprocess.run([cli,'status','--json'],capture_output=True,timeout=5,check=True).stdout)
    address=next((ip for ip in status['Self'].get('TailscaleIPs',[]) if ':' not in ip),None)
    if not address:raise SystemExit('Connect Tailscale first')
    bind='127.0.0.1' if args.serve else address;identity='serve' if args.serve else 'whois'
    cfg=root/'host.json';cfg.write_text(json.dumps({'owner':args.owner,'port':args.port,'bind':bind,'identity':identity,'worker':args.worker,'keep_reachable':args.keep_reachable}));cfg.chmod(0o600)
    if sys_platform()=='darwin':
        import plistlib
        folder=pathlib.Path.home()/'Library/LaunchAgents';folder.mkdir(parents=True,exist_ok=True)
        plist=folder/'com.kai.ninja.inference.plist'
        worker_args=(['--worker',str(pathlib.Path(args.worker).resolve())] if args.worker else [])+(['--keep-reachable'] if args.keep_reachable else [])
        plist.write_bytes(plistlib.dumps({'Label':'com.kai.ninja.inference','ProgramArguments':['/usr/bin/python3',str(target),'--owner',args.owner,'--port',str(args.port),'--bind',bind,'--identity',identity,'--tailscale',cli]+worker_args,'RunAtLoad':True,'KeepAlive':True,'EnvironmentVariables':{'PATH':os.environ.get('PATH','')+':/opt/homebrew/bin:/usr/local/bin'},'ThrottleInterval':10}))
        subprocess.run(['launchctl','bootout','gui/'+str(os.getuid()),str(plist)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        subprocess.run(['launchctl','bootstrap','gui/'+str(os.getuid()),str(plist)],check=True)
    else:
        raise SystemExit('Host service installation currently supports macOS; run this script directly on Linux')
    if args.serve:
        subprocess.run([cli,'serve','--bg','--set-path=/ninja','http://127.0.0.1:'+str(args.port)],check=True,timeout=20)
    else:print('Private Tailscale inference endpoint: http://'+address+':'+str(args.port))
    print('Private Ninja inference host installed'+(' with native task execution.' if args.worker else '.'))
def sys_platform():import sys;return sys.platform
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--worker');p.add_argument('--keep-reachable',action='store_true');p.add_argument('--owner',required=True,help='Your Tailscale login email');p.add_argument('--port',type=int,default=32187);p.add_argument('--install',action='store_true');p.add_argument('--serve',action='store_true');p.add_argument('--bind',default='127.0.0.1');p.add_argument('--identity',choices=['serve','whois'],default='serve');p.add_argument('--tailscale');args=p.parse_args()
    if not args.owner or len(args.owner)>254 or any(c.isspace() for c in args.owner):p.error('Invalid Tailscale owner login')
    if args.install:install(args)
    else:
        server=Server(args)
        def terminate(*_):raise SystemExit(0)
        signal.signal(signal.SIGTERM,terminate)
        try:server.serve_forever(poll_interval=1)
        finally:server.engines.close();server.server_close()

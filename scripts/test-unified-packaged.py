#!/usr/bin/env python3
"""Run an isolated signed bundle through its purpose-built native probes."""
import argparse,base64,contextlib,http.server,json,os,plistlib,re,shutil,signal,struct,subprocess,tempfile,threading,time,zlib
from pathlib import Path


@contextlib.contextmanager
def image_fixture(env,enabled):
    """Exercise the real host download queue without touching a user worker."""
    downloaded=set()
    if not enabled:
        yield downloaded
        return
    def chunk(kind,data):
        return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
    pixels=b''.join(b'\0'+bytes([47,133+y//3,184,255])*320 for y in range(100))
    png=b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',320,100,8,6,0,0,0))+chunk(b'IDAT',zlib.compress(pixels))+chunk(b'IEND',b'')
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self,*args):pass
        def do_POST(self):
            length=int(self.headers.get('Content-Length','0'))
            if self.path!='/agents' or not 0<length<=4096:
                self.send_error(400);return
            request=json.loads(self.rfile.read(length));path=request.get('path','')
            if request.get('action')!='artifact' or not re.fullmatch(r'/fixture/preview-[0-7]\.(svg|png)',path) or request.get('offset')!=0:
                self.send_error(400);return
            data=png if path.endswith('.png') else b'<svg xmlns="http://www.w3.org/2000/svg" width="320" height="100"><rect width="320" height="100" rx="16" fill="#23576e"/><circle cx="50" cy="50" r="24" fill="#79d4df"/><text x="92" y="57" font-family="sans-serif" font-size="18" fill="white">inline svg preview</text></svg>'
            downloaded.add(path)
            body=json.dumps({'total_bytes':len(data),'offset':0,'next_offset':None,'base64':base64.b64encode(data).decode()}).encode()
            self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
    server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
    env['KAI_NINJA_IMAGE_PROBE_ENDPOINT']='http://127.0.0.1:'+str(server.server_port)
    thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
    try:yield downloaded
    finally:server.shutdown();server.server_close();thread.join()


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('mode',choices=['ninja-chat','menu-bar','wallpaper','glass','startup-workspace','startup-extended','startup-compact','performance','latency','background-idle'])
    parser.add_argument('--bundle',type=Path,help='Verify another signed bundle, including an installed baseline')
    args=parser.parse_args();mode=args.mode
    root=Path(__file__).resolve().parent.parent
    source=args.bundle or root/'.build/unified/Kai Unified.app'
    report=root/'.build/validation';report.mkdir(parents=True,exist_ok=True)
    log=report/(mode+'.log')
    env=os.environ.copy();env['KAI_PERFORMANCE_LOG']='1'
    flags=['--unified-smoke-test']
    if mode=='ninja-chat':
        flags+=['--unified-ninja-render-probe'];marker='KAI_NINJA_RENDER_PROBE_OK';env['KAI_STARTUP_SURFACE']='workspace'
    elif mode=='menu-bar':
        flags+=['--unified-menu-bar-probe'];marker='KAI_MENU_BAR_PROBE_OK';env['KAI_STARTUP_SURFACE']='workspace'
    elif mode=='latency':
        flags+=['--unified-latency-probe'];marker='KAI_LATENCY_PROBE_OK';env['KAI_STARTUP_SURFACE']='workspace'
    elif mode=='background-idle':
        # A visible extended chat is foreground UI work. Measure the actual
        # compact native surface with the Code window hidden for this budget.
        flags+=['--unified-startup-probe'];marker='KAI_STARTUP_PROBE_OK';env['KAI_STARTUP_SURFACE']='compact';env['KAI_STARTUP_WAIT_MS']='30000'
    elif mode in {'glass','wallpaper'}:
        flags+=['--unified-glass-probe' if mode=='glass' else '--unified-wallpaper-probe'];marker='Kai unified smoke: continuous morph reversal'
        if mode=='wallpaper':env['KAI_VALIDATION_BACKGROUND_SOURCE']=str(root/'integrations/Code/assets/wallpapers/bonsai-sakura.png')
    else:
        flags+=['--unified-startup-probe'];marker='KAI_STARTUP_PROBE_OK'
        if mode!='performance':env['KAI_STARTUP_SURFACE']=mode.removeprefix('startup-')
        else:env['KAI_STARTUP_WAIT_MS']='30000'
    samples=[];sampled=False
    with image_fixture(env,mode=='ninja-chat') as downloads,tempfile.TemporaryDirectory(prefix='kai-packaged-'+mode+'-') as scratch:
        app=Path(scratch)/'Kai Validation.app'
        subprocess.run(['ditto','--norsrc','--noextattr',str(source),str(app)],check=True)
        info_path=app/'Contents/Info.plist';info=plistlib.loads(info_path.read_bytes())
        executable='kai-validation-'+mode
        (app/'Contents/MacOS/kai-unified').rename(app/'Contents/MacOS'/executable)
        if env.get('KAI_VALIDATION_EXECUTABLE_PATH'):shutil.copy2(env['KAI_VALIDATION_EXECUTABLE_PATH'],app/'Contents/MacOS'/executable)
        info.update(CFBundleIdentifier='com.kai.validation.'+mode,CFBundleName='Kai Validation',CFBundleExecutable=executable)
        info_path.write_bytes(plistlib.dumps(info))
        subprocess.run(['codesign','--force','--sign','-',str(app)],check=True,stdout=subprocess.DEVNULL)
        subprocess.run(['codesign','--verify','--deep','--strict',str(app)],check=True)
        launched=time.monotonic()
        env['KAI_LAUNCH_ORIGIN_UNIX_NS']=str(time.time_ns())
        with log.open('w') as output:
            command=[str(app/'Contents/MacOS'/executable),*flags]
            if env.get('KAI_VALIDATION_FIXTURE') or env.get('KAI_VALIDATION_BACKGROUND_SOURCE'):
                prepare="""import os,sys,json,tempfile,shutil
from pathlib import Path
p=Path(tempfile.gettempdir())/('kai-unified-smoke-'+str(os.getpid()));p.mkdir(exist_ok=True)
(p/'workspace.json').write_text(os.environ.get('KAI_VALIDATION_FIXTURE','{}'))
if os.environ.get('KAI_VALIDATION_BACKGROUND_SOURCE'):
 b=p/'backgrounds';b.mkdir(exist_ok=True);source=b/'probe.png';shutil.copy2(os.environ['KAI_VALIDATION_BACKGROUND_SOURCE'],source)
 (b/'global.json').write_text(json.dumps({'path':str(source),'effect':'dither','everywhere':True}))
os.execve(sys.argv[1],sys.argv[1:],os.environ)
"""
                command=['/usr/bin/python3','-c',prepare,*command]
            process=subprocess.Popen(command,env=env,stdout=output,stderr=subprocess.STDOUT,start_new_session=True)
            try:
                while process.poll() is None:
                    elapsed=time.monotonic()-launched
                    if elapsed>65:raise TimeoutError('Packaged probe timed out')
                    if mode in {'performance','background-idle'} and 8<=elapsed<=27:
                        stat=subprocess.run(['ps','-o','pcpu=,rss=,time=','-p',str(process.pid)],capture_output=True,text=True)
                        threads=subprocess.run(['ps','-M','-p',str(process.pid)],capture_output=True,text=True)
                        fields=stat.stdout.split()
                        if len(fields)==3:
                            parts=[float(n) for n in fields[2].split(':')];cpu_seconds=sum(n*(60**i) for i,n in enumerate(reversed(parts)))
                            samples.append({'seconds':round(elapsed,2),'cpu_percent':float(fields[0]),'cpu_seconds':cpu_seconds,'rss_kib':int(fields[1]),'threads':max(0,len(threads.stdout.splitlines())-1)})
                        if mode in {'performance','background-idle'} and elapsed>=14 and not sampled:
                            sampled=True
                            subprocess.run(['sample',str(process.pid),'3','-file',str(report/(mode+'.sample.txt'))],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
                    time.sleep(1 if mode in {'performance','background-idle'} else 0.2)
            finally:
                if process.poll() is None:
                    os.killpg(process.pid,signal.SIGTERM)
                    try:process.wait(timeout=5)
                    except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
        text=log.read_text(errors='replace')
        # Retire only this probe's PID-scoped data even when it was interrupted.
        data=Path(tempfile.gettempdir())/f'kai-unified-smoke-{process.pid}'
        if data.is_dir():shutil.rmtree(data)
        if process.returncode or marker not in text:raise RuntimeError(f'{mode}: failed ({process.returncode}); {log}')
        timings={name:float(ms) for name,ms in re.findall(r'KAI_LAUNCH (\S+) ([0-9.]+)ms',text)}
        if mode in {'performance','background-idle'} and len(samples)<6:raise RuntimeError('Insufficient process samples; run the isolated probe with process-inspection access')
        native_commands=re.findall(r'KAI_NATIVE_COMMAND (\S+)',text)
        interactions=[name for name in native_commands if name not in {'pet_visuals','workspace_presentation','workspace_wallpaper_visibility'}]
        result={'mode':mode,'version':info.get('CFBundleShortVersionString'),'elapsed_seconds':round(time.monotonic()-launched,2),'launch_milliseconds':timings,'samples':samples,'native_commands':native_commands,'idle_interactions':interactions,'metrics':{name:[float(ms) for key,ms in re.findall(r'KAI_METRIC (\S+) ([0-9.]+)',text) if key==name] for name in ['warm_chat_switch','cold_chat_switch','warm_reveal','root_render','ninja_frame']},'log':str(log)}
        if samples:
            result['idle_average']={'cpu_percent_from_time_delta':100*(samples[-1]['cpu_seconds']-samples[0]['cpu_seconds'])/(samples[-1]['seconds']-samples[0]['seconds']),'cpu_percent':sum(s['cpu_percent'] for s in samples)/len(samples),'rss_mib':sum(s['rss_kib'] for s in samples)/len(samples)/1024,'threads':sum(s['threads'] for s in samples)/len(samples)}
        if mode=='ninja-chat':
            loaded=re.search(r'KAI_NINJA_IMAGE_PREVIEWS (\d+)',text)
            assert loaded and int(loaded[1])==8 and len(downloads)==8,'Image queue did not finish all eight previews'
            result['inline_previews_loaded']=int(loaded[1])
            frames=sorted(result['metrics']['ninja_frame']);assert len(frames)>=100,'Insufficient real-window frame samples'
            result['frame_p95_ms']=frames[int((len(frames)-1)*.95)]
            result['frame_median_ms']=frames[len(frames)//2]
            result['smooth_frame_budget_passed']=result['frame_p95_ms']<34
        (report/(mode+'.json')).write_text(json.dumps(result,indent=2))
        if mode=='ninja-chat' and not result['smooth_frame_budget_passed']:
            raise RuntimeError(f'Ninja frame p95 exceeded 34 ms; evidence saved to {report/(mode+".json")}')
        if mode in {'performance','background-idle'} and interactions:
            raise RuntimeError(f'Idle measurement received native input ({interactions}); evidence saved, but this run cannot certify idle CPU')
        print(json.dumps({k:v for k,v in result.items() if k!='samples'},indent=2))
        if samples:print('Idle sample averages:',json.dumps(result['idle_average']))
if __name__=='__main__':main()

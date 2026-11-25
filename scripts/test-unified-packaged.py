#!/usr/bin/env python3
"""Run an isolated signed bundle through its purpose-built native probes."""
import argparse,base64,contextlib,ctypes,http.server,json,os,plistlib,re,shutil,signal,struct,subprocess,tempfile,threading,time,zlib
from pathlib import Path


# Use only keyboard, pointer, and scroll events. The aggregate Quartz event
# counter also sees application/system events and is unsuitable for idle checks.
def hid_idle_clock():
    quartz=ctypes.CDLL('/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics')
    seconds=quartz.CGEventSourceSecondsSinceLastEventType
    seconds.argtypes=(ctypes.c_int32,ctypes.c_uint32)
    seconds.restype=ctypes.c_double
    event_types=(1,2,3,4,5,6,7,10,11,12,22,23,24,25,26,27)
    return lambda: min(seconds(1,event) for event in event_types)

def process_usage_clock():
    # RUSAGE_INFO_V0 reports cumulative process CPU in nanoseconds.
    class Usage(ctypes.Structure):
        _fields_ = [('uuid', ctypes.c_uint8 * 16)] + [
            (name, ctypes.c_uint64) for name in (
                'user', 'system', 'wakeups', 'interrupt_wakeups', 'pageins',
                'wired', 'resident', 'footprint', 'started', 'exited'
            )
        ]
    libproc=ctypes.CDLL('/usr/lib/libproc.dylib')
    rusage=libproc.proc_pid_rusage
    rusage.argtypes=(ctypes.c_int,ctypes.c_int,ctypes.POINTER(Usage))
    rusage.restype=ctypes.c_int
    def read(pid):
        usage=Usage()
        if rusage(pid,0,ctypes.byref(usage))!=0:
            return None
        return (usage.user+usage.system)/1e9,usage.resident//1024,usage.wakeups
    return read


def verified_idle(samples):
    start=longest_start=longest_end=0
    for index in range(1,len(samples)):
        previous,current=samples[index-1:index+1]
        elapsed=current['seconds']-previous['seconds']
        # Require one continuous quiet interval. Leave one second after
        # input for the renderer's input-related presentation tail.
        if current['hid_idle_seconds']<elapsed+1.05:
            start=index
        if current['seconds']-samples[start]['seconds'] > samples[longest_end]['seconds']-samples[longest_start]['seconds']:
            longest_start,longest_end=start,index
    first,last=samples[longest_start],samples[longest_end]
    duration=last['seconds']-first['seconds']
    return {'seconds':round(duration,2),
            'start_seconds':first['seconds'],
            'end_seconds':last['seconds'],
            'wakeups_per_second':(last['wakeups']-first['wakeups'])/duration if duration>=20 else None,
            'cpu_percent_from_time_delta':100*(last['cpu_seconds']-first['cpu_seconds'])/duration if duration>=20 else None}


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
    parser.add_argument('mode',choices=['discovery','surfaces','ninja-chat','menu-bar','chat-split','wallpaper','glass','startup-workspace','startup-extended','startup-compact','performance','latency','background-idle'])
    parser.add_argument('--idle-seconds',type=int,default=30,help='Idle probe duration from 30 to 60 seconds')
    parser.add_argument('--native-history',action='store_true',help='Use long imported Codex chats in the latency probe')
    parser.add_argument('--report-dir',type=Path)
    parser.add_argument('--sample-stacks',action='store_true',help='Sample stacks in a separate diagnostic run')
    parser.add_argument('--bundle',type=Path,help='Verify another signed bundle, including an installed baseline')
    args=parser.parse_args();mode=args.mode
    if not 30<=args.idle_seconds<=60:parser.error('--idle-seconds must be between 30 and 60')
    idle_mode=mode in {'performance','background-idle'}
    input_idle=hid_idle_clock() if idle_mode else None
    process_usage=process_usage_clock() if idle_mode else None
    if mode in {'startup-workspace','ninja-chat','latency','performance','background-idle'}:
        # WindowServer suspends display updates at the lock screen. Identity
        # checks can still run there, but visible-frame timings cannot certify UI.
        console=plistlib.loads(subprocess.check_output(['ioreg','-a','-n','Root','-d','1']))
        if isinstance(console,list):console=console[0]
        if any(user.get('CGSSessionScreenIsLocked') for user in console.get('IOConsoleUsers',[])):
            raise RuntimeError('Unlock this Mac before measuring visible-window rendering; macOS has paused display updates.')
    root=Path(__file__).resolve().parent.parent
    source=args.bundle or root/'.build/unified/Kai Unified.app'
    report=args.report_dir or root/'.build/validation';report.mkdir(parents=True,exist_ok=True)
    log=report/(mode+'.log')
    env=os.environ.copy();env['KAI_PERFORMANCE_LOG']='1'
    if args.native_history:
        if mode!='latency':parser.error('--native-history requires latency')
        env['KAI_LATENCY_NATIVE_FIXTURE']='1'
    flags=['--unified-smoke-test']
    if mode=='discovery':
        flags+=['--unified-discovery-probe'];marker='KAI_DISCOVERY_PROBE_OK'
        cached=Path.home()/'Library/Application Support/Kai Unified/history.sqlite'
        if cached.is_file():env['KAI_DISCOVERY_CACHE_SOURCE']=str(cached)
    elif mode=='surfaces':
        flags+=['--unified-surface-probe'];marker='KAI_SURFACE_PROBE_OK'
    elif mode=='ninja-chat':
        flags+=['--unified-ninja-render-probe'];marker='KAI_NINJA_RENDER_PROBE_OK';env['KAI_STARTUP_SURFACE']='workspace'
    elif mode=='chat-split':
        flags+=['--unified-chat-split-probe'];marker='KAI_CHAT_SPLIT_PROBE_OK';env['KAI_STARTUP_SURFACE']='workspace'
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
        else:env['KAI_STARTUP_SURFACE']='workspace'
    if idle_mode:env['KAI_STARTUP_WAIT_MS']=str(1000*args.idle_seconds)
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
            if env.get('KAI_VALIDATION_FIXTURE') or env.get('KAI_VALIDATION_BACKGROUND_SOURCE') or env.get('KAI_DISCOVERY_CACHE_SOURCE'):
                prepare="""import os,sys,json,tempfile,shutil
from pathlib import Path
p=Path(tempfile.gettempdir())/('kai-unified-smoke-'+str(os.getpid()));p.mkdir(exist_ok=True)
if os.environ.get('KAI_DISCOVERY_CACHE_SOURCE'):
 import sqlite3
 source=sqlite3.connect(Path(os.environ['KAI_DISCOVERY_CACHE_SOURCE']).as_uri()+'?mode=ro',uri=True)
 target=sqlite3.connect(p/'history.sqlite')
 target.execute('CREATE TABLE sessions(id TEXT PRIMARY KEY,title TEXT NOT NULL,agent TEXT NOT NULL,project_path TEXT NOT NULL,updated_at INTEGER NOT NULL,external_id TEXT,transcript_path TEXT,execution_path TEXT)')
 rows=source.execute('SELECT id,title,agent,project_path,updated_at,external_id,transcript_path,execution_path FROM sessions ORDER BY updated_at DESC LIMIT 6000')
 target.executemany('INSERT INTO sessions VALUES(?,?,?,?,?,?,?,?)',rows);target.commit();target.close();source.close()
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
                    if elapsed>max(65,args.idle_seconds+15):raise TimeoutError('Packaged probe timed out')
                    if idle_mode and 8<=elapsed<=args.idle_seconds-3:
                        usage=process_usage(process.pid)
                        if usage:
                            cpu_seconds,rss_kib,wakeups=usage
                            samples.append({'seconds':elapsed,'cpu_seconds':cpu_seconds,'rss_kib':rss_kib,
                                            'wakeups':wakeups,'hid_idle_seconds':input_idle()})
                        if args.sample_stacks and elapsed>=14 and not sampled:
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
        if mode=='discovery':
            for name in ['cached-navigation-ready','providers-ready','discovery-ready']:
                if name not in timings:raise RuntimeError('Discovery readiness was not measured: '+name)
            if timings['providers-ready']>=timings['discovery-ready']:raise RuntimeError('Providers waited for history discovery')
            if timings['cached-navigation-ready']>=timings['discovery-ready']:raise RuntimeError('Cached navigation waited for history discovery')
        native_commands=re.findall(r'KAI_NATIVE_COMMAND (\S+)',text)
        interactions=[name for name in native_commands if name not in {'pet_visuals','workspace_presentation','workspace_wallpaper_visibility'}]
        result={'mode':mode,'version':info.get('CFBundleShortVersionString'),'elapsed_seconds':round(time.monotonic()-launched,2),'launch_milliseconds':timings,'samples':samples,'native_commands':native_commands,'idle_interactions':interactions,'metrics':{name:[float(ms) for key,ms in re.findall(r'KAI_METRIC (\S+) ([0-9.]+)',text) if key==name] for name in ['warm_chat_switch','cold_chat_switch','warm_reveal','warm_order_front','warm_frame_wake','codex_split_resize_frame','root_render','ninja_frame','chat_open_prepare','chat_model_lookup','chat_history_load','chat_history_apply','project_discovery','project_discovery_install','viewport_prepare','viewport_disk_commit']},'log':str(log)}
        if args.native_history:
            metrics=result['metrics']
            if len(metrics['cold_chat_switch'])!=24 or len(metrics['chat_history_apply'])<48:
                raise RuntimeError('Native latency probe did not render all 48 imported chat opens')
        if samples:
            result['verified_idle_average']=verified_idle(samples)
            if args.sample_stacks or (mode=='performance' and 'occlusionVisible=false' in text):
                result['verified_idle_average']['cpu_percent_from_time_delta']=None
            result['idle_average']={'cpu_percent_from_time_delta':100*(samples[-1]['cpu_seconds']-samples[0]['cpu_seconds'])/(samples[-1]['seconds']-samples[0]['seconds']),'rss_mib':sum(s['rss_kib'] for s in samples)/len(samples)/1024}
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
        if idle_mode and interactions:
            raise RuntimeError(f'Idle measurement received native input ({interactions}); evidence saved, but this run cannot certify idle CPU')
        print(json.dumps({k:v for k,v in result.items() if k!='samples'},indent=2))
        if samples:print('Idle sample averages:',json.dumps(result['idle_average']))
if __name__=='__main__':main()

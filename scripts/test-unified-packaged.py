#!/usr/bin/env python3
"""Run an isolated signed bundle through its purpose-built native probes."""
import argparse,json,os,plistlib,re,shutil,signal,subprocess,tempfile,time
from pathlib import Path

def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('mode',choices=['wallpaper','glass','startup-workspace','startup-extended','startup-compact','performance','latency','background-idle'])
    args=parser.parse_args();mode=args.mode
    root=Path(__file__).resolve().parent.parent
    source=root/'.build/unified/Kai Unified.app'
    report=root/'.build/validation';report.mkdir(parents=True,exist_ok=True)
    log=report/(mode+'.log')
    env=os.environ.copy();env['KAI_PERFORMANCE_LOG']='1'
    flags=['--unified-smoke-test']
    if mode=='latency':
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
    with tempfile.TemporaryDirectory(prefix='kai-packaged-'+mode+'-') as scratch:
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
        result={'mode':mode,'version':info.get('CFBundleShortVersionString'),'elapsed_seconds':round(time.monotonic()-launched,2),'launch_milliseconds':timings,'samples':samples,'native_commands':native_commands,'idle_interactions':interactions,'metrics':{name:[float(ms) for key,ms in re.findall(r'KAI_METRIC (\S+) ([0-9.]+)',text) if key==name] for name in ['warm_chat_switch','cold_chat_switch','warm_reveal','root_render']},'log':str(log)}
        if samples:
            result['idle_average']={'cpu_percent_from_time_delta':100*(samples[-1]['cpu_seconds']-samples[0]['cpu_seconds'])/(samples[-1]['seconds']-samples[0]['seconds']),'cpu_percent':sum(s['cpu_percent'] for s in samples)/len(samples),'rss_mib':sum(s['rss_kib'] for s in samples)/len(samples)/1024,'threads':sum(s['threads'] for s in samples)/len(samples)}
        (report/(mode+'.json')).write_text(json.dumps(result,indent=2))
        if mode in {'performance','background-idle'} and interactions:
            raise RuntimeError(f'Idle measurement received native input ({interactions}); evidence saved, but this run cannot certify idle CPU')
        print(json.dumps({k:v for k,v in result.items() if k!='samples'},indent=2))
        if samples:print('Idle sample averages:',json.dumps(result['idle_average']))
if __name__=='__main__':main()

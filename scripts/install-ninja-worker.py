#!/usr/bin/env python3
"""Install an explicitly selected Kai build as a headless, user-owned Ninja host.
Run on the destination Mac. Uses its existing Tailscale and provider logins.
"""
import argparse,hashlib,json,os,pathlib,plistlib,shutil,socket,subprocess,tempfile,time,signal

def worker_state(address):
    try:
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(5);client.connect(address);client.sendall(b'{"action":"snapshot"}\n')
            with client.makefile('rb') as stream:line=stream.readline(8*1024*1024)
            return json.loads(line) if line else None
    except (FileNotFoundError,ConnectionRefusedError,ConnectionResetError):return None

def retire_idle_worker(address):
    state=worker_state(address)
    if state is None:return
    if any(t['state'] in ('queued','running','approval') for t in state.get('tasks',[])):
        raise ValueError('Finish or stop assigned tasks before replacing the worker process')
    # The inference host may have spawned a detached worker before launchd was
    # installed. Restarting only the launch agent leaves that process serving
    # its old protocol forever. Retire the exact idle socket owner instead.
    result=subprocess.run(['/usr/sbin/lsof','-t','-a','-U',address],capture_output=True,text=True,check=False)
    owners={int(pid) for pid in result.stdout.split() if pid.isdigit()}
    if not owners:raise ValueError('Could not identify the old worker socket owner')
    for pid in owners:
        command=subprocess.check_output(['ps','-ww','-p',str(pid),'-o','args='],text=True)
        if '--ninja-worker' not in command:raise ValueError('Socket belongs to a different process; left it running')
    for pid in owners:os.kill(pid,signal.SIGTERM)
    for _ in range(50):
        # KeepAlive may immediately start the replacement at the same socket.
        # Wait for the original owners, not for an empty socket pathname.
        alive=False
        for pid in owners:
            try:os.kill(pid,0);alive=True
            except ProcessLookupError:pass
        if not alive or worker_state(address) is None:return
        time.sleep(.1)
    raise ValueError('Old worker did not release its socket')

def host_config(root,owner=None):
    path=root/'host.json'
    if path.exists():
        config=json.loads(path.read_text())
        if owner:config['owner']=owner
        return config
    cli=shutil.which('tailscale') or next((str(p) for p in [pathlib.Path('/opt/homebrew/bin/tailscale'),pathlib.Path('/usr/local/bin/tailscale'),pathlib.Path('/Applications/Tailscale.app/Contents/MacOS/Tailscale')] if p.is_file()),None)
    if not cli:raise ValueError('Install and sign in to Tailscale on this host first')
    status=json.loads(subprocess.check_output([cli,'status','--json'],text=True,timeout=8))
    user=status.get('User',{}).get(str(status.get('Self',{}).get('UserID')), {})
    owner=owner or user.get('LoginName')
    if not owner:raise ValueError('Specify --owner with the Tailscale login allowed to use this host')
    return dict(owner=owner,port=32187,identity='whois')

def install(bundle,keep_reachable,worker_only=False,owner=None):
    source=bundle.resolve(strict=True);binary=source/'Contents/MacOS/kai-unified'
    launcher=source/'Contents/Resources/kai-ninja-host'
    if not binary.is_file() or not launcher.is_file():raise ValueError('Choose a Kai build containing its persistent host launcher')
    home=pathlib.Path.home();root=home/'.kai-ninja';root.mkdir(mode=0o700,exist_ok=True)
    data=home/'Library/Application Support/Kai Unified/ninjas'
    address='/private/tmp/kai-ninjas-'+str(os.getuid())+'-'+hashlib.sha256(str(data).encode()).hexdigest()[:10]+'.sock'
    state=worker_state(address)
    if state and any(t['state'] in ('queued','running','approval') for t in state.get('tasks',[])):
        raise ValueError('Finish or stop assigned tasks before updating this worker')
    config=host_config(root,owner)
    domain='gui/'+str(os.getuid());folder=home/'Library/LaunchAgents';folder.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(dir=root) as temp:
        staged=pathlib.Path(temp)/'Runtime.app';resources=staged/'Contents/Resources';resources.mkdir(parents=True)
        target=staged/'Contents/MacOS/kai-unified';target.parent.mkdir();shutil.copy2(binary,target);target.chmod(0o700)
        for name in ['kai_ninja.py','kai_inference_stream.py','kai_ninja_handoff.py','kai_workspace_mcp.py']:
            shutil.copy2(source/'Contents/Resources'/name,resources/name)
        info=plistlib.loads((source/'Contents/Info.plist').read_bytes())
        (staged/'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleIdentifier':'com.kai.ninjas.runtime','CFBundleExecutable':'kai-unified','CFBundleName':'Kai Ninja Runtime','CFBundleVersion':info['CFBundleVersion'],'CFBundleShortVersionString':info['CFBundleShortVersionString'],'LSUIElement':True}))
        subprocess.run(['codesign','--force','--sign','-',str(staged)],check=True)
        host=root/'Worker.app'
        try:stable=plistlib.loads((host/'Contents/Info.plist').read_bytes()).get('KaiStableHost')==1
        except (OSError,ValueError):stable=False
        staged_host=pathlib.Path(temp)/'Worker.app'
        if not stable:
            executable=staged_host/'Contents/MacOS/kai-ninja-host';executable.parent.mkdir(parents=True);shutil.copy2(launcher,executable);executable.chmod(0o700)
            (staged_host/'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleIdentifier':'com.kai.ninjas.worker','CFBundleExecutable':'kai-ninja-host','CFBundleName':'Kai Ninja Worker','CFBundleVersion':'1','KaiStableHost':1,'LSUIElement':True,'NSAppleEventsUsageDescription':'Your Ninjas automate apps on this host for tasks you authorize.'}))
            subprocess.run(['codesign','--force','--sign','-',str(staged_host)],check=True)
        # Validate all replacement artifacts before interrupting the installed host.
        for artifact in [staged]+([] if stable else [staged_host]):
            subprocess.run(['codesign','--verify','--strict',str(artifact)],check=True)
        for label in ['com.kai.ninja.inference','com.kai.ninjas.worker']:
            subprocess.run(['launchctl','bootout',domain+'/'+label],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        retire_idle_worker(address)
        destination=root/'Runtime.app'
        if destination.exists():destination.rename(root/('runtime-backup-'+str(time.time_ns())+'.app'))
        shutil.move(str(staged),str(destination))
        if not stable:
            if host.exists():host.rename(root/('worker-backup-'+str(time.time_ns())+'.app'))
            shutil.move(str(staged_host),str(host))
    executable=host/'Contents/MacOS/kai-ninja-host'
    plist=folder/'com.kai.ninjas.worker.plist'
    plist.write_bytes(plistlib.dumps({'Label':'com.kai.ninjas.worker','ProgramArguments':[str(executable)]+(['--keep-reachable'] if keep_reachable else []),'RunAtLoad':True,'KeepAlive':True,'ThrottleInterval':10,'AssociatedBundleIdentifiers':['com.kai.ninjas.worker'],'EnvironmentVariables':{'PATH':'/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:'+str(home/'.local/bin')},'StandardErrorPath':str(root/'worker.log')}));plist.chmod(0o600)
    subprocess.run(['launchctl','bootstrap',domain,str(plist)],check=True)
    # Refresh the forwarding host as well: it must wake launchd, never spawn a
    # competing runtime under Python's unrelated privacy identity.
    args=['/usr/bin/python3',str(source/'Contents/Resources/kai_ninja_host.py'),'--install','--owner',config['owner'],'--port',str(config['port']),'--worker',str(destination/'Contents/MacOS/kai-unified')]
    if keep_reachable:args.append('--keep-reachable')
    if config.get('identity')=='serve':args.append('--serve')
    subprocess.run(args,check=True)
    print('Installed persistent host:',host)
    if not stable:print('Screen Recording: re-add Kai Ninja Worker in macOS Privacy & Security once. Runtime updates now leave this signed host unchanged.')
    print('The user must log in after reboot. This host uses its own Tailscale identity, provider logins, and workspace paths.')
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('bundle',type=pathlib.Path);p.add_argument('--keep-reachable',action='store_true',help='Prevent system sleep on AC power, without keeping the display awake');p.add_argument('--worker-only',action='store_true',help='Accepted for compatibility; the forwarding host is refreshed to preserve managed ownership');p.add_argument('--owner',help='Allowed Tailscale login; inferred from this host on first setup');args=p.parse_args();install(args.bundle,args.keep_reachable,args.worker_only,args.owner)

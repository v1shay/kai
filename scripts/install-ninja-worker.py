#!/usr/bin/env python3
"""Install an explicitly selected Kai build as a headless, user-owned Ninja host.
Run on the destination Mac. Uses its existing Tailscale and provider logins.
"""
import argparse,hashlib,json,os,pathlib,plistlib,shutil,socket,subprocess,tempfile,time

def install(bundle,keep_reachable,worker_only=False):
    source=bundle.resolve(strict=True);binary=source/'Contents/MacOS/kai-unified'
    if not binary.is_file():raise ValueError('Choose a built Kai Unified.app')
    home=pathlib.Path.home();root=home/'.kai-ninja';root.mkdir(mode=0o700,exist_ok=True)
    data=home/'Library/Application Support/Kai Unified/ninjas'
    address='/private/tmp/kai-ninjas-'+str(os.getuid())+'-'+hashlib.sha256(str(data).encode()).hexdigest()[:10]+'.sock'
    try:
        with socket.socket(socket.AF_UNIX) as c:
            c.settimeout(5);c.connect(address);c.sendall(b'{"action":"snapshot"}\n')
            with c.makefile('rb') as f:state=json.loads(f.readline(8*1024*1024))
            if any(t['state'] in ('queued','running','approval') for t in state['tasks']):raise ValueError('Finish or stop assigned tasks before updating this worker')
    except (FileNotFoundError,ConnectionRefusedError):pass
    config=json.loads((root/'host.json').read_text())
    with tempfile.TemporaryDirectory(dir=root) as temp:
        staged=pathlib.Path(temp)/'Worker.app';resources=staged/'Contents/Resources';resources.mkdir(parents=True)
        target=staged/'Contents/MacOS/kai-unified';target.parent.mkdir();shutil.copy2(binary,target);target.chmod(0o700)
        for name in ['kai_ninja.py','kai_inference_stream.py','kai_ninja_handoff.py','kai_workspace_mcp.py']:
            shutil.copy2(source/'Contents/Resources'/name,resources/name)
        info={'CFBundleIdentifier':'com.kai.ninjas.worker','CFBundleExecutable':'kai-unified','CFBundleName':'Kai Ninja Worker','CFBundleVersion':plistlib.loads((source/'Contents/Info.plist').read_bytes())['CFBundleVersion'],'CFBundleShortVersionString':plistlib.loads((source/'Contents/Info.plist').read_bytes())['CFBundleShortVersionString'],'LSUIElement':True,'NSDownloadsFolderUsageDescription':'Read the reference files you explicitly send through Taildrop.','NSAppleEventsUsageDescription':'Your Ninjas automate apps on this execution host for tasks you authorize.','NSDocumentsFolderUsageDescription':'Your Ninjas work in the project directories you select on this execution host.','NSDesktopFolderUsageDescription':'Your Ninjas work in the project directories you select on this execution host.'}
        (staged/'Contents/Info.plist').write_bytes(plistlib.dumps(info))
        subprocess.run(['codesign','--force','--sign','-','--deep',str(staged)],check=True)
        destination=root/'Worker.app';backup=root/('worker-backup-'+str(int(time.time()))+'.app')
        if destination.exists():destination.rename(backup)
        shutil.move(str(staged),str(destination))
    executable=destination/'Contents/MacOS/kai-unified'
    subprocess.run([str(executable),'--install-ninja-worker']+(['--keep-reachable'] if keep_reachable else []),check=True)
    subprocess.run(['launchctl','kickstart','-k','gui/'+str(os.getuid())+'/com.kai.ninjas.worker'],check=True)
    if worker_only:
        print('Updated headless Ninja worker; inference host left running:',destination)
        return
    args=['/usr/bin/python3',str(source/'Contents/Resources/kai_ninja_host.py'),'--install','--owner',config['owner'],'--port',str(config['port']),'--worker',str(executable)]
    if keep_reachable:args.append('--keep-reachable')
    if config.get('identity')=='serve':args.append('--serve')
    subprocess.run(args,check=True)
    print('Installed headless Ninja worker:',destination)
    print('The user must log in after a reboot. macOS may request access to Downloads or protected project folders.')
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('bundle',type=pathlib.Path);p.add_argument('--keep-reachable',action='store_true',help='Prevent system sleep on AC power, without keeping the display awake');p.add_argument('--worker-only',action='store_true',help='Update the worker without restarting an existing inference host');args=p.parse_args();install(args.bundle,args.keep_reachable,args.worker_only)

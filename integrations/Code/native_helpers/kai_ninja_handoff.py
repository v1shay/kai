#!/usr/bin/env python3
"""Explicit Taildrop workspace snapshots. No credentials or arbitrary archive paths."""
import hashlib,json,os,pathlib,shutil,subprocess,sys,tarfile,tempfile,uuid
LIMIT=512*1024*1024

def digest(path):
    h=hashlib.sha256()
    with open(path,'rb') as f:
        for b in iter(lambda:f.read(1024*1024),b''):h.update(b)
    return h.hexdigest()

def export(project,context,out):
    root=pathlib.Path(project).resolve(strict=True)
    top=subprocess.check_output(['git','-C',str(root),'rev-parse','--show-toplevel'],timeout=10).decode().strip()
    if pathlib.Path(top).resolve()!=root:raise ValueError('Choose the repository root for a complete handoff')
    names=subprocess.check_output(['git','-C',str(root),'ls-files','-z','--cached','--others','--exclude-standard'],timeout=20).split(b'\0')
    files={};total=0
    for raw in set(names):
        if not raw:continue
        name=os.fsdecode(raw);p=root/name
        if not p.exists():continue # staged or unstaged deletion is preserved in manifest
        if p.is_symlink() or not p.is_file():raise ValueError('Handoff requires regular files: '+name)
        total+=p.stat().st_size
        if total>LIMIT or len(files)>=20000:raise ValueError('Workspace exceeds handoff limits (512 MiB / 20,000 files)')
        files[name]={'sha256':digest(p),'mode':p.stat().st_mode&0o777}
    ident=str(uuid.uuid4());dest=pathlib.Path(out)/('kai-ninja-handoff-'+ident+'.tar.gz')
    dest.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory() as temp:
        bundle=pathlib.Path(temp)/'repository.bundle'
        subprocess.run(['git','-C',str(root),'bundle','create',str(bundle),'--all'],check=True,capture_output=True,timeout=60)
        if bundle.stat().st_size+total>LIMIT:raise ValueError('Repository history and workspace exceed the 512 MiB handoff limit')
        branch=subprocess.check_output(['git','-C',str(root),'rev-parse','--abbrev-ref','HEAD'],timeout=10).decode().strip()
        head=subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],timeout=10).decode().strip()
        manifest={'id':ident,'files':files,'head':head,'branch':branch,'context':context,'name':root.name}
        meta=pathlib.Path(temp)/'manifest.json';meta.write_text(json.dumps(manifest));meta.chmod(0o600)
        with tarfile.open(dest,'w:gz') as t:
            t.add(meta,arcname='manifest.json');t.add(bundle,arcname='repository.bundle')
            for name,info in files.items():
                t.add(root/name,arcname='workspace/'+name,recursive=False)
                if digest(root/name)!=info['sha256']:raise ValueError('A workspace file changed during handoff; retry when edits finish')
    dest.chmod(0o600)
    return {'id':ident,'sha256':digest(dest),'archive':str(dest),'bytes':dest.stat().st_size}

def receive(ident,sha,folder):
    ident=str(uuid.UUID(ident))
    if len(sha)!=64 or any(c not in '0123456789abcdef' for c in sha):raise ValueError('Invalid handoff checksum')
    source=pathlib.Path.home()/'Downloads'/('kai-ninja-handoff-'+ident+'.tar.gz')
    if not source.is_file():return {'received':False}
    if source.is_symlink() or source.stat().st_size>LIMIT or digest(source)!=sha:raise ValueError('Handoff checksum or size mismatch')
    managed=pathlib.Path(folder)/'workspaces';managed.mkdir(mode=0o700,parents=True,exist_ok=True)
    target=managed/ident
    if target.exists():
        receipt=json.loads((target/'.kai-handoff.json').read_text())
        if receipt['sha256']!=sha:raise ValueError('Handoff identity collision')
        return receipt
    with tempfile.TemporaryDirectory(dir=managed) as temp:
        temp=pathlib.Path(temp);total=0
        with tarfile.open(source,'r:gz') as tar:
            seen=set()
            for m in tar:
                parts=pathlib.PurePosixPath(m.name).parts
                if m.name in seen or not m.isfile() or '..' in parts or m.name.startswith('/') or len(seen)>20002:raise ValueError('Unsafe handoff archive')
                if m.name not in ('manifest.json','repository.bundle') and (not parts or parts[0]!='workspace' or '.git' in parts):raise ValueError('Invalid handoff member')
                total+=m.size
                if total>LIMIT:raise ValueError('Expanded handoff exceeds 512 MiB')
                seen.add(m.name);dest=temp/m.name;dest.parent.mkdir(parents=True,exist_ok=True)
                with tar.extractfile(m) as src,open(dest,'xb') as dst:shutil.copyfileobj(src,dst)
        meta=json.loads((temp/'manifest.json').read_text())
        if meta['id']!=ident or len(meta['files'])>20000:raise ValueError('Invalid handoff manifest')
        actual={str(p.relative_to(temp/'workspace')) for p in (temp/'workspace').rglob('*') if p.is_file()}
        if actual!=set(meta['files']):raise ValueError('Handoff file inventory mismatch')
        for name,info in meta['files'].items():
            if digest(temp/'workspace'/name)!=info['sha256']:raise ValueError('Handoff file checksum mismatch')
        checkout=temp/'checkout'
        subprocess.run(['git','clone','--quiet',str(temp/'repository.bundle'),str(checkout)],check=True,capture_output=True,timeout=60)
        subprocess.run(['git','-C',str(checkout),'checkout','--quiet','--detach',meta['head']],check=True,capture_output=True,timeout=20)
        # Overlay the exact working tree, preserving deletions and untracked files.
        for p in checkout.iterdir():
            if p.name!='.git':shutil.rmtree(p) if p.is_dir() else p.unlink()
        for name,info in meta['files'].items():
            dest=checkout/name;dest.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(temp/'workspace'/name,dest);dest.chmod(info['mode']&0o777)
        subprocess.run(['git','-C',str(checkout),'remote','remove','origin'],check=True,capture_output=True,timeout=10)
        receipt={'received':True,'id':ident,'sha256':sha,'project':str(target),'context':meta['context'],'name':meta['name']}
        (checkout/'.kai-handoff.json').write_text(json.dumps(receipt));(checkout/'.kai-handoff.json').chmod(0o600)
        with (checkout/'.git/info/exclude').open('a') as f:f.write('\n.kai-handoff.json\n')
        checkout.rename(target)
    return receipt

if __name__=='__main__':
    try:
        args=json.load(sys.stdin)
        result=export(args['project'],args.get('context',''),args['out']) if args['action']=='export' else receive(args['id'],args['sha256'],args['root'])
        print(json.dumps(result))
    except Exception as e:print(json.dumps({'error':str(e)}))

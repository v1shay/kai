#!/usr/bin/env python3
"""Content-addressed Swift builds. Failed compiles never replace a usable library."""
import hashlib,json,os,subprocess,sys,tempfile
from pathlib import Path

def main():
    output=Path(sys.argv[1]);args=sys.argv[2:]
    compiler=subprocess.check_output(['xcrun','--find','swiftc'],text=True).strip()
    version=subprocess.check_output([compiler,'--version'])
    sdk=subprocess.check_output(['xcrun','--show-sdk-path'],text=True).strip()
    sdk_version=subprocess.check_output(['xcrun','--show-sdk-version'])
    sources=[Path(arg) for arg in args if arg.endswith('.swift')]
    digest=hashlib.sha256(version+sdk.encode()+sdk_version+json.dumps(args).encode())
    for source in sources:
        digest.update(str(source.resolve()).encode());digest.update(source.read_bytes())
    key=digest.hexdigest();manifest=output.with_suffix(output.suffix+'.build.json')
    try:
        saved=json.loads(manifest.read_text())
        if saved['source']==key and hashlib.sha256(output.read_bytes()).hexdigest()==saved['library']:
            print(f'Cached native library: {output.name}',flush=True);return
    except (OSError,ValueError,KeyError):pass
    output.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.native-',dir=output.parent) as temporary:
        staged=Path(temporary)/output.name
        subprocess.run([compiler,'-sdk',sdk,*args,'-o',str(staged)],check=True)
        record={'source':key,'library':hashlib.sha256(staged.read_bytes()).hexdigest()}
        staged_manifest=Path(temporary)/'build.json';staged_manifest.write_text(json.dumps(record))
        os.replace(staged,output);os.replace(staged_manifest,manifest)
    print(f'Compiled native library: {output.name}',flush=True)
if __name__=='__main__':main()

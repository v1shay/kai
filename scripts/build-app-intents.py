#!/usr/bin/env python3
"""Extract real Shortcuts metadata from the same compiled AppIntent declarations."""
import json,pathlib,subprocess,sys,tempfile
root=pathlib.Path(__file__).resolve().parent.parent
binary,resources=map(pathlib.Path,sys.argv[1:])
def query(*args):return subprocess.check_output(args,text=True).strip()
sdk=query('xcrun','--show-sdk-path');processor=query('xcrun','--find','appintentsmetadataprocessor');swift=query('xcrun','--find','swiftc')
version=query('xcodebuild','-version').split('Build version ')[-1].strip()
source=root/'Sources/NotchPrototype/WorkspaceIntents.swift'
with tempfile.TemporaryDirectory(prefix='kai-intents-')as scratch:
 work=pathlib.Path(scratch);protocols=work/'protocols.json';constants=work/'intent.swiftconstvalues'
 protocols.write_text(json.dumps(['AppIntent','AppShortcutsProvider','AppIntents.AppIntent','AppIntents.AppShortcutsProvider']))
 subprocess.run([swift,'-sdk',sdk,'-parse-as-library','-emit-object','-module-name','KaiNotch','-module-cache-path','/private/tmp/kai-unified-swift-v2','-emit-const-values-path',str(constants),'-Xfrontend','-const-gather-protocols-file','-Xfrontend',str(protocols),str(source),'-o',str(work/'intent.o')],check=True)
 values=json.loads(constants.read_text())
 if not any(v.get('typeName')=='KaiNotch.RunKaiWorkflowIntent' for v in values):raise RuntimeError('The workflow intent was not extracted by Swift')
 arch=query('uname','-m')
 subprocess.run([processor,'--binary-file',str(binary),'--module-name','KaiNotch','--source-files',str(source),'--swift-const-vals',str(constants),'--output',str(resources),'--target-triple',arch+'-apple-macosx13.0','--sdk-root',sdk,'--toolchain-dir',str(pathlib.Path(processor).parent.parent),'--xcode-version',version,'--force','--compile-time-extraction'],check=True)
 output=resources/'Metadata.appintents/extract.actionsdata'
 if not output.is_file()or output.stat().st_size<100:raise RuntimeError('No usable App Intents metadata was produced')
 print('Verified extracted workflow metadata for Shortcuts')

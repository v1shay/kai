#!/usr/bin/env python3
"""Explicit live validation against a selected inference host, isolated native homes."""
import argparse, os, pathlib, subprocess, tempfile, time, json

def main():
 p=argparse.ArgumentParser();p.add_argument('--binary',required=True,type=pathlib.Path);p.add_argument('--endpoint',required=True);p.add_argument('--model',required=True);p.add_argument('--node',action='store_true');p.add_argument('--matrix',action='store_true');p.add_argument('--harness',choices=['codex','claude'],default='codex');p.add_argument('--engine',choices=['ollama','llama_cpp'],default='ollama');args=p.parse_args()
 report=pathlib.Path(__file__).resolve().parents[1]/'.build/validation/ninja';report.mkdir(parents=True,exist_ok=True);results=[]
 for engine,harness in ([(e,h) for e in ['ollama','llama_cpp'] for h in ['codex','claude']] if args.matrix else [(args.engine,args.harness)]):
  with tempfile.TemporaryDirectory(prefix='kai-ninja-live-validation-') as temporary:
   root=pathlib.Path(temporary);(root/'codex').mkdir();(root/'claude').mkdir();env=os.environ.copy();env.update(ORBIT_DATA_DIR=str(root/'data'),CODEX_HOME=str(root/'codex'),CLAUDE_CONFIG_DIR=str(root/'claude'),KAI_NINJA_TEST_ENDPOINT=args.endpoint,KAI_NINJA_TEST_MODEL=args.model,KAI_NINJA_TEST_NODE='1' if args.node else '0',KAI_NINJA_TEST_ENGINE=engine,KAI_NINJA_TEST_HARNESS=harness)
   started=time.monotonic();log=report/(engine+'-'+harness+'.log')
   with log.open('w') as output:
    result=subprocess.run([str(args.binary),'ninja::tests::installed_harness_uses_ninja_inference_and_native_tools','--ignored','--exact','--nocapture'],env=env,stdout=output,stderr=subprocess.STDOUT,timeout=260)
   entry={'engine':engine,'harness':harness,'passed':result.returncode==0,'seconds':round(time.monotonic()-started,2)};results.append(entry);print(json.dumps(entry),flush=True)
   (report/'matrix.json').write_text(json.dumps(results,indent=2))
 return 0 if all(r['passed'] for r in results) else 1
if __name__=='__main__':raise SystemExit(main())

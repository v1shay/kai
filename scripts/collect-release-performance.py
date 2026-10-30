#!/usr/bin/env python3
"""Measure the signed, isolated build; fail the release gate on missing/slow data.

The running user app and all provider sessions remain untouched. Benchmarks use
saved fixture chats, the real renderer, and the real native surface transitions.
"""
import hashlib,json,subprocess,sys
from pathlib import Path
root=Path(__file__).resolve().parent.parent
reports=root/'.build/validation'
def probe(mode):
 subprocess.run([sys.executable,str(root/'scripts/test-unified-packaged.py'),mode],check=True)
 return json.loads((reports/(mode+'.json')).read_text())
def main():
 reports.mkdir(parents=True,exist_ok=True)
 (reports/'release-performance-gate.json').write_text(json.dumps({'passed':False,'reason':'Measurement in progress; previous results are not a certification of this candidate'})+'\n')
 bundle=root/'.build/unified/Kai Unified.app/Contents'
 def cohort():return {str(path.relative_to(bundle)):hashlib.sha256(path.read_bytes()).hexdigest() for path in [bundle/'MacOS/kai-unified',bundle/'Resources/libKaiNotch.dylib']}
 initial=cohort()
 evidence={name:[] for name in ['first_workspace_frame_ms_p95','warm_chat_switch_ms_p95','warm_reveal_ms_p95','render_cpu_ms_p95','visible_idle_cpu_percent','background_idle_cpu_percent']}
 failure=None
 try:
  for _ in range(5):
   result=probe('startup-workspace');evidence['first_workspace_frame_ms_p95'].append(result['launch_milliseconds']['first-workspace-frame']);evidence['render_cpu_ms_p95'].extend(result['metrics']['root_render'])
  latency=probe('latency');evidence['render_cpu_ms_p95'].extend(latency['metrics']['root_render'])
  for key in ['warm_chat_switch','warm_reveal']:evidence[key+'_ms_p95']=latency['metrics'][key]
  for mode,key in [('performance','visible_idle_cpu_percent'),('background-idle','background_idle_cpu_percent')]:
   value=probe(mode);evidence[key]=[value['idle_average']['cpu_percent_from_time_delta']]
 except (subprocess.CalledProcessError,RuntimeError,KeyError) as error:failure=str(error)
 path=reports/'release-performance-evidence.json';path.write_text(json.dumps(evidence,indent=2)+'\n')
 matched=cohort()==initial
 (reports/'release-performance-cohort.json').write_text(json.dumps({'binaries':initial,'matched':matched,'failure':failure},indent=2)+'\n')
 status=subprocess.call([sys.executable,str(root/'scripts/check-release-performance.py'),str(path),'--output',str(reports/'release-performance-gate.json')])
 if failure or not matched:
  report=json.loads((reports/'release-performance-gate.json').read_text());report.update(passed=False,reason=failure or 'Candidate changed during measurement')
  (reports/'release-performance-gate.json').write_text(json.dumps(report,indent=2)+'\n');return 1
 return status
if __name__=='__main__':sys.exit(main())

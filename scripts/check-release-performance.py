#!/usr/bin/env python3
"""Strict release gate. Missing samples fail; thresholds never auto-adjust.

Measurements are app process-only; CLI agents and dev servers must be reported
separately. Cold OS-cache launch is a separate manual/device benchmark.
"""
import argparse,json,math,re,sys
from pathlib import Path

LIMITS={"first_workspace_frame_ms_p95":200,"warm_chat_switch_ms_p95":50,"warm_reveal_ms_p95":50,"visible_idle_cpu_percent":0.5,"background_idle_cpu_percent":0.1,"render_cpu_ms_p95":8.3}
def percentile(values,p=.95):
 if not values:raise ValueError("Missing performance samples")
 values=sorted(float(v)for v in values)
 if any(not math.isfinite(v)or v<0 for v in values):raise ValueError("Invalid performance sample")
 return values[max(0,math.ceil(len(values)*p)-1)]
def evaluate(evidence):
 results={}
 for name,budget in LIMITS.items():
  values=evidence.get(name)
  minimum=20 if "switch" in name or "reveal" in name or "render_cpu" in name else 5 if "frame" in name else 1
  if not isinstance(values,list)or len(values)<minimum:results[name]={"pass":False,"reason":f"Requires {minimum} real samples","budget":budget};continue
  try:
   checked=percentile(values);value=checked if "p95"in name else max(float(v)for v in values);results[name]={"pass":value<=budget,"measured":value,"budget":budget,"samples":len(values)}
  except ValueError as e:results[name]={"pass":False,"reason":str(e),"budget":budget}
 return {"passed":all(r["pass"]for r in results.values()),"metrics":results,"scope":"App-process measurements; rendering CPU is not GPU/presentation latency"}
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument("evidence",type=Path);p.add_argument("--output",type=Path);args=p.parse_args()
 report=evaluate(json.loads(args.evidence.read_text()))
 text=json.dumps(report,indent=2);print(text)
 if args.output:args.output.write_text(text+"\n")
 return 0 if report["passed"]else 1
if __name__=="__main__":sys.exit(main())

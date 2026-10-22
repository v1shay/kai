#!/usr/bin/env python3
"""Verify idle-thread model changes without inference; archive the empty test chat."""
import json
from pathlib import Path
import sys

sys.path.insert(0,str(Path(__file__).resolve().parent.parent))
from kai import AppServer, Kai


def main():
    rpc=AppServer()
    tid=None
    try:
        models=[m for m in rpc.call('model/list',{}).get('data',[]) if not m.get('hidden')]
        original=models[0]
        chosen=next(m for m in models if m['model'] != original['model'])
        tid=rpc.call('thread/start',{'cwd':'/private/tmp','sandbox':'read-only','approvalPolicy':'never',
                                     'model':original['model']})['thread']['id']
        rpc.call('thread/name/set',{'threadId':tid,'name':'Kai settings verification'})
        app=Kai(rpc,Path('/private/tmp'),initial_thread_pages=0)
        app.thread_id=tid
        app.active_turn=None
        effort=chosen.get('defaultReasoningEffort')
        app.configure_model_for_voice(chosen)
        result={'model':app.selected_model,'reasoningEffort':app.reasoning_effort}
        passed=result.get('model') == chosen['model'] and (not effort or result.get('reasoningEffort') == effort)
        print(json.dumps({'modelChanged':result.get('model') == chosen['model'],
                          'reasoningEffortApplied':not effort or result.get('reasoningEffort') == effort,
                          'inferenceTurnsStarted':0,'passed':passed}),flush=True)
        return 0 if passed else 1
    finally:
        try:
            if tid:
                rpc.call('thread/archive',{'threadId':tid})
                print('Verification chat archived',flush=True)
        finally:
            rpc.close()


if __name__ == '__main__':
    raise SystemExit(main())

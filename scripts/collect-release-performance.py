#!/usr/bin/env python3
"""Measure a signed build through its isolated workspace and native surfaces."""
import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUNDLE = ROOT / '.build/unified/Kai Unified.app/Contents'


def cohort():
    binaries = ('MacOS/kai-unified', 'Resources/libKaiNotch.dylib')
    return {name: hashlib.sha256((BUNDLE / name).read_bytes()).hexdigest() for name in binaries}


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--report-dir', type=Path, default=ROOT / '.build/validation')
    reports = parser.parse_args().report_dir
    reports.mkdir(parents=True, exist_ok=True)
    gate = reports / 'release-performance-gate.json'
    write(gate, {'passed': False, 'reason': 'Measurement in progress'})
    initial = cohort()
    evidence = {name: [] for name in (
        'first_workspace_frame_ms_max', 'cold_chat_switch_ms_p95',
        'cold_chat_switch_ms_max', 'warm_chat_switch_ms_p95', 'warm_reveal_ms_p95',
        'render_cpu_ms_p95', 'visible_idle_cpu_percent', 'background_idle_cpu_percent',
    )}
    failures = []

    def probe(mode, *options):
        try:
            subprocess.run([
                sys.executable, str(ROOT / 'scripts/test-unified-packaged.py'),
                mode, '--report-dir', str(reports), *options,
            ], check=True, stdout=subprocess.DEVNULL)
            return json.loads((reports / (mode + '.json')).read_text())
        except (subprocess.CalledProcessError, OSError, ValueError) as error:
            failures.append(f'{mode}: {error}')
            return None

    for index in range(1, 6):
        result = probe('startup-workspace')
        if result:
            frame = result['launch_milliseconds'].get('first-workspace-frame')
            if frame is not None:
                evidence['first_workspace_frame_ms_max'].append(frame)
            evidence['render_cpu_ms_p95'].extend(result['metrics']['root_render'])
            write(reports / f'startup-workspace-{index}.json', result)
    latency = probe('latency')
    if latency:
        evidence['render_cpu_ms_p95'].extend(latency['metrics']['root_render'])
        for key in ('warm_chat_switch', 'warm_reveal'):
            evidence[key + '_ms_p95'] = latency['metrics'][key]
        write(reports / 'workspace-latency.json', latency)
    native = probe('latency', '--native-history')
    if native:
        for key in ('cold_chat_switch_ms_p95', 'cold_chat_switch_ms_max'):
            evidence[key] = native['metrics']['cold_chat_switch']
    for mode, key in (('performance', 'visible_idle_cpu_percent'),
                      ('background-idle', 'background_idle_cpu_percent')):
        result = probe(mode, '--idle-seconds', '60')
        if result:
            measured = result['verified_idle_average']['cpu_percent_from_time_delta']
            if measured is not None:
                evidence[key] = [measured]

    path = reports / 'release-performance-evidence.json'
    write(path, evidence)
    matched = cohort() == initial
    write(reports / 'release-performance-cohort.json',
          {'binaries': initial, 'matched': matched, 'failures': failures})
    status = subprocess.call([
        sys.executable, str(ROOT / 'scripts/check-release-performance.py'),
        str(path), '--output', str(gate),
    ])
    if failures or not matched:
        report = json.loads(gate.read_text())
        report.update(passed=False, failures=failures, candidate_changed=not matched)
        write(gate, report)
        return 1
    return status


if __name__ == '__main__':
    sys.exit(main())

#!/usr/bin/env python3
"""Fail-closed release checks for the shared app and isolated packaged workflows.

Never installs a bundle, restarts a user worker, or uses real provider credentials.
Set CARGO_TARGET_DIR to reuse a test cache; KAI_RELEASE_TARGET_DIR selects the
separate release cache. Reports retain the exact command, exit code and duration.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--bundle', type=Path, help='Validate this existing bundle instead of building one')
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    report = root / '.build/validation' / ('quality-' + datetime.datetime.now().strftime('%Y%m%d-%H%M%S'))
    report.mkdir(parents=True)
    env = os.environ.copy()
    env.setdefault('CARGO_BUILD_JOBS', '2')
    # UI fixtures also create database and runtime worker threads. Bound test
    # concurrency so their I/O deadlines don't measure hundreds of other fixtures.
    env.setdefault('RUST_TEST_THREADS', '2')
    env.setdefault('CARGO_TARGET_DIR', str(root / '.build/unified-test-cargo'))
    records = []

    def run(name, command, environment=env):
        print(name, flush=True)
        started = time.monotonic()
        with (report / (name + '.log')).open('w') as log:
            result = subprocess.run(command, cwd=root, env=environment, stdout=log, stderr=subprocess.STDOUT)
        records.append(dict(name=name, command=command, exit_code=result.returncode,
                            seconds=round(time.monotonic() - started, 2)))
        (report / 'checks.json').write_text(json.dumps(records, indent=2))
        if result.returncode:
            raise RuntimeError(f'{name} failed; see {report / (name + ".log")}')

    run('rust', ['cargo', 'test', '--offline', '--manifest-path', 'integrations/Code/Cargo.toml', '--bin', 'orbit'])
    run('swift', ['swift', 'test'])
    run('native-continuity', ['scripts/test-unified-native.sh', 'continuity'])
    run('native-matrix', ['scripts/test-unified-native.sh', 'matrix'])
    run('native-picker', ['scripts/test-unified-native.sh', 'picker'])
    for name in ['ninjas-worker', 'ninja', 'ninja-installer']:
        run(name, [sys.executable, 'scripts/test-' + name + '.py'])
    bundle = args.bundle.resolve() if args.bundle else root / '.build/unified/Kai Unified.app'
    if not args.bundle:
        release_env = dict(env, CARGO_TARGET_DIR=env.get('KAI_RELEASE_TARGET_DIR', str(root / '.build/unified-cargo')))
        run('build', ['scripts/build-unified-app.sh'], release_env)
    run('signature', ['codesign', '--verify', '--deep', '--strict', str(bundle)])
    run('approval-protocol', [sys.executable, 'scripts/test-ninja-approval-protocol.py', str(bundle / 'Contents/MacOS/kai-unified')])
    # Sequential probes avoid contaminating frame and CPU measurements with one another.
    for mode in ['surfaces', 'ninja-chat', 'menu-bar', 'startup-workspace', 'startup-extended', 'startup-compact', 'latency', 'performance', 'background-idle']:
        run(mode, [sys.executable, 'scripts/test-unified-packaged.py', mode, '--bundle', str(bundle)])
        source = root / '.build/validation' / (mode + '.json')
        (report / source.name).write_bytes(source.read_bytes())
    print(f'All checks passed. Evidence: {report}', flush=True)


if __name__ == '__main__':
    main()

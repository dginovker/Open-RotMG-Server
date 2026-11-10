#!/usr/bin/env python3
"""Actual unsupported-platform CLI/ack/privacy proof, NOT execution of Win32 publication."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
args = parser.parse_args()
assert sys.platform != 'win32', 'Native Windows publication is covered by the conditional Rust cases'
out = Path(os.environ['QA_RESULTS_DIR'])
binary = out / 'openrotmg'; os.link(args.binary.resolve(), binary)
root = out / 'PRIVATE_ROOT_CANARY'; root.mkdir(mode=0o700)
uuid = '01234567-89ab-cdef-0123-456789abcdef'
source = f'runtime-activity.json.partial-{uuid}'
for name, data in [(source, b'PRIVATE_CONTROL_CONTENT_CANARY'), ('state/game.db', b'PRIVATE_GAME_CANARY'), ('backups/original.db', b'PRIVATE_BACKUP_CANARY')]:
    path = root / name; path.parent.mkdir(parents=True, exist_ok=True)
    with path.open('xb') as file:
        file.write(data); file.flush(); os.fsync(file.fileno())

def snapshot():
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest() if path.is_file() else 'directory'
            for path in sorted(root.rglob('*'))}
before = snapshot()
report = dict(binary=str(binary), binarySha256=hashlib.sha256(binary.read_bytes()).hexdigest(), platform=sys.platform,
              scope='explicit unsupported platform only; no Win32 call executed', cases=[])
for operation, values, expected in [
    ('publish-replace', [source, 'runtime-activity.json'], 'publish-replace'),
    ('publish-distinct', [source, 'runtime-activity.json'], 'publish-distinct'),
    ('move-distinct', ['runtime-pending.json', 'runtime-recovery-history/' + 'a'*64 + '.pending.json'], 'move-distinct'),
    ('create-container', ['runtime-staging'], 'create-container'),
    ('publish-replace', ['state/game.db', 'state/game.db'], 'publish-replace'),
    ('create-container', ['backups'], 'create-container'),
    ('create-root', ['new-root'], 'invalid'),
]:
    command = [str(binary), '--desktop-metadata', operation, str(root), *values]
    result = subprocess.run(command, cwd=out, capture_output=True, timeout=30)
    assert result.returncode != 0
    assert len(result.stdout) <= 16384 and result.stdout.endswith(b'\n') and result.stdout.count(b'\n') == 1
    ack = json.loads(result.stdout)
    assert set(ack) == {'protocol', 'operation', 'outcome', 'error'}
    assert ack['protocol'] == 1 and ack['operation'] == expected and ack['outcome'] == 'unknown'
    for private in [str(root).encode(), b'PRIVATE_CONTROL_CONTENT_CANARY', b'PRIVATE_GAME_CANARY', b'PRIVATE_BACKUP_CANARY']:
        assert private not in result.stdout + result.stderr
    assert b'runtime platform capabilities' not in result.stdout + result.stderr
    assert snapshot() == before and not (out / '.local').exists()
    report['cases'].append(dict(operation=operation, exit=result.returncode, ack=ack, sourceUnchanged=True))
with open('/dev/full', 'wb') as full:
    result = subprocess.run([str(binary), '--desktop-metadata', 'create-container', str(root), 'runtime-staging'], cwd=out, stdout=full, stderr=subprocess.PIPE, timeout=30)
assert result.returncode != 0 and snapshot() == before
assert b'outcome unknown' in result.stderr and b'PRIVATE_ROOT_CANARY' not in result.stderr
report['brokenStdout'] = dict(exit=result.returncode, usableAck=False, sourceUnchanged=True)
(out / 'desktop-metadata-result.json').write_text(json.dumps(report, indent=2)+'\n')
print(json.dumps(report, indent=2))

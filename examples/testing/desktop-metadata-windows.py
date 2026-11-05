#!/usr/bin/env python3
"""Native Windows CLI wire/publication case for parent-owned CI, not a power-loss or updater journey."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
args = parser.parse_args()
assert sys.platform == 'win32', 'This case must execute the actual Win32 producer'
binary = args.binary.resolve()
out = Path(os.environ['QA_RESULTS_DIR'])
root = out / 'existing-private-metadata-树-🌙'; root.mkdir()
(root / 'game.db').write_bytes(b'SYNTHETIC_PROGRESS_CANARY')
records = []

def invoke(operation, destination, source=None, success=True):
    command = [str(binary), '--desktop-metadata', operation, str(root)]
    if source is not None: command.append(source)
    command.append(destination)
    child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    pid = child.pid; stdout, stderr = child.communicate(timeout=30)
    assert (child.returncode == 0) == success
    assert len(stdout) <= 16384 and stdout.endswith(b'\n') and stdout.count(b'\n') == 1
    ack = json.loads(stdout)
    if success:
        assert set(ack) == {'protocol','operation','process','root','source','destination','kind','published'}
        assert ack == dict(protocol=1,operation=operation,process=pid,root=str(root),source=source,destination=destination,
                           kind='directory' if operation=='create-container' or operation=='move-distinct' and destination.startswith('runtimes/') else 'file',published=True)
    else:
        assert set(ack) == {'protocol','operation','outcome','error'} and ack['outcome'] == 'unknown'
        assert str(root).encode() not in stdout+stderr
    records.append(dict(operation=operation,process=pid,exit=child.returncode,ack=ack))

def temporary(destination, data):
    source = destination + '.partial-' + str(uuid.uuid4())
    with (root / source).open('xb') as file:
        file.write(data); file.flush(); os.fsync(file.fileno())
    return source

for directory in ['runtime-staging','runtimes','runtime-recovery-history']:
    invoke('create-container', directory)
    invoke('create-container', directory, success=False)
source = temporary('runtime-activity.json', b'ACTIVE')
invoke('publish-distinct', 'runtime-activity.json', source)
assert not (root/source).exists() and (root/'runtime-activity.json').read_bytes() == b'ACTIVE'
source = temporary('runtime-activity.json', b'NEW_ACTIVE')
invoke('publish-distinct', 'runtime-activity.json', source, success=False)
assert (root/source).read_bytes() == b'NEW_ACTIVE' and (root/'runtime-activity.json').read_bytes() == b'ACTIVE'
invoke('publish-replace', 'runtime-activity.json', source)
retired = 'runtime-recovery-history/' + hashlib.sha256(b'NEW_ACTIVE').hexdigest() + '.activity.json'
invoke('move-distinct', retired, 'runtime-activity.json')
assert (root/retired).read_bytes() == b'NEW_ACTIVE' and not (root/'runtime-activity.json').exists()
stage = 'runtime-staging/generation-Test1'; invoke('create-container', stage)
(root/stage/'runtime').mkdir()
descriptor = stage + '/generation.json'
invoke('publish-distinct', descriptor, temporary(descriptor, b'SYNTHETIC namespace-only descriptor'))
invoke('move-distinct', 'runtimes/source-' + 'a'*64, stage)
# Closing the pipe reader BEFORE spawn deterministically makes acknowledgement writing fail.
source = temporary('runtime-activity.json', b'COMMITTED_WITH_LOST_ACK')
reader, writer = os.pipe(); os.close(reader)
with os.fdopen(writer, 'wb') as broken:
    child = subprocess.run([str(binary),'--desktop-metadata','publish-replace',str(root),source,'runtime-activity.json'],stdout=broken,stderr=subprocess.PIPE,timeout=30)
assert child.returncode != 0 and not (root/source).exists()
assert (root/'runtime-activity.json').read_bytes() == b'COMMITTED_WITH_LOST_ACK'
assert (root/'game.db').read_bytes() == b'SYNTHETIC_PROGRESS_CANARY'
assert not (root/'state.lock').exists() and not (root/'server.pem').exists()
result = dict(scope='native Win32 CLI/close/publication only; not recursive tree power-loss or desktop authority qualification',
              binarySha256=hashlib.sha256(binary.read_bytes()).hexdigest(),cases=records,ackFailure=dict(exit=child.returncode,destinationPreserved=True,sourceConsumed=True))
(out/'desktop-metadata-windows-result.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))

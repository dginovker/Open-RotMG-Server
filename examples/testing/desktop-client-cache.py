#!/usr/bin/env python3
"""Actual server HTTP proof with synthetic static fixtures, not browser/Ruffle gameplay."""
import argparse
import hashlib
import http.client
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--reference-swf', type=Path, required=True)
args = parser.parse_args()
repo = Path(__file__).resolve().parents[2]
out = Path(os.environ['QA_RESULTS_DIR'])
binary = out / 'openrotmg'; os.link(args.binary.resolve(), binary)
sha = lambda value: hashlib.sha256(value).hexdigest()
reference = args.reference_swf.resolve()
reference_sha = sha(reference.read_bytes())
assert reference_sha == '07ae4fc3ca021a5bf08bbf94f057f7620be18b85e01abd763b3a198382e8fa48'
client = out / 'synthetic-client'; client.mkdir()
files = ['index.html', 'launcher.js', 'draw.html', 'native-texture-editor.swf', 'vendor/ruffle.js', 'vendor/fixture.wasm']
for name in files:
    path = client / name; path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(('SYNTHETIC HTTP FIXTURE, NOT EXECUTABLE UPSTREAM ASSET: ' + name).encode())
    os.utime(path, (946684800, 946684800))
(client / 'client.swf').symlink_to(reference)
assets = {name: (client / name).read_bytes() for name in files}
assets['sfx/arrowShoot.mp3'] = (repo / 'data/sfx/arrowShoot.mp3').read_bytes()
state = out / 'state'
with socket.socket() as probe:
    probe.bind(('127.0.0.1', 0)); port = probe.getsockname()[1]
origin = f'http://127.0.0.1:{port}'
result = dict(binary_sha256=sha(binary.read_bytes()), reference_swf=str(reference), reference_sha256=reference_sha,
              source_client='synthetic fixture only; no mutable ROOT/client read', origin=origin, cases=[])

def request(port, method, path, headers=None, body=None):
    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=10)
    try:
        connection.request(method, path, body=body, headers=headers or {})
        response = connection.getresponse()
        status = response.status; values = dict((k.lower(), v) for k, v in response.getheaders())
        value = response.read()
        return status, values, value
    finally:
        connection.close()

def generation(label):
    identity = dict(scope='SYNTHETIC HTTP fixture identity, not a signed production pair', runtime=result['binary_sha256'],
                    reference_swf=reference_sha, files={k: sha(v) for k, v in assets.items()})
    raw = json.dumps(identity, sort_keys=True, separators=(',', ':')).encode()
    (out / f'{label}-synthetic-identity.json').write_bytes(raw)
    return sha(raw)

def run(label, desktop, selected=None, old=None):
    config = dict(bind=f'127.0.0.1:{port}', origin=origin, data=str(repo / 'data'), client=str(client), state=str(state),
                  max_players=85, behind_proxy=False, shore_bots=False, bot_llm=False, realm_seed=42)
    if selected is not None: config['client_generation'] = selected
    config_path = out / f'{label}.json'; config_path.write_text(json.dumps(config))
    command = [str(binary), '--config', str(config_path)]
    if desktop: command.append('--desktop')
    records = []
    with (out / f'{label}.log').open('w') as log:
        child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 180
            while True:
                assert child.poll() is None, 'server exited before ready'
                try:
                    status, _, body = request(port, 'GET', '/health')
                    assert status == 200 and json.loads(body)['process'] == child.pid
                    break
                except (OSError, http.client.HTTPException):
                    assert time.monotonic() < deadline
                    time.sleep(.05)
            prefix = f'/_runtime/{selected}/' if selected else '/'
            if selected:
                for path in ['/', prefix.rstrip('/')]:
                    status, headers, _ = request(port, 'GET', path)
                    assert status == 307 and headers['location'] == prefix and headers['cache-control'] == 'no-store'
                assert request(port, 'GET', prefix)[2] == assets['index.html']
                for name in assets:
                    status, headers, _ = request(port, 'GET', '/' + name)
                    assert status == 404 and headers['cache-control'] == 'no-store'
                assert request(port, 'GET', '/client.swf')[0] == 404
                foreign = old if old is not None else 'f' * 64
                assert foreign != selected
                assert request(port, 'GET', f'/_runtime/{foreign}/launcher.js')[0] == 404
            swf_status, swf_headers, patched = request(port, 'GET', prefix + 'client.swf')
            assert swf_status == 200 and swf_headers['content-type'] == 'application/x-shockwave-flash'
            assert patched[:3] in (b'CWS', b'FWS') and sha(patched) != reference_sha
            selected_assets = {**assets, 'client.swf': patched}
            for path, expected in selected_assets.items():
                status, headers, body = request(port, 'GET', prefix + path)
                assert status == 200 and body == expected
                if desktop: assert headers['cache-control'] == 'no-store'
                records.append(dict(path=prefix + path, status=status, bytes=len(body), sha256=sha(body), type=headers['content-type']))
                for method in ['GET', 'HEAD']:
                    if desktop:
                        conditions = {'If-Modified-Since': 'Thu, 01 Jan 2099 00:00:00 GMT', 'If-None-Match': '*',
                                      'If-Match': '"wrong-etag"', 'If-Unmodified-Since': 'Thu, 01 Jan 1970 00:00:00 GMT',
                                      'If-Range': 'Sat, 01 Jan 2000 00:00:00 GMT', 'Range': 'bytes=1-3'}
                        status, metadata, body = request(port, method, prefix + path, conditions)
                        assert status == 200 and metadata['cache-control'] == 'no-store' and 'content-range' not in metadata
                        assert int(metadata['content-length']) == len(expected) and metadata['content-type'] == headers['content-type']
                        assert body == (b'' if method == 'HEAD' else expected)
                    elif path != 'client.swf':
                        assert request(port, method, prefix + path, {'If-Modified-Since': headers['last-modified']})[0] == 304
                        assert request(port, method, prefix + path, {'If-Modified-Since': 'Thu, 01 Jan 2099 00:00:00 GMT'})[0] == 304
                        status, metadata, body = request(port, method, prefix + path, {'Range': 'bytes=1-3'})
                        assert status == 206 and 'content-range' in metadata and body == (b'' if method == 'HEAD' else expected[1:4])
            status, headers, _ = request(port, 'GET', prefix + 'missing.js')
            assert status == 404
            if desktop: assert headers['cache-control'] == 'no-store'
            status, headers, body = request(port, 'POST', '/legacy-api/account/verify',
                                            {'Origin': origin, 'Content-Type': 'application/x-www-form-urlencoded'}, 'guid=synthetic-http&password=')
            assert status == 200 and headers['content-type'] == 'application/xml' and body.startswith(b'<Error>')
            assert request(port, 'GET', '/game', {'Origin': origin, 'Connection': 'Upgrade', 'Upgrade': 'websocket',
                                                 'Sec-WebSocket-Version': '13', 'Sec-WebSocket-Key': 'dGhlIHNhbXBsZSBub25jZQ=='})[0] == 101
            if desktop: child.stdin.write(b'stop\n'); child.stdin.flush()
            else: child.send_signal(signal.SIGTERM)
            assert child.wait(timeout=30) == 0
            assert f'Open: {origin}{prefix}' in (out / f'{label}.log').read_text()
            result['cases'].append(dict(name=label, process=child.pid, generation=selected, entry=origin+prefix, assets=records,
                                        ready=True, exit=child.returncode, root_api_and_ws=True))
        finally:
            if child.poll() is None:
                if desktop: child.stdin.write(b'stop\n'); child.stdin.flush()
                else: child.send_signal(signal.SIGTERM)
                try: child.wait(timeout=30)
                except BaseException:
                    child.kill(); child.wait(); raise RuntimeError('forced cleanup is not qualification')

run('web-root', False)
run('desktop-legacy-root', True)
first = generation('A'); run('desktop-qualified-A', True, first)
path = client / 'launcher.js'; path.write_bytes(b'SYNTHETIC HTTP FIXTURE B: deliberately changed launcher body, NOT upstream source')
os.utime(path, (946684800, 946684800)); assets['launcher.js'] = path.read_bytes()
second = generation('B'); assert second != first
run('desktop-qualified-B', True, second, first)
snapshot = out / 'verified-stopped-game.db'
for values in [('snapshot', state, repo / 'data', snapshot), ('verify-snapshot', snapshot, repo / 'data')]:
    command = [str(binary), '--local-save', *map(str, values)]
    completed = subprocess.run(command, capture_output=True)
    assert completed.returncode == 0, completed.stderr.decode()
    value = json.loads(completed.stdout)
    if values[0] == 'verify-snapshot': assert value['verified'] is True and value['schema'] == 22
    result.setdefault('current_cli', []).append(dict(operation=values[0], exit=completed.returncode, response=value))
assert sha(reference.read_bytes()) == reference_sha
(out / 'desktop-client-cache-result.json').write_text(json.dumps(result, indent=2)+'\n')
print(json.dumps(result, indent=2))

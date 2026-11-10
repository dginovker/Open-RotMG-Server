#!/usr/bin/env python3
"""Owned current-CLI/desktop startup backup boundary proof, without browser or gameplay."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import time
import urllib.error
import urllib.request

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--client', type=Path, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
out = Path(os.environ['QA_RESULTS_DIR'])
binary = out / 'openrotmg'
os.link(args.binary.resolve(), binary)
data = root / 'data'
http = urllib.request.build_opener(urllib.request.ProxyHandler({}))
report = dict(binary=str(binary), sha256=hashlib.sha256(binary.read_bytes()).hexdigest(), cases=[])

def cli(*values, password=None, success=True):
    result = subprocess.run([str(binary), '--local-save', *map(str, values)], input=password, capture_output=True, cwd=out)
    assert (result.returncode == 0) == success, result.stderr.decode()
    assert b'private-startup-password' not in result.stdout + result.stderr
    value = json.loads(result.stdout)
    if not success:
        assert value['protocol'] == 1 and value['operation'] == values[0] and value['outcome'] == 'unchanged'
    return value

def rows(path):
    with sqlite3.connect(f'file:{path}?mode=ro', uri=True) as db:
        return list(db.iterdump()), db.execute('PRAGMA user_version').fetchone()[0]

def fixture(name):
    state = out / name
    subprocess.run([str(binary), '--backup-fixture', str(state), str(data)], check=True, capture_output=True)
    return state

def run(name, state, backup, *, success, schema=None):
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0)); port = probe.getsockname()[1]
    origin = f'http://127.0.0.1:{port}'
    config = out / f'{name}.json'
    value = dict(bind=f'127.0.0.1:{port}', origin=origin, data=str(data), client=str(args.client.resolve()), state=str(state), max_players=85, behind_proxy=False, shore_bots=False, bot_llm=False, realm_seed=42)
    if backup is not None:
        value['pre_migration_backup'] = str(backup)
    config.write_text(json.dumps(value))
    ack_path = Path(str(backup) + '.ack.json') if backup is not None else None
    before = rows(state / 'game.db') if (state / 'game.db').is_file() and name not in ['corrupt', 'foreign', 'unreadable', 'source-symlink'] else None
    with (out / f'{name}.log').open('w') as log:
        child = subprocess.Popen([str(binary), '--desktop', '--config', str(config)], stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT)
        try:
            deadline = time.monotonic() + 180
            health = None
            while child.poll() is None:
                try:
                    with http.open(origin + '/health', timeout=1) as response:
                        health = json.load(response)
                    assert health['process'] == child.pid
                    break
                except (urllib.error.URLError, TimeoutError):
                    assert time.monotonic() < deadline, 'startup deadline'
                    time.sleep(.05)
            if not success:
                assert child.wait(timeout=30) != 0
                assert health is None
                if before is not None:
                    assert rows(state / 'game.db') == before
                report['cases'].append(dict(name=name, exit=child.returncode, ready=False))
                return
            assert health is not None and child.poll() is None
            if backup is not None:
                raw = ack_path.read_bytes(); assert len(raw) <= 16384
                ack = json.loads(raw)
                assert set(ack) == {'protocol', 'operation', 'process', 'backup', 'sha256', 'source_schema', 'target_schema', 'outcome'}
                assert ack['protocol'] == 1 and ack['operation'] == 'pre-migration' and ack['process'] == child.pid
                assert ack['backup'] == str(backup) and ack['target_schema'] == 23 and ack['source_schema'] == schema
                if schema is None:
                    assert ack['outcome'] == 'no-game-db' and ack['sha256'] is None and not backup.exists()
                else:
                    assert ack['outcome'] == 'verified-backup' and len(ack['sha256']) == 64
                    assert ack['sha256'] == hashlib.sha256(backup.read_bytes()).hexdigest()
                    assert rows(backup) == before
                assert rows(state / 'game.db')[1] == 23
                refused = cli('snapshot', state, data, out / f'{name}-peer.db', success=False)
                assert not (out / f'{name}-peer.db').exists()
                report['cases'].append(dict(name=name, ack=ack, ready=True, concurrentCli=refused))
            else:
                report['cases'].append(dict(name=name, ready=True, legacyAbsentField=True))
            child.stdin.write(b'stop\n'); child.stdin.flush(); assert child.wait(timeout=30) == 0
            report['cases'][-1]['exit'] = child.returncode
        finally:
            if child.poll() is None:
                child.stdin.write(b'stop\n'); child.stdin.flush()
                try:
                    assert child.wait(timeout=30) == 0
                except BaseException:
                    child.kill(); child.wait(); raise RuntimeError('forced cleanup is not qualification')

backups = out / 'backups'; backups.mkdir(mode=0o700)
state = fixture('schema21')
with sqlite3.connect(state / 'game.db') as db:
    db.executescript('ALTER TABLE collections DROP COLUMN source; DROP TABLE portable_history; DROP TABLE portable_consumed_levels; DROP TABLE portable_assists; DROP TABLE account_origins; DROP TABLE save_namespace; PRAGMA user_version=21;')
run('schema21', state, backups / 'schema21.db', success=True, schema=21)
run('schema23-unconditional', state, backups / 'schema23.db', success=True, schema=23)
old22 = fixture('schema22')
with sqlite3.connect(old22 / 'game.db') as db:
    db.executescript('ALTER TABLE collections DROP COLUMN source; PRAGMA user_version=22;')
run('schema22', old22, backups / 'schema22.db', success=True, schema=22)
empty = out / 'empty'
run('fresh', empty, backups / 'no-game.db', success=True)
assert not (backups / 'no-game.db').exists()
run('legacy', empty, None, success=True)
# A legitimate CLI restore after an earlier stopped snapshot must be captured afresh.
changed = fixture('external-cli')
earlier = backups / 'earlier.db'; cli('snapshot', changed, data, earlier)
portable = out / 'personal.json'; cli('export', changed, data, 2, portable)
external = out / 'external-input'
p = cli('preview', external, data, portable)
cli('import', external, data, portable, backups / 'unused.db', p['expected'], password=b'private-startup-password\n')
newer = backups / 'newer.db'; cli('snapshot', external, data, newer)
cli('restore', changed, data, newer, backups / 'external-pre-restore.db')
assert rows(changed / 'game.db') == rows(newer) and rows(newer) != rows(earlier)
run('external-cli', changed, backups / 'freshest.db', success=True, schema=23)
assert rows(backups / 'freshest.db') == rows(newer) != rows(earlier)
# Actual process refusals preserve source and never become ready.
for name in ['ack-exists', 'backup-exists', 'inside-state', 'alias-state', 'unsupported', 'foreign', 'corrupt', 'unreadable', 'source-symlink', 'backup-symlink', 'ack-symlink', 'backup-unwritable']:
    state = fixture(name)
    backup = backups / f'{name}.db'
    if name == 'ack-exists': Path(str(backup)+'.ack.json').write_text('held ack')
    if name == 'backup-exists': backup.write_bytes(b'held backup')
    if name == 'inside-state': backup = state / 'inside.db'
    if name == 'alias-state':
        alias = out / 'state-alias'; alias.symlink_to(state, target_is_directory=True); backup = alias / 'inside.db'
    if name == 'unsupported':
        with sqlite3.connect(state / 'game.db') as db: db.execute('PRAGMA user_version=24')
    if name == 'foreign':
        (state / 'game.db').unlink()
        with sqlite3.connect(state / 'game.db') as db: db.executescript('CREATE TABLE foreign_data(x); PRAGMA user_version=22;')
    if name == 'corrupt': (state / 'game.db').write_bytes(b'not SQLite')
    if name == 'unreadable':
        unreadable_original = (state / 'game.db').read_bytes(); (state / 'game.db').chmod(0)
    if name == 'source-symlink':
        (state / 'game.db').rename(state / 'held.db'); (state / 'game.db').symlink_to(state / 'held.db')
    if name == 'backup-symlink': backup.symlink_to(out / 'absent')
    if name == 'ack-symlink': Path(str(backup)+'.ack.json').symlink_to(out / 'absent')
    if name == 'backup-unwritable':
        blocked = out / 'blocked-backups'; blocked.mkdir(mode=0o500); backup = blocked / 'required.db'
    before_bytes = unreadable_original if name == 'unreadable' else (state / 'game.db').read_bytes()
    run(name, state, backup, success=False)
    if name == 'unreadable': (state / 'game.db').chmod(0o600)
    assert (state / 'game.db').read_bytes() == before_bytes
    if name == 'ack-exists': assert Path(str(backup)+'.ack.json').read_text() == 'held ack' and not backup.exists()
    if name == 'backup-exists': assert backup.read_bytes() == b'held backup'
    if name == 'backup-unwritable': blocked.chmod(0o700)
for sidecar in ['wal', 'shm', 'journal']:
    state = out / ('orphan-' + sidecar); state.mkdir(); (state / ('game.db-' + sidecar)).write_bytes(b'held progress')
    run('orphan-' + sidecar, state, backups / ('orphan-' + sidecar + '.db'), success=False)
(out / 'startup-backup-result.json').write_text(json.dumps(report, indent=2)+'\n')
print(json.dumps(report, indent=2))

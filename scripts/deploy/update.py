#!/usr/bin/python3 -I
import ctypes
import datetime
import fcntl
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import shutil
import signal
import sqlite3
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request
from contextlib import closing

REPO = 'https://github.com/OpenRotMG/Open-RotMG-Server'
LIVE = Path('/opt/openrotmg/server')
ROOT = Path('/var/lib/openrotmg-deploy')
BACKUPS = Path('/opt/openrotmg-backup')
CONFIG = Path('/etc/openrotmg/config.json')
ANNOUNCE_TOKEN = Path('/etc/openrotmg/discord-link-token')
RESTART_NOTICE_SECONDS = 20
LAST_BACKUP = Path('/var/lib/openrotmg-backup/last-success.json')
LOCK = Path('/run/lock/openrotmg-deploy.lock')
LIMIT = 64 * 1024 * 1024
sys.dont_write_bytecode = True
os.environ['PYTHONDONTWRITEBYTECODE'] = '1'


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def files(root):
    return {str(p.relative_to(root)): digest(p) for p in root.rglob('*') if p.is_file()}


def outcome(work, state, stage=None, reason=None):
    identity = list((stage.stat().st_dev, stage.stat().st_ino)) if stage is not None and stage.exists() else None
    value = {'format': 1, 'state': state, 'stage': str(stage) if stage is not None else None,
             'stage_identity': identity, 'reason': reason}
    temporary = work / 'outcome.json.new'
    temporary.write_text(json.dumps(value) + '\n')
    temporary.replace(work / 'outcome.json')


def active_paths():
    paths = set()
    for process in Path('/proc').glob('[0-9]*'):
        for link in [process / 'exe', process / 'cwd', *list((process / 'fd').glob('*'))]:
            try:
                target = Path(os.readlink(link))
            except (FileNotFoundError, ProcessLookupError):
                continue
            if target.is_absolute():
                paths.add(target)
        try:
            paths.update(Path(os.fsdecode(arg)) for arg in (process / 'cmdline').read_bytes().split(b'\0') if arg.startswith(b'/'))
        except (FileNotFoundError, ProcessLookupError):
            continue
    return paths


def retain_deployments(apply=True):
    attempts, metadata, pinned = {}, {}, []
    for work in sorted(ROOT.iterdir(), key=lambda p: p.name):
        if not re.fullmatch('[0-9a-f]{40}-[0-9]+', work.name):
            continue
        require(work.is_dir() and not work.is_symlink(), 'Unsafe deployment workspace: ' + str(work))
        if (work / 'release.json').exists():
            value = json.loads((work / 'release.json').read_text())
            require(value['revision'] == work.name[:40] and value['format'] == 1, 'Invalid retained release metadata')
            if value['revision'] in metadata:
                require(metadata[value['revision']]['manifest'] == value['manifest'], 'Conflicting release metadata')
            metadata[value['revision']] = value
        receipt = json.loads((work / 'outcome.json').read_text()) if (work / 'outcome.json').exists() else None
        if receipt is not None:
            require(receipt['format'] == 1 and receipt['state'] in {'preparing', 'cutover', 'success', 'failed', 'pinned', 'artifact_only'},
                    'Invalid deployment outcome: ' + str(work))
            if receipt['state'] == 'success':
                require(receipt['stage'] is not None and (work / 'previous-server-path').read_text().strip() == receipt['stage']
                        and all((work / 'cutover' / n).is_file() for n in ['game.db', 'server.pem']),
                        'Invalid successful cutover receipt: ' + str(work))
        attempts[work] = receipt
    live_revision = (LIVE / 'REVISION').read_text().strip()
    keep = set()
    for state, limit in [('success', 2), ('failed', 3)]:
        ordered = sorted((w for w, r in attempts.items() if r is not None and r['state'] == state),
                         key=lambda w: int(w.name.rsplit('-', 1)[1]), reverse=True)
        keep.update(ordered[:limit])
    # Two completed cutovers retain the two preceding source trees and frozen saves.
    keep.update(w for w, r in attempts.items() if r is None or r['state'] not in {'success', 'failed', 'artifact_only'}
                or w.name[:40] == live_revision or (w / 'retain').exists() or (w / 'rehearsal-running').exists())
    running = active_paths() if set(attempts) - keep else set()
    keep.update(w for w in attempts if any(w == p or w in p.parents for p in running))
    for work, receipt in attempts.items():
        if work in keep:
            continue
        if receipt['state'] == 'artifact_only':
            require(receipt['stage'] is None and {p.name for p in work.iterdir()} <= {'outcome.json', 'release.json', 'server.tar.gz'},
                    'Artifact-only workspace contains unclassified evidence: ' + str(work))
        if receipt['stage'] is None:
            continue
        stage = Path(receipt['stage'])
        require(stage.parent == LIVE.parent and re.fullmatch('server-stage-[0-9]+', stage.name)
                and not stage.is_symlink(), 'Unsafe retained stage: ' + str(stage))
        if not stage.exists() and receipt['state'] == 'failed':
            continue
        valid = stage.exists() and [stage.stat().st_dev, stage.stat().st_ino] == receipt['stage_identity']
        revision = (stage / 'REVISION').read_text().strip() if valid else None
        valid = valid and revision in metadata and not any(p.is_symlink() for p in stage.rglob('*'))
        if not valid or files(stage) != metadata[revision]['manifest'] or any(stage == p or stage in p.parents for p in running):
            keep.add(work)
            pinned.append({'path': str(work), 'reason': 'stage identity, contents or use requires review'})
    protected = {LIVE}
    revisions = {live_revision}
    for work in keep:
        revisions.add(work.name[:40])
        receipt = attempts[work]
        if receipt is not None and receipt['stage'] is not None:
            protected.add(Path(receipt['stage']))
        if (work / 'previous-server-path').exists():
            protected.add(Path((work / 'previous-server-path').read_text().strip()))
        if (work / 'shadow.json').exists():
            protected.add(Path(json.loads((work / 'shadow.json').read_text())['data']).parent)
    for stage in protected:
        if (stage / 'REVISION').exists():
            revisions.add((stage / 'REVISION').read_text().strip())
    remove = set(attempts) - keep
    for work in remove.copy():
        receipt = attempts[work]
        if receipt['stage'] is not None:
            stage = Path(receipt['stage'])
            if stage in protected:
                remove.discard(work)
                keep.add(work)
                revisions.add(work.name[:40])
                pinned.append({'path': str(work), 'reason': 'stage is referenced by a kept attempt'})
            elif stage.exists():
                remove.add(stage)
    releases = BACKUPS / 'releases'
    current = (BACKUPS / 'current').resolve()
    if attempts and releases.exists():
        require((BACKUPS / 'current').is_symlink() and current.is_dir(), 'Backup tool pointer is invalid')
        for bundle in releases.iterdir():
            if not re.fullmatch('[0-9a-f]{40}', bundle.name) or bundle == current or bundle.name in revisions:
                continue
            if any(bundle == p or bundle in p.parents for p in running):
                continue
            mapping = {'backup.py': 'scripts/backups/backup.py', 'openrotmg': 'openrotmg', 'REVISION': 'REVISION',
                       'openrotmg-backup.sh': 'scripts/backups/openrotmg-backup.sh'}
            manifest = metadata[bundle.name]['manifest'] if bundle.name in metadata else {}
            mapping.update({n: n for n in manifest if n.startswith('data/')})
            valid = all(n in manifest for n in mapping.values()) and bundle.is_dir() and not bundle.is_symlink()
            if not valid or any(p.is_symlink() for p in bundle.rglob('*')) or files(bundle) != {n: manifest[s] for n, s in mapping.items()}:
                pinned.append({'path': str(bundle), 'reason': 'backup tool provenance requires review'})
                continue
            remove.add(bundle)
    result = {'retention': 'applied' if apply else 'dry_run', 'remove': sorted(str(p) for p in remove),
              'kept_attempts': len(keep), 'unresolved_attempts': sum(r is None or r['state'] not in {'success', 'failed', 'artifact_only'}
                  or (w / 'retain').exists() or (w / 'rehearsal-running').exists() for w, r in attempts.items()),
              'review': pinned}
    if apply and attempts:
        for path in sorted(remove, key=str):
            shutil.rmtree(path)
        temporary = ROOT / 'retention.json.new'
        temporary.write_text(json.dumps(result) + '\n')
        temporary.replace(ROOT / 'retention.json')
    print(json.dumps(result), flush=True)
    return result


def status(state, desired, **fields):
    value = {'state': state, 'desired': desired, 'running': (LIVE / 'REVISION').read_text().strip(),
             'checked_at': int(time.time()), **fields}
    temporary = ROOT / 'status.json.new'
    temporary.write_text(json.dumps(value) + '\n')
    temporary.replace(ROOT / 'status.json')
    print(json.dumps(value), flush=True)


def unpack(archive, directory, metadata):
    directory.mkdir(mode=0o755)
    directory.chmod(0o755)
    with tarfile.open(archive) as bundle:
        members = bundle.getmembers()
        require(sum(m.size for m in members) <= LIMIT and len({m.name for m in members}) == len(members),
                'Release exceeds size limit or contains duplicate paths')
        for member in members:
            require(not Path(member.name).is_absolute() and '..' not in Path(member.name).parts
                    and str(Path(member.name)) == member.name and member.isfile(), 'Unsafe release archive member')
        bundle.extractall(directory)
    for path in directory.rglob('*'):
        path.chmod(0o755 if path.is_dir() or path.name == 'openrotmg' or path.suffix == '.sh' else 0o644)
    require(files(directory) == metadata['manifest'], 'Release manifest mismatch')
    require(digest(directory / 'openrotmg') == metadata['binary_sha256'], 'Release binary mismatch')
    require((directory / 'REVISION').read_text().strip() == metadata['revision'], 'Release revision mismatch')


def copy_db(source, target):
    with closing(sqlite3.connect(Path(source).resolve().as_uri() + '?mode=ro', uri=True)) as src, closing(sqlite3.connect(target)) as dst:
        src.backup(dst)
    target.chmod(0o600)


def rows(path):
    with closing(sqlite3.connect(Path(path).resolve().as_uri() + '?mode=ro', uri=True)) as db:
        db.row_factory = sqlite3.Row
        db.execute('BEGIN')
        require(db.execute('PRAGMA integrity_check').fetchone()[0] == 'ok', 'SQLite integrity failed')
        require(not db.execute('PRAGMA foreign_key_check').fetchall(), 'SQLite foreign keys failed')
        tables = [r[0] for r in db.execute("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")]
        return {table: [dict(row) for row in db.execute('SELECT * FROM "' + table.replace('"', '""') + '" ORDER BY 1')] for table in tables}


def verify_migration(before, after):
    # Only old lazy Vaults gain intentionally provisioned rows; current paid state is never repaired.
    if 'vaults' in before and all('vault_mask' not in row for row in before.get('accounts', [])):
        existing = {row['account'] for row in before['vaults']}
        expected = [{**row, 'chest': 0} for row in before['vaults']]
        expected += [{'account': row['id'], 'chest': 0, 'items': '[-1,-1,-1,-1,-1,-1,-1,-1]', 'revision': 0}
                     for row in before.get('accounts', []) if row['id'] not in existing]
        require(sorted(after.get('vaults', []), key=lambda row: row['account'])
                == sorted(expected, key=lambda row: row['account']), 'Vault migration changed stored items or ownership')
        before = {**before, 'vaults': sorted(expected, key=lambda row: row['account'])}
    require(all(table in after and len(data) == len(after[table])
                and all(a.items() <= b.items() for a, b in zip(data, after[table]))
                for table, data in before.items()), 'Migration changed existing records')


def ready(port, process=None):
    deadline = time.monotonic() + 45
    while True:
        if process is not None:
            require(process.poll() is None, 'Rehearsal exited before readiness; inspect shadow.log')
        try:
            with urllib.request.urlopen(f'http://127.0.0.1:{port}/health', timeout=3) as response:
                health = json.load(response)
            require(type(health['uptime_seconds']) is int, 'Invalid server health')
            return health
        except OSError:
            require(time.monotonic() < deadline, 'Server readiness deadline exceeded')
            time.sleep(.2)


def backup_module(stage):
    os.environ['OPENROTMG_BACKUP_BINARY'] = str(stage / 'openrotmg')
    spec = importlib.util.spec_from_file_location('release_backup', stage / 'scripts/backups/backup.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def verify_backup(started):
    proof = json.loads(LAST_BACKUP.read_text())
    uploaded = datetime.datetime.fromisoformat(proof['uploaded_at']).timestamp()
    require(proof.get('restored') is True and uploaded >= started
            and proof['revisions']['server'] == (LIVE / 'REVISION').read_text().strip(),
            'Fresh uploaded backup and exact restore were not verified')


def start_backup():
    require(subprocess.check_output(['systemctl', 'show', 'openrotmg-backup', '-p', 'Result', '--value'], text=True).strip() == 'success',
            'Previous backup failed; resolve it or await its scheduled retry')
    # Successful deployment backups must not consume the scheduled job's failure-retry budget.
    run('systemctl', 'reset-failed', 'openrotmg-backup.service')
    started = time.time()
    run('systemctl', 'start', 'openrotmg-backup.service')
    require(subprocess.check_output(['systemctl', 'show', 'openrotmg-backup', '-p', 'Result', '--value'], text=True).strip() == 'success', 'Independent GitHub backup failed')
    verify_backup(started)


def install_backups(stage):
    state = subprocess.check_output(['systemctl', 'show', 'openrotmg-backup', '-p', 'ActiveState', '--value'], text=True).strip()
    require(state == 'inactive', f'Backup service is {state}; inspect its journal before retrying')
    run('runuser', '-u', 'openrotmg', '--', sys.executable, '-B', '-W', 'error::ResourceWarning',
        '-m', 'unittest', 'discover', '-s', str(stage / 'scripts/backups'), '-p', 'test_*.py')
    revision = (stage / 'REVISION').read_text().strip()
    releases = BACKUPS / 'releases'
    releases.mkdir(exist_ok=True)
    bundle = releases / revision
    sources = {'backup.py': stage / 'scripts/backups/backup.py', 'openrotmg': stage / 'openrotmg',
               'REVISION': stage / 'REVISION', 'openrotmg-backup.sh': stage / 'scripts/backups/openrotmg-backup.sh'}
    sources.update({str(path.relative_to(stage)): path for path in (stage / 'data').rglob('*') if path.is_file()})
    if not bundle.exists():
        temporary = releases / (revision + '.new')
        temporary.mkdir()
        for name, source in sources.items():
            target = temporary / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(source.read_bytes())
            target.chmod(0o755 if name in {'openrotmg', 'openrotmg-backup.sh'} else 0o644)
        temporary.rename(bundle)
    require(files(bundle) == {name: digest(path) for name, path in sources.items()}, 'Backup tool bundle differs from release')
    pointer = BACKUPS / 'current.new'
    pointer.symlink_to(bundle)
    pointer.replace(BACKUPS / 'current')
    wrapper = BACKUPS / 'run.sh.new'
    wrapper.write_text('#!/bin/sh\nset -eu\nexec /opt/openrotmg-backup/current/openrotmg-backup.sh\n')
    wrapper.chmod(0o755)
    wrapper.replace(BACKUPS / 'run.sh')
    start_backup()


def announce(config, text):
    token = ANNOUNCE_TOKEN.read_text().strip()
    request = urllib.request.Request(
        'http://127.0.0.1:' + config['bind'].rsplit(':', 1)[1] + '/internal/discord/announce',
        data=json.dumps({'text': text}).encode(), headers={'authorization': 'Bearer ' + token, 'content-type': 'application/json'})
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        error.close()
        if error.code == 404:
            return None
        raise RuntimeError('Announcement rejected: HTTP ' + str(error.code)) from error


def warn_restart(config):
    state = subprocess.check_output(['systemctl', 'show', 'openrotmg', '-p', 'ActiveState', '--value'], text=True).strip()
    if state != 'active':
        print(json.dumps({'restart_warning': 'skipped', 'service_state': state}), flush=True)
        return False
    notice = 'Server restarting in ' + str(RESTART_NOTICE_SECONDS) + ' seconds to apply an update.'
    result = announce(config, notice)
    if result is None:
        print(json.dumps({'restart_warning': 'unsupported', 'detail': 'running server predates the announcement endpoint'}), flush=True)
        return False
    print(json.dumps({'restart_warning': notice, 'seconds': RESTART_NOTICE_SECONDS, **result}), flush=True)
    time.sleep(RESTART_NOTICE_SECONDS)
    return True


def rehearse(stage, work, config):
    run(sys.executable, '-B', str(stage / 'scripts/backups/release_contract.py'),
        str(stage / 'openrotmg'), str(stage / 'data'), str(work / 'writer-contract'),
        (stage / 'REVISION').read_text().strip())
    shadow = work / 'shadow'
    shadow.mkdir(mode=0o700)
    copy_db(Path(config['state']) / 'game.db', shadow / 'game.db')
    shutil.copyfile(Path(config['state']) / 'server.pem', shadow / 'server.pem')
    (shadow / 'server.pem').chmod(0o600)
    before = rows(shadow / 'game.db')
    cfg = {**config, 'bind': '127.0.0.1:18939', 'data': str(stage / 'data'),
           'state': str(shadow), 'shore_bots': False, 'bot_llm': False}
    path = work / 'shadow.json'
    path.write_text(json.dumps(cfg))
    with (work / 'shadow.log').open('w') as log:
        # An unconfirmed child exit must retain its executable and diagnostics.
        running = work / 'rehearsal-running'
        running.touch()
        process = subprocess.Popen([str(stage / 'openrotmg'), '--config', str(path)],
                                   stdout=log, stderr=subprocess.STDOUT)
        try:
            health = ready(18939, process)
            require(health['players'] == health['bots'] == health['qa_actors'] == 0, 'Unexpected rehearsal actors')
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
            process.wait(timeout=20)
            running.unlink()
        require(process.returncode == 0, 'Rehearsal shutdown failed')
    after = rows(shadow / 'game.db')
    verify_migration(before, after)
    module = backup_module(stage)
    archive = work / 'shadow-export.json.gz'
    bundle = module.export(shadow / 'game.db', '/var/lib/openrotmg-discord-bot/levels.sqlite', archive,
                           stage / 'REVISION', '/opt/openrotmg-discord-bot/REVISION', stage / 'REVISION')
    restored = work / 'restored'
    module.restore(archive, restored)
    run(str(stage / 'openrotmg'), '--backup-load', str(restored / 'game.db'), str(stage / 'data'))
    require(module.snapshot(restored / 'game.db', module.RELOADED_GAME) == bundle['game']
            and module.snapshot(restored / 'levels.sqlite', module.DISCORD) == bundle['discord'], 'Credential-free restore mismatch')


def deploy(stage, work, metadata, config):
    try:
        run('runuser', '-u', 'openrotmg', '--', 'test', '-x', str(stage / 'openrotmg'))
        rehearse(stage, work, config)
    except Exception:
        require(stage.parent == LIVE.parent and stage.name.startswith('server-stage-') and not stage.is_symlink(),
                'Refusing cleanup outside the owned candidate directory')
        reason = ('rehearsal exit unconfirmed' if (work / 'rehearsal-running').exists()
                  else 'release files changed' if files(stage) != metadata['manifest'] else None)
        if reason is None:
            shutil.rmtree(stage)
        print(json.dumps({'candidate_cleanup': 'removed' if reason is None else 'retained',
                          'stage': str(stage), 'reason': reason, 'recovery_directory': str(work)}), flush=True)
        raise
    install_backups(stage)
    require(files(stage) == metadata['manifest'], 'Rehearsal changed release files')
    require(main_revision() == metadata['revision'], 'Main advanced during rehearsal; no cutover performed')
    with LAST_BACKUP.with_name('backup.lock').open('rb') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        warn_restart(config)
        outcome(work, 'cutover', stage)
        run('systemctl', 'stop', 'openrotmg.service')
        require(subprocess.check_output(['systemctl', 'show', 'openrotmg', '-p', 'Result', '--value'], text=True).strip() == 'success', 'Shutdown save failed; no source exchange performed')
        frozen = work / 'cutover'
        frozen.mkdir(mode=0o700)
        copy_db(Path(config['state']) / 'game.db', frozen / 'game.db')
        shutil.copyfile(Path(config['state']) / 'server.pem', frozen / 'server.pem')
        (frozen / 'server.pem').chmod(0o600)
        libc = ctypes.CDLL(None, use_errno=True)
        require(libc.renameat2(-100, os.fsencode(LIVE), -100, os.fsencode(stage), 2) == 0,
                'Atomic source exchange failed; frozen recovery state retained')
        (work / 'previous-server-path').write_text(str(stage) + '\n')
        run('runuser', '-u', 'openrotmg', '--', 'test', '-x', str(LIVE / 'openrotmg'))
        run('systemctl', 'start', 'openrotmg.service')
        ready(int(config['bind'].rsplit(':', 1)[1]))
        require((LIVE / 'REVISION').read_text().strip() == metadata['revision'], 'Activated revision mismatch')
        require(digest(LIVE / 'openrotmg') == metadata['binary_sha256'], 'Activated binary mismatch')
        pid = int(subprocess.check_output(['systemctl', 'show', 'openrotmg', '-p', 'MainPID', '--value'], text=True))
        require(pid > 0 and digest(f'/proc/{pid}/exe') == metadata['binary_sha256'], 'Running process binary mismatch')
    start_backup()


def main_revision():
    revision = subprocess.check_output(['git', 'ls-remote', '--exit-code', REPO + '.git', 'refs/heads/main'], text=True, timeout=30).split()[0]
    require(re.fullmatch('[0-9a-f]{40}', revision), 'Invalid main revision')
    return revision


def main(desired, raw):
    require(re.fullmatch('[0-9a-f]{40}', desired), 'Only an exact server commit SHA is accepted')
    require(main_revision() == desired, 'Main advanced; refusing stale server deployment')
    require(len(raw) <= LIMIT, 'Deployment archive exceeds 64 MiB')
    ROOT.mkdir(mode=0o700, exist_ok=True)
    retain_deployments()
    require(shutil.disk_usage(ROOT).free >= 2 * 1024**3, 'Less than 2 GiB free; inspect retained recovery data before deploying')
    work = None
    try:
        with tarfile.open(fileobj=io.BytesIO(raw), mode='r:') as archive:
            members = archive.getmembers()
            require(sorted(m.name for m in members) == ['release.json', 'server.tar.gz']
                    and all(m.isfile() for m in members) and sum(m.size for m in members) <= LIMIT,
                    'Expected only release.json and server.tar.gz')
            release = archive.extractfile('release.json').read()
            package = archive.extractfile('server.tar.gz').read()
        metadata = json.loads(release)
        require(metadata['format'] == 1 and metadata['revision'] == desired, 'Artifact source mismatch')
        require(hashlib.sha256(package).hexdigest() == metadata['archive_sha256'], 'Artifact archive mismatch')
        config = json.loads(CONFIG.read_text())
        if (LIVE / 'REVISION').read_text().strip() == desired:
            require(files(LIVE) == metadata['manifest'], 'Already deployed server differs from this artifact')
            pid = int(subprocess.check_output(['systemctl', 'show', 'openrotmg', '-p', 'MainPID', '--value'], text=True))
            require(pid > 0 and digest(f'/proc/{pid}/exe') == metadata['binary_sha256'], 'Running process binary mismatch')
            ready(int(config['bind'].rsplit(':', 1)[1]))
            verify_backup(0)
            status('up_to_date', desired)
            return
        work = ROOT / (desired + '-' + str(time.time_ns()))
        work.mkdir(mode=0o700)
        (work / 'release.json').write_bytes(release)
        (work / 'server.tar.gz').write_bytes(package)
        del release, package
        outcome(work, 'preparing')
        stage = LIVE.with_name('server-stage-' + str(time.time_ns()))
        unpack(work / 'server.tar.gz', stage, metadata)
        outcome(work, 'preparing', stage)
        status('rehearsing', desired)
        deploy(stage, work, metadata, config)
        outcome(work, 'success', stage)
        retain_deployments()
        status('deployed', desired, recovery_directory=str(work), backup=json.loads(LAST_BACKUP.read_text()))
    except Exception as error:
        if work is not None and (work / 'outcome.json').exists():
            receipt = json.loads((work / 'outcome.json').read_text())
            if receipt['state'] != 'success':
                stage = Path(receipt['stage']) if receipt['stage'] is not None else None
                safe = receipt['state'] == 'preparing' and stage is not None and not (work / 'rehearsal-running').exists()
                safe = safe and (not stage.exists() or files(stage) == metadata['manifest'])
                outcome(work, 'failed' if safe else 'pinned', stage, str(error))
            retain_deployments()
        activated = (LIVE / 'REVISION').read_text().strip() == desired
        state = 'activated_unverified' if activated else 'failed'
        status(state, desired, recovery_directory=str(work) if work is not None else None, error=str(error))
        raise


if __name__ == '__main__':
    os.umask(0o077)
    require(len(sys.argv) == 2, 'Expected the server commit SHA')
    raw = sys.stdin.buffer.read(LIMIT + 1)
    with LOCK.open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        main(sys.argv[1], raw)

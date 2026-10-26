#!/usr/bin/env python3
"""Repo-owned Linux QA jobs: durable results, cgroup-owned disposable scratch."""
import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import stat
import subprocess
import sys
import time
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]
MARKER = 'OpenRotMG QA jobs v1\n'
SCRATCH_MARKER = 'OpenRotMG disposable QA scratch v1\n'
ID = re.compile(r'[0-9a-f]{32}')


def private(path):
    info = path.lstat()
    if path.resolve() != path or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise RuntimeError(f'Expected a canonical private owned directory: {path}')


def save(path, value):
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, prefix='.record-', delete=False) as stream:
        temporary = Path(stream.name)
        json.dump(value, stream, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


@contextmanager
def lock(path):
    fd = os.open(path, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1 or info.st_mode & 0o077:
            raise RuntimeError(f'Unsafe job lock: {path}')
        fcntl.flock(fd, fcntl.LOCK_EX)
        yield
    finally:
        os.close(fd)


def owned_root(root, marker):
    if not root.is_absolute() or root.resolve() != root or root.is_relative_to(ROOT):
        raise RuntimeError('QA storage must be canonical, absolute and outside the checkout')
    root.parent.mkdir(parents=True, exist_ok=True)
    with lock(root.parent / ('.' + root.name + '.lock')):
        if not root.exists():
            root.mkdir(mode=0o700)
            (root / '.owner').write_text(marker)
        private(root)
        if (root / '.owner').is_symlink() or (root / '.owner').read_text() != marker:
            raise RuntimeError(f'Refusing to adopt an existing QA directory: {root}')
    return root


def jobs_root():
    base = Path(os.environ.get('XDG_STATE_HOME', Path.home() / '.local/state'))
    return owned_root(base / 'openrotmg/qa-jobs', MARKER)


def scratch_root():
    base = Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache'))
    root = Path(os.environ.get('OPENROTMG_QA_SCRATCH_ROOT', base / 'oq'))
    # Chromium appends a SingletonSocket suffix within Linux's 108-byte Unix socket limit.
    if len(os.fsencode(str(root / ('0' * 32)))) > 60:
        raise RuntimeError('QA scratch path is too long; set OPENROTMG_QA_SCRATCH_ROOT to a short on-disk path')
    root = owned_root(root, SCRATCH_MARKER)
    filesystem = subprocess.check_output(['stat', '--file-system', '--format=%T', str(root)], text=True).strip()
    if filesystem in ('tmpfs', 'ramfs', 'devtmpfs', 'hugetlbfs'):
        raise RuntimeError(f'QA scratch must be disk-backed, not {filesystem}: {root}')
    return root


def scratch_path(directory, row):
    if row['version'] == 1:
        return directory / 'scratch'
    root = Path(row['scratch_root'])
    private(root)
    if (root / '.owner').is_symlink() or (root / '.owner').read_text() != SCRATCH_MARKER:
        raise RuntimeError(f'Unrecognized scratch ownership: {root}')
    return root / row['id']


def load(directory):
    private(directory.parent)
    private(directory)
    if not ID.fullmatch(directory.name) or directory.parent.name != 'qa-jobs':
        raise RuntimeError(f'Not a managed QA job: {directory}')
    if (directory.parent / '.owner').read_text() != MARKER or (directory / 'job.json').is_symlink():
        raise RuntimeError(f'Unrecognized QA ownership: {directory}')
    row = json.loads((directory / 'job.json').read_text())
    unit = f'openrotmg-cache-{row["service"]}.service' if row.get('service') else f'openrotmg-qa-{directory.name}.service'
    if row['version'] not in (1, 2) or row['id'] != directory.name or row['uid'] != os.getuid() or row['unit'] != unit:
        raise RuntimeError(f'Invalid QA job identity: {directory}')
    return row


def cgroup():
    unified = [line[3:] for line in Path('/proc/self/cgroup').read_text().splitlines() if line.startswith('0::/')]
    if len(unified) != 1:
        raise RuntimeError('QA requires Linux cgroup v2')
    return unified[0]


def group_processes(group):
    if not group.startswith('/') or '..' in Path(group).parts:
        raise RuntimeError(f'Invalid job cgroup: {group}')
    path = Path('/sys/fs/cgroup') / group.lstrip('/')
    try:
        path.lstat()
    except FileNotFoundError:
        return set()
    found = set()
    for parent, dirs, files in os.walk(path, onerror=lambda error: (_ for _ in ()).throw(error)):
        if 'cgroup.procs' not in files:
            raise RuntimeError(f'Missing cgroup process proof: {parent}')
        found.update(int(pid) for pid in (Path(parent) / 'cgroup.procs').read_text().split())
    return found


def unit_state(unit):
    result = subprocess.run(['systemctl', '--user', 'show', unit, '--property=LoadState,ActiveState,InvocationID'],
                            capture_output=True, text=True)
    values = dict(line.split('=', 1) for line in result.stdout.splitlines() if '=' in line)
    if result.returncode or not {'LoadState', 'ActiveState', 'InvocationID'} <= values.keys():
        raise RuntimeError(f'Cannot establish job service state: {unit}: {result.stderr.strip()}')
    return values


def preservation(row):
    policy = Path(row['preservation_policy'])
    try:
        text = policy.read_text()
    except FileNotFoundError:
        if row['preservation_required'] or policy.is_symlink():
            raise
        return lambda paths: None
    value = json.loads(text)
    if value.get('version') != 1 or not isinstance(value.get('protectedPaths'), list) or not isinstance(value.get('protectedWords'), list):
        raise RuntimeError(f'Invalid preservation policy: {policy}')
    protected = [Path(p) for p in value['protectedPaths']]
    words = value['protectedWords']
    if any(not p.is_absolute() or '..' in p.parts for p in protected) or any(not isinstance(w, str) or not w for w in words):
        raise RuntimeError(f'Invalid preservation paths/words: {policy}')
    def held(paths):
        for path in paths:
            if any(path.is_relative_to(p) or p.is_relative_to(path) for p in protected) or any(w.casefold() in str(path).casefold() for w in words):
                return f'Configured preservation hold: {path}'
        return None
    return held


def collect(directory, stopping=False):
    row = load(directory)
    with lock(directory / '.lock'):
        receipt = directory / 'collection.json'
        if receipt.exists() and json.loads(receipt.read_text())['status'] == 'reclaimed':
            return
        try:
            service_result = os.environ['SERVICE_RESULT'] if stopping else 'orphan-recovery'
            if 'cgroup' not in row:
                if stopping:
                    save(receipt, {'status': 'reclaimed', 'reason': 'Worker failed before allocating scratch',
                                   'service_result': service_result})
                return
            if stopping:
                if cgroup() != row['cgroup'] or os.environ['INVOCATION_ID'] != row['invocation']:
                    raise RuntimeError('Stop collector does not own this service invocation')
                allowed = {os.getpid()}
            else:
                service = unit_state(row['unit'])
                if service['ActiveState'] not in ('inactive', 'failed'):
                    return
                if service['InvocationID'] not in ('', row['invocation']):
                    raise RuntimeError('Service invocation identity changed')
                allowed = set()
            live = group_processes(row['cgroup']) - allowed
            if live:
                raise RuntimeError(f'Scratch still has owned processes: {sorted(live)}')
            if (directory / 'preserve-scratch').exists() or (directory / 'preserve-scratch').is_symlink():
                save(receipt, {'status': 'preserved', 'reason': 'Explicit preserve-scratch hold', 'service_result': service_result})
                return
            scratch = scratch_path(directory, row)
            if scratch.exists() or scratch.is_symlink():
                private(scratch)
                if row['version'] == 2 and ((scratch / '.owner').is_symlink() or (scratch / '.owner').read_text() != str(directory)):
                    raise RuntimeError('Scratch creation receipt changed; refusing to adopt it')
                if not shutil.rmtree.avoids_symlink_attacks:
                    raise RuntimeError('Python lacks fd-safe recursive removal')
                device = scratch.stat().st_dev
                protected = preservation(row)
                held = protected([scratch, Path(row['source'])])
                for parent, dirs, files in os.walk(scratch, followlinks=False, onerror=lambda error: (_ for _ in ()).throw(error)):
                    paths = [Path(parent) / name for name in dirs + files]
                    for path in paths:
                        if path.lstat().st_dev != device:
                            raise RuntimeError('Scratch contains another filesystem; preserving it')
                    held = held or protected(paths)
                if held:
                    save(receipt, {'status': 'preserved', 'reason': held, 'service_result': service_result})
                    return
                shutil.rmtree(scratch)
            save(receipt, {'status': 'reclaimed', 'time': time.time(), 'cgroup': row['cgroup'],
                           'service_result': service_result})
        except Exception as error:
            save(receipt, {'status': 'error', 'error': f'{type(error).__name__}: {error}'})
            raise


def recover(root):
    for directory in root.iterdir():
        if not ID.fullmatch(directory.name):
            continue
        try:
            collect(directory)
        except (OSError, ValueError, KeyError, RuntimeError) as error:
            print(f'QA recovery preserved {directory}: {error}', file=sys.stderr)


def command_for(kind, arguments, source, results):
    if kind == 'cargo':
        if not arguments or 'clean' in arguments:
            raise RuntimeError('Choose a Cargo build/test/fmt/check/clippy command, not global clean')
        return [sys.executable, str(source / 'scripts/cargo.py'), *arguments]
    if kind == 'native':
        if not arguments or any(arg.split('=', 1)[0] == '--run-dir' for arg in arguments):
            raise RuntimeError('native requires CASE.mjs; the runner owns --run-dir')
        return ['node', str(source / 'scripts/native-test.mjs'), *arguments, '--run-dir', str(results / 'native')]
    if arguments[:1] == ['--']:
        arguments = arguments[1:]
    if not arguments:
        raise RuntimeError('exec requires a command after --')
    return arguments


def run_job(directory):
    row = load(directory)
    with lock(directory / '.lock'):
        group = cgroup()
        if not group.endswith('/' + row['unit']):
            raise RuntimeError('QA worker is not inside its owned service cgroup')
        row.update(cgroup=group, invocation=os.environ['INVOCATION_ID'])
        save(directory / 'job.json', row)
        # A crash must not leave scratch without a durable owner.
        temporary = scratch_path(directory, row)
        temporary.mkdir(mode=0o700)
        if row['version'] == 2:
            (temporary / '.owner').write_text(str(directory))
    results = directory / 'results'
    env = {**os.environ, 'TMPDIR': str(temporary), 'QA_SCRATCH_DIR': str(temporary),
           'QA_RESULTS_DIR': str(results), 'QA_JOB_DIR': str(directory)}
    with (results / 'stdout.log').open('ab', buffering=0) as out, (results / 'stderr.log').open('ab', buffering=0) as err:
        process = subprocess.Popen(row['command'], cwd=row['source'], env=env, stdin=subprocess.DEVNULL, stdout=out, stderr=err)
        code = process.wait()
    save(directory / 'command.json', {'returncode': code, 'finished': time.time()})
    if code == 0 and row.get('service'):
        address = os.environ['NOTIFY_SOCKET']
        with socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM) as notify:
            notify.connect('\0' + address[1:] if address.startswith('@') else address)
            notify.sendall(b'READY=1')
    return code if code >= 0 else 128 - code


def wait_job(directory):
    offsets = {'stdout': 0, 'stderr': 0}
    while True:
        for name, stream in [('stdout', sys.stdout.buffer), ('stderr', sys.stderr.buffer)]:
            path = directory / 'results' / (name + '.log')
            if path.exists():
                with path.open('rb') as log:
                    log.seek(offsets[name])
                    data = log.read(65536)
                    offsets[name] += len(data)
                    stream.write(data)
                    stream.flush()
        if (directory / 'collection.json').exists():
            receipt = json.loads((directory / 'collection.json').read_text())
            if receipt['status'] == 'error':
                raise RuntimeError(f'Scratch cleanup failed: {receipt}')
            # Drain logs before returning even when a fast command already finished.
            if any((directory / 'results' / (name + '.log')).exists() and
                   (directory / 'results' / (name + '.log')).stat().st_size > offset for name, offset in offsets.items()):
                continue
            if not (directory / 'command.json').exists():
                raise RuntimeError(f'Job terminated before recording command status; see {directory}')
            code = json.loads((directory / 'command.json').read_text())['returncode']
            if code == 0 and receipt['service_result'] != 'success':
                raise RuntimeError(f'Job did not retire successfully: {receipt}')
            return code
        state = unit_state(load(directory)['unit'])
        if state['ActiveState'] in ('inactive', 'failed'):
            raise RuntimeError(f'Job ended without a cleanup receipt; see {directory}')
        time.sleep(0.2)


def allocate(kind, arguments, service=None):
    if service is not None and not re.fullmatch('[a-z0-9-]+', service):
        raise RuntimeError('Invalid shared cache service name')
    root = jobs_root()
    recover(root)
    temporary_root = None if service else scratch_root()
    identifier = os.environ['INVOCATION_ID'] if service else uuid.uuid4().hex
    if not ID.fullmatch(identifier):
        raise RuntimeError('Invalid service invocation ID')
    directory = root / identifier
    directory.mkdir(mode=0o700)
    results = directory / 'results'
    results.mkdir(mode=0o700)
    data = Path(__file__).read_bytes()
    snapshot = directory / 'runner.py'
    snapshot.write_bytes(data)
    snapshot.chmod(0o400)
    unit = f'openrotmg-cache-{service}.service' if service else f'openrotmg-qa-{identifier}.service'
    row = {'version': 1 if service else 2, 'scratch_root': str(temporary_root) if temporary_root is not None else None,
           'id': identifier, 'uid': os.getuid(), 'source': str(ROOT), 'unit': unit,
           'service': service, 'command': command_for(kind, arguments, ROOT, results),
           'runner_sha256': hashlib.sha256(data).hexdigest(), 'created': time.time(),
           'head': None if service else subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
           'preservation_required': 'OPENROTMG_QA_PRESERVATION_FILE' in os.environ,
           'preservation_policy': str(Path(os.environ.get('OPENROTMG_QA_PRESERVATION_FILE',
                Path(os.environ.get('XDG_CONFIG_HOME', Path.home() / '.config')) / 'openrotmg/qa-preservation.json')).absolute())}
    save(directory / 'job.json', row)
    print(f'QA job: {directory}\nQA results: {results}\nQA scratch: {scratch_path(directory, row)}\nQA service: {unit}', flush=True)
    return directory


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, epilog='Results: $XDG_STATE_HOME/openrotmg/qa-jobs/ID/results (default ~/.local/state). Requires a user systemd manager and cgroup v2. Short disk-backed scratch: $OPENROTMG_QA_SCRATCH_ROOT (default $XDG_CACHE_HOME/oq). Long jobs: --background; inspect/cancel using the printed job directory. A preserve-scratch file in that job directory is an explicit hold. Host holds: $OPENROTMG_QA_PRESERVATION_FILE (default $XDG_CONFIG_HOME/openrotmg/qa-preservation.json), JSON version:1 with protectedPaths/protectedWords arrays; malformed policy preserves scratch. Host shared cache units can be listed in $XDG_CONFIG_HOME/openrotmg/qa-services.json; they start before jobs, not inside their cgroups. No cleanup/release command is needed.')
    parser.add_argument('--background', action='store_true')
    parser.add_argument('--timeout', type=int, default=3600, help='Whole-job deadline in seconds, including surviving children (default 3600)')
    parser.add_argument('kind', choices=['cargo', 'native', 'exec', 'status', 'cancel', '__run', '__collect', '__service', '__service-collect'])
    parser.add_argument('arguments', nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    os.umask(0o077)
    try:
        if os.getuid() == 0:
            raise RuntimeError('Run QA as the checkout owner, never root')
        if args.kind == '__service':
            return run_job(allocate('exec', args.arguments[1:], service=args.arguments[0]))
        if args.kind == '__service-collect':
            collect(jobs_root() / os.environ['INVOCATION_ID'], stopping=True)
            return 0
        if args.kind in ('__run', '__collect', 'status', 'cancel'):
            directory = Path(args.arguments[0])
            row = load(directory)
            if args.kind == '__run':
                return run_job(directory)
            if args.kind == '__collect':
                collect(directory, stopping=True)
                return 0
            if args.kind == 'status':
                print(json.dumps({'job': row, 'service': unit_state(row['unit']),
                                  'command': json.loads((directory / 'command.json').read_text()) if (directory / 'command.json').exists() else None,
                                  'collection': json.loads((directory / 'collection.json').read_text()) if (directory / 'collection.json').exists() else None}, indent=2))
                return 0
            state = unit_state(row['unit'])
            if state['InvocationID'] != row.get('invocation'):
                raise RuntimeError('Cannot cancel a different or unrecorded service invocation')
            return subprocess.call(['systemctl', '--user', 'stop', row['unit']])
        if args.timeout < 1:
            raise RuntimeError('--timeout must be positive')
        if ROOT.stat().st_uid != os.getuid():
            raise RuntimeError('Run QA as the source checkout owner')
        services = Path(os.environ.get('XDG_CONFIG_HOME', Path.home() / '.config')) / 'openrotmg/qa-services.json'
        try:
            configured = services.read_text()
        except FileNotFoundError:
            if services.is_symlink():
                raise
            configured = None
        if configured is not None:
            required = json.loads(configured)
            if not isinstance(required, list) or any(not isinstance(name, str) or not re.fullmatch('openrotmg-cache-[a-z0-9-]+\\.service', name) for name in required):
                raise RuntimeError(f'Invalid shared cache service configuration: {services}')
            if required:
                subprocess.run(['systemctl', '--user', 'start', *required], check=True)
        directory = allocate(args.kind, args.arguments)
        results = directory / 'results'
        snapshot = directory / 'runner.py'
        unit = load(directory)['unit']
        inherit = [f'--setenv={key}' for key in os.environ if key not in ('INVOCATION_ID', 'NOTIFY_SOCKET', 'JOURNAL_STREAM', 'SYSTEMD_EXEC_PID') and not key.startswith(('LISTEN_', 'WATCHDOG_'))]
        launch = ['systemd-run', '--user', '--quiet', '--collect', '--service-type=exec', '--unit=' + unit,
                  '--property=ExitType=cgroup', '--property=KillMode=control-group', '--property=Restart=no',
                  '--property=RuntimeMaxSec=' + str(args.timeout), '--property=TimeoutStopSec=20',
                  '--property=UMask=0077', '--property=WorkingDirectory=' + str(ROOT).replace('%', '%%'),
                  '--property=StandardOutput=append:' + str(results / 'worker.log').replace('%', '%%'),
                  '--property=StandardError=append:' + str(results / 'worker.log').replace('%', '%%'),
                  '--property=ExecStopPost=' + ' '.join('"' + str(p).replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%').replace('$', '$$') + '"' for p in [sys.executable, snapshot, '__collect', directory]),
                  *inherit, sys.executable, str(snapshot), '__run', str(directory)]
        subprocess.run(launch, check=True)
        if args.background:
            return 0
        try:
            return wait_job(directory)
        except KeyboardInterrupt:
            subprocess.run(['systemctl', '--user', 'stop', unit], check=True)
            return 130
    except (OSError, ValueError, IndexError, KeyError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'QA error: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())

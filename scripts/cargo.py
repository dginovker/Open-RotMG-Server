#!/usr/bin/env python3
import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
MARKER = 'OpenRotMG disposable Cargo intermediates v1\n'


def cache_root():
    base = Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache'))
    if not base.is_absolute():
        raise RuntimeError('XDG_CACHE_HOME must be absolute')
    return base / 'openrotmg' / 'cargo'


def walk_error(error):
    raise error


def validate_cleanup(root):
    device = root.stat().st_dev
    for parent, dirs, files in os.walk(root, followlinks=False, onerror=walk_error):
        for name in dirs + files:
            path = Path(parent) / name
            info = path.lstat()
            if info.st_dev != device or path.is_symlink():
                raise RuntimeError(f'Build cache contains a symlink or another filesystem: {path}')


@contextmanager
def locked(root, cleaning=False, marker_text=MARKER):
    if root.resolve() != root:
        raise RuntimeError(f'Build cache path must not contain symlinks: {root}')
    root.mkdir(parents=True, exist_ok=True)
    with os.fdopen(os.open(root / '.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600), 'a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            if cleaning:
                raise RuntimeError(f'Cannot clean an active build cache: {root}') from None
            print(f'Waiting for artifact store lock: {root}', flush=True)
            fcntl.flock(lock, fcntl.LOCK_EX)
        marker = root / '.owner'
        if marker.is_symlink():
            raise RuntimeError(f'Build cache marker must not be a symlink: {marker}')
        if marker.exists():
            if marker.read_text() != marker_text:
                raise RuntimeError(f'Unrecognized artifact store ownership: {root}')
        elif any(path.name != '.lock' for path in root.iterdir()):
            raise RuntimeError(f'Refusing to adopt a nonempty build cache: {root}')
        else:
            marker.write_text(marker_text)
        # Children retain the lock if the wrapper exits before Cargo does.
        yield lock


def clean(root):
    validate_cleanup(root)
    for target in root.iterdir():
        if not target.is_dir():
            continue
        if len(target.name) != 20 or any(c not in '0123456789abcdef' for c in target.name):
            raise RuntimeError(f'Unexpected cache directory; refusing cleanup: {target}')
        candidates = []
        for parent, dirs, files in os.walk(target, onerror=walk_error):
            for name in dirs[:]:
                path = Path(parent) / name
                if name in ('deps', 'incremental') and path.parent.name in ('debug', 'release'):
                    candidates.append(path)
                    dirs.remove(name)
                elif name == 'build':
                    dirs.remove(name)
        for path in candidates:
            print(f'Removing compiler intermediates: {path}', flush=True)
            shutil.rmtree(path)


def binary_digest(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def write_json(path, record):
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix='.metadata-', delete=False) as temporary:
        stage = Path(temporary.name)
    try:
        with stage.open('w') as stream:
            json.dump(record, stream)
            stream.flush()
            os.fsync(stream.fileno())
        stage.replace(path)
        sync_directory(path.parent)
    finally:
        stage.unlink(missing_ok=True)


def record_source(target, output):
    if target.resolve() != target:
        raise RuntimeError('Cargo target must not contain symlinks')
    target.mkdir(parents=True, exist_ok=True)
    path = target / '.source.json'
    if path.is_symlink():
        raise RuntimeError('Cargo source provenance must not be a symlink')
    present = path.exists()
    record = json.loads(path.read_text()) if present else {'version': 1, 'source': str(ROOT), 'outputs': []}
    original = record.copy() if isinstance(record, dict) else record
    if not isinstance(record, dict) or record.get('version') != 1 or record.get('source') != str(ROOT) or not isinstance(record.get('outputs'), list) or any(not isinstance(p, str) or not Path(p).is_absolute() for p in record['outputs']):
        raise RuntimeError('Cargo source provenance does not match this checkout')
    if output is not None:
        record['outputs'] = sorted(set(record['outputs']) | {str(output.absolute()), str(output.resolve())})
    if not present or record != original:
        write_json(path, record)


def retain_binary(executable, output):
    root = Path(os.environ.get('OPENROTMG_RETAINED_STORE', Path.home() / '.local/share/openrotmg/retained-binaries'))
    if not root.is_absolute() or root.resolve() != root:
        raise RuntimeError('Retained binary store must be absolute and contain no symlinks')
    if output.resolve().is_relative_to(root):
        raise RuntimeError('Retained output must be outside the immutable object store')
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = root.stat()
    if info.st_uid != os.getuid() or info.st_mode & 0o022:
        raise RuntimeError('Retained binary store must be owned by the current user and not writable by others')
    output.parent.mkdir(parents=True, exist_ok=True)
    if root.stat().st_dev != output.parent.stat().st_dev:
        raise RuntimeError('Retained binary store must share the output filesystem; set OPENROTMG_RETAINED_STORE explicitly')
    with locked(root, marker_text='OpenRotMG immutable retained binaries v1\n'):
        before = executable.stat()
        mode = before.st_mode & 0o777 & ~0o222
        if not mode & 0o111:
            raise RuntimeError('Expected an executable compiler artifact')
        digest = binary_digest(executable)
        obj = root / f'{digest}-{mode:o}.bin'
        if obj.is_symlink():
            raise RuntimeError('Retained binary object must not be a symlink')
        created = not obj.exists()
        if created:
            with tempfile.NamedTemporaryFile(dir=root, prefix='.retain-', delete=False) as temporary:
                stage = Path(temporary.name)
            try:
                shutil.copy2(executable, stage)
                if binary_digest(stage) != digest:
                    raise RuntimeError('Executable changed during retention')
                stage.chmod(mode)
                with stage.open('rb') as stream:
                    os.fsync(stream.fileno())
                stage.replace(obj)
            finally:
                stage.unlink(missing_ok=True)
        info = obj.lstat()
        if obj.is_symlink() or not obj.is_file() or info.st_uid != os.getuid() or info.st_mode & 0o777 != mode:
            raise RuntimeError('Retained binary object ownership or permissions changed')
        if binary_digest(obj) != digest:
            raise RuntimeError('Retained binary object content changed')
        after = executable.stat()
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_mode) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_mode):
            raise RuntimeError('Executable changed before retention completed')
        provenance = obj.with_suffix('.json')
        if provenance.is_symlink():
            raise RuntimeError('Retained binary provenance must not be a symlink')
        if created or provenance.exists():
            record = json.loads(provenance.read_text()) if provenance.exists() else {'version': 1, 'sources': [], 'outputs': []}
            if not isinstance(record, dict) or record.get('version') != 1 or any(not isinstance(record.get(key), list) or
                    any(not isinstance(path, str) or not Path(path).is_absolute() for path in record[key])
                    for key in ('sources', 'outputs')):
                raise RuntimeError('Invalid retained binary provenance')
            record['sources'] = sorted(set(record['sources']) | {str(ROOT)})
            record['outputs'] = sorted(set(record['outputs']) | {str(output.absolute()), str(output.resolve())})
            write_json(provenance, record)
        sync_directory(root)
        with tempfile.NamedTemporaryFile(dir=output.parent, prefix='.openrotmg-', delete=False) as temporary:
            stage = Path(temporary.name)
        stage.unlink()
        try:
            os.link(obj, stage, follow_symlinks=False)
            stage.replace(output)
            sync_directory(output.parent)
        finally:
            stage.unlink(missing_ok=True)
    print(f'Retained binary object: sha256={digest}; output={output}', flush=True)


def execute(command, target, output, lock):
    env = {**os.environ, 'CARGO_TARGET_DIR': str(target)}
    if output is None:
        return subprocess.call(command, cwd=ROOT, env=env, pass_fds=(lock.fileno(),))
    if output.resolve().is_relative_to(cache_root().resolve()) or output.is_symlink():
        raise RuntimeError('Retained binary must be outside the cache and not a symlink')
    if any(arg.startswith('--message-format') for arg in command):
        raise RuntimeError('--output owns Cargo message format')
    executable = None
    with subprocess.Popen([*command, '--message-format=json-render-diagnostics'], cwd=ROOT, env=env, stdout=subprocess.PIPE, text=True, pass_fds=(lock.fileno(),)) as process:
        for line in process.stdout:
            message = json.loads(line)
            if message.get('reason') == 'compiler-message':
                print(message['message'].get('rendered', ''), end='', flush=True)
            if message.get('reason') == 'compiler-artifact' and message.get('executable') and message['target']['name'] == 'openrotmg':
                executable = Path(message['executable'])
        code = process.wait()
    if code:
        return code
    if executable is None or not executable.resolve().is_relative_to(target.resolve()):
        raise RuntimeError('Cargo did not identify the built openrotmg executable inside its cache')
    retain_binary(executable, output)
    print(f'Retained binary: {output}', flush=True)
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description='Cargo builds with isolated, reusable targets and explicit cache cleanup.',
                                     epilog='User-wide cache: $XDG_CACHE_HOME/openrotmg/cargo (default ~/.cache/openrotmg/cargo). clean removes only managed deps/incremental, never SDK/build data or binaries.')
    parser.add_argument('--toolchain', help='Explicit rustup toolchain, e.g. 1.94.1')
    parser.add_argument('--output', type=Path, help='Atomically retain a read-only executable outside the cache, sharing identical retained binaries (build only)')
    parser.add_argument('command', choices=['build', 'check', 'test', 'clippy', 'fmt', 'clean'])
    parser.add_argument('arguments', nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    try:
        if args.command == 'clean' and args.arguments:
            raise RuntimeError('clean does not accept Cargo arguments')
        if args.output is not None and args.command != 'build':
            raise RuntimeError('--output requires build')
        toolchain = ['+' + args.toolchain] if args.toolchain else []
        if args.command == 'fmt':
            return subprocess.call(['cargo', *toolchain, 'fmt', *args.arguments], cwd=ROOT)
        if any(arg.split('=', 1)[0] in ('--target-dir', '--config', '--manifest-path', '--out-dir', '--artifact-dir') for arg in args.arguments):
            raise RuntimeError('Storage/manifest overrides bypass the managed build cache and are not allowed')
        if 'CARGO_TARGET_DIR' in os.environ or 'CARGO_BUILD_TARGET_DIR' in os.environ or 'CARGO_BUILD_BUILD_DIR' in os.environ:
            raise RuntimeError('Unset Cargo target/build directory overrides; use the managed cache, not a per-task target')
        root = cache_root()
        if root.resolve().is_relative_to(ROOT.resolve()):
            raise RuntimeError('Build cache must be outside the source worktree')
        with locked(root, cleaning=args.command == 'clean') as lock:
            if args.command == 'clean':
                clean(root)
                return 0
            version = subprocess.check_output(['rustc', *toolchain, '-vV'], cwd=ROOT, text=True)
            # Cargo tracks env! inputs; runtime diagnostics and job counts need no separate target tree.
            settings = {key: value for key, value in os.environ.items()
                        if key.startswith(('RUST', 'CARGO_PROFILE_', 'CARGO_BUILD_'))
                        and key not in ('RUST_LOG', 'RUST_BACKTRACE', 'RUST_LIB_BACKTRACE', 'RUST_TEST_THREADS', 'RUST_MIN_STACK', 'CARGO_BUILD_JOBS')}
            # Cargo can treat an older sibling checkout's different inputs as fresh.
            key = hashlib.sha256(json.dumps([str(ROOT), version, settings], sort_keys=True).encode()).hexdigest()[:20]
            target = root / key
            # Failed builds can stop before Cargo emits source-bound depfiles.
            record_source(target, args.output)
            print(f'Cargo target: {target}; checkout={ROOT}', flush=True)
            return execute(['cargo', *toolchain, args.command, *args.arguments], target, args.output, lock)
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'Cargo wrapper error: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())

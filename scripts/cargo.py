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
GIB = 1024**3
MARKER = 'OpenRotMG disposable Cargo intermediates v1\n'


def cache_root():
    base = Path(os.environ.get('XDG_CACHE_HOME', Path.home() / '.cache'))
    if not base.is_absolute():
        raise RuntimeError('XDG_CACHE_HOME must be absolute')
    return base / 'openrotmg' / 'cargo'


def walk_error(error):
    raise error


def allocation(root):
    seen = set()
    total = 0
    device = root.stat().st_dev
    for parent, dirs, files in os.walk(root, followlinks=False, onerror=walk_error):
        for name in dirs + files:
            path = Path(parent) / name
            info = path.lstat()
            if info.st_dev != device or path.is_symlink():
                raise RuntimeError(f'Build cache contains a symlink or another filesystem: {path}')
            identity = (info.st_dev, info.st_ino)
            if identity not in seen:
                seen.add(identity)
                total += info.st_blocks * 512
    return total


def report(root):
    free = shutil.disk_usage(root).free / GIB
    used = allocation(root) / GIB
    print(f'Build storage: path={root}; free={free:.2f}GiB; cache={used:.2f}GiB', flush=True)


@contextmanager
def locked(root, cleaning=False):
    if root.resolve() != root:
        raise RuntimeError(f'Build cache path must not contain symlinks: {root}')
    root.mkdir(parents=True, exist_ok=True)
    with os.fdopen(os.open(root / '.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600), 'a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            if cleaning:
                raise RuntimeError(f'Cannot clean an active build cache: {root}') from None
            print(f'Waiting for build cache lock: {root}', flush=True)
            fcntl.flock(lock, fcntl.LOCK_EX)
        marker = root / '.owner'
        if marker.is_symlink():
            raise RuntimeError(f'Build cache marker must not be a symlink: {marker}')
        if marker.exists():
            if marker.read_text() != MARKER:
                raise RuntimeError(f'Unrecognized build cache: {root}')
        elif any(path.name != '.lock' for path in root.iterdir()):
            raise RuntimeError(f'Refusing to adopt a nonempty build cache: {root}')
        else:
            marker.write_text(MARKER)
        # Children retain the lock if the wrapper exits before Cargo does.
        yield lock


def clean(root):
    allocation(root)
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


def execute(command, target, output, lock, output_bin='openrotmg'):
    env = {**os.environ, 'CARGO_TARGET_DIR': str(target)}
    if output is None:
        return subprocess.call(command, cwd=ROOT, env=env, pass_fds=(lock.fileno(),))
    if output.resolve().is_relative_to(cache_root().resolve()) or output.is_symlink():
        raise RuntimeError('Retained binary must be outside the cache and not a symlink')
    if any(arg.startswith('--message-format') for arg in command):
        raise RuntimeError('--output owns Cargo message format')
    executables = []
    with subprocess.Popen([*command, '--message-format=json-render-diagnostics'], cwd=ROOT, env=env, stdout=subprocess.PIPE, text=True, pass_fds=(lock.fileno(),)) as process:
        for line in process.stdout:
            message = json.loads(line)
            if message.get('reason') == 'compiler-message':
                print(message['message'].get('rendered', ''), end='', flush=True)
            if (message.get('reason') == 'compiler-artifact' and message.get('executable')
                    and message['target']['name'] == output_bin and message['target']['kind'] == ['bin']
                    and message['profile']['test'] is False):
                executables.append(Path(message['executable']))
        code = process.wait()
    if code:
        return code
    if len(executables) != 1 or not executables[0].resolve().is_relative_to(target.resolve()):
        raise RuntimeError(f'Cargo must identify exactly one built {output_bin} executable inside its cache')
    executable = executables[0]
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=output.parent, prefix='.openrotmg-', delete=False) as temporary:
        stage = Path(temporary.name)
    try:
        shutil.copy2(executable, stage)
        stage.replace(output)
    finally:
        stage.unlink(missing_ok=True)
    print(f'Retained binary: {output}', flush=True)
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description='Serialized Cargo builds with storage reporting, not free-space or cache-size admission gates.',
                                     epilog='User-wide cache: $XDG_CACHE_HOME/openrotmg/cargo (default ~/.cache/openrotmg/cargo). clean removes only managed deps/incremental, never SDK/build data or binaries.')
    parser.add_argument('--toolchain', help='Explicit rustup toolchain, e.g. 1.94.1')
    parser.add_argument('--output', type=Path, help='Atomically retain the server executable outside the cache (build only)')
    parser.add_argument('--output-bin', choices=['openrotmg', 'openrotmg-build100'], help='Executable to retain with --output (default: openrotmg)')
    parser.add_argument('command', choices=['build', 'check', 'test', 'clippy', 'fmt', 'status', 'clean'])
    parser.add_argument('arguments', nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    try:
        if args.output_bin is not None and args.output is None:
            raise RuntimeError('--output-bin requires --output')
        if args.command in ('status', 'clean') and args.arguments:
            raise RuntimeError('status and clean do not accept Cargo arguments')
        if args.output is not None and args.command != 'build':
            raise RuntimeError('--output requires build')
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
            report(root)
            if args.command == 'status':
                return 0
            toolchain = ['+' + args.toolchain] if args.toolchain else []
            version = subprocess.check_output(['rustc', *toolchain, '-vV'], cwd=ROOT, text=True)
            settings = {key: value for key, value in os.environ.items() if key.startswith(('RUST', 'CARGO_PROFILE_', 'CARGO_BUILD_'))}
            # Cargo can treat an older sibling checkout's different inputs as fresh.
            key = hashlib.sha256(json.dumps([str(ROOT), version, settings], sort_keys=True).encode()).hexdigest()[:20]
            target = root / key
            print(f'Cargo target: {target}; checkout={ROOT}', flush=True)
            code = execute(['cargo', *toolchain, args.command, *args.arguments], target, args.output, lock, args.output_bin or 'openrotmg')
            if code:
                return code
            report(root)
            return 0
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'Build storage error: {error}', file=sys.stderr)
        return 1


if __name__ == '__main__':
    raise SystemExit(main())

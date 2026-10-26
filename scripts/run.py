#!/usr/bin/env python3
import argparse
import hashlib
import io
import json
from pathlib import Path
import re
import shutil
import signal
import socket
import subprocess
import sys
from urllib.request import Request, urlopen
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parents[1]
CLIENT_SELECTION_URL = "https://openrotmg.org/diagnostics-build.json"
LOG = ROOT / ".local/server.log"


def client_selection(manifest=None):
    if manifest is None:
        try:
            request = Request(CLIENT_SELECTION_URL, headers={'Cache-Control': 'no-cache'})
            with urlopen(request, timeout=30) as response:
                value = json.load(response)
        except Exception as error:
            raise RuntimeError(f"Resolve deployed client {CLIENT_SELECTION_URL} failed: {error}") from error
    else:
        value = json.loads(manifest.read_text())
    for name, pattern in [('client', '[0-9a-f]{40}'), ('client_archive_sha256', '[0-9a-f]{64}'),
                          ('runtime_version', r'[0-9]+\.[0-9]+\.[0-9]+-openrotmg\.[0-9]+'),
                          ('runtime_bundle_sha256', '[0-9a-f]{64}')]:
        if not isinstance(value, dict) or not isinstance(value.get(name), str) or not re.fullmatch(pattern, value[name]):
            raise RuntimeError(f"Invalid deployed client selection: {name}. The website must publish a complete client manifest.")
    return {name: value[name] for name in ('client', 'client_archive_sha256', 'runtime_version', 'runtime_bundle_sha256')}


def prepare_client(manifest=None):
    selection = client_selection(manifest)
    revision, expected = selection['client'], selection['client_archive_sha256']
    url = f"https://codeload.github.com/OpenRotMG/Open-RotMG-Client/zip/{revision}"
    cache = LOG.parent / f"client-{revision}.zip"
    cache.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    if cache.is_symlink():
        raise RuntimeError(f"Client archive cache must not be a symlink: {cache}")
    if cache.exists():
        data = cache.read_bytes()
    else:
        print(f"Downloading local client: {url}", flush=True)
        try:
            with urlopen(url, timeout=60) as response:
                data = response.read()
        except Exception as error:
            raise RuntimeError(f"Download {url} failed: {type(error).__name__}: {error}") from error
    actual = hashlib.sha256(data).hexdigest()
    if actual != expected:
        raise RuntimeError(f"Client archive checksum mismatch: expected {expected}, got {actual}. Remove {cache} and rerun.")
    prefix = f"Open-RotMG-Client-{revision}"
    client = cache.parent / prefix
    with ZipFile(io.BytesIO(data)) as archive:
        if len(archive.namelist()) != len(set(archive.namelist())):
            raise RuntimeError('Duplicate client archive paths')
        for item in archive.infolist():
            path = Path(item.filename)
            if path.is_absolute() or ".." in path.parts or path.parts[0] != prefix or (item.external_attr >> 16) & 0o170000 not in (0, 0o100000, 0o040000):
                raise RuntimeError(f"Unsafe client archive entry: {item.filename}")
        lock = json.loads(archive.read(f'{prefix}/ruffle/lock.json'))
        if lock['version'] != selection['runtime_version'] or lock['bundleSha256'] != selection['runtime_bundle_sha256']:
            raise RuntimeError('Client runtime lock does not match the deployed client selection')
        if client.is_symlink():
            raise RuntimeError(f'Cached client must not be a symlink: {client}')
        if not client.exists():
            stage = cache.parent / f"client-extract-{revision}"
            stage.mkdir(mode=0o700)
            archive.extractall(stage)
            (stage / prefix).rename(client)
            stage.rmdir()
        for item in archive.infolist():
            path = cache.parent / item.filename
            if any(parent.is_symlink() for parent in [path, *path.parents] if parent != cache.parent):
                raise RuntimeError(f'Cached client path must not contain symlinks: {path}')
            if not item.is_dir() and (not path.is_file() or path.read_bytes() != archive.read(item.filename)):
                raise RuntimeError(f"Cached client asset is missing or modified: {path}. Remove {client} and rerun.")
        # ZipFile drops Unix permissions; restore the verified launcher on fresh and cached installs.
        (client / "run.sh").chmod((archive.getinfo(f"{prefix}/run.sh").external_attr >> 16) & 0o777)
    if not cache.exists():
        cache.write_bytes(data)
    receipt = json.dumps(selection, sort_keys=True)
    (cache.parent / 'client-selection.json').write_text(receipt + '\n')
    print(f'Client selection: {receipt}', flush=True)
    with LOG.open('a') as log:
        log.write(f'Client selection: {receipt}\n')
    return client


def run(command, cwd, log):
    with subprocess.Popen(command, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True) as process:
        try:
            for line in process.stdout:
                print(line, end="", flush=True)
                log.write(line)
                log.flush()
            return process.wait()
        except KeyboardInterrupt:
            process.send_signal(signal.SIGINT)
            process.wait()
            raise


def port_number(value):
    try:
        port = int(value)
    except ValueError:
        raise argparse.ArgumentTypeError("port must be an integer between 1 and 65535") from None
    if not 1 <= port <= 65535:
        raise argparse.ArgumentTypeError("port must be between 1 and 65535")
    return port


def main():
    parser = argparse.ArgumentParser(prog="run.sh", description="Download the client and start your local OpenRotMG game.")
    parser.add_argument("port", type=port_number, help="Local server port")
    client_mode = parser.add_mutually_exclusive_group()
    client_mode.add_argument("--client", "--local-client", dest="local_client", type=Path, metavar="DIR", help="Use your own client checkout instead of the deployed client")
    client_mode.add_argument("--client-manifest", type=Path, metavar="FILE", help="Reproduce an exact saved client-selection.json instead of resolving the deployed client")
    states = parser.add_mutually_exclusive_group()
    states.add_argument("--state", type=Path, default=ROOT / ".local", metavar="DIR", help="Isolate saves, keys, client cache and logs in this directory")
    states.add_argument("--test-state", type=Path, metavar="DIR", help="Create new PREPARED test state with local controls; refuses an existing game database")
    parser.add_argument("--dev-control", action="store_true", help="Enable loopback-only QA control")
    bots = parser.add_mutually_exclusive_group()
    bots.add_argument("--shore-bots", action="store_true", help="Enable the three default autonomous Wizards explicitly")
    bots.add_argument("--no-shore-bots", action="store_true", help="Disable autonomous Wizards for isolated testing")
    parser.add_argument("--bot-llm", action="store_true", help="Enable Nemotron social dialogue; requires AI_GATEWAY_API_KEY")
    parser.add_argument("--release", action="store_true", help="Build optimized gameplay")
    args = parser.parse_args()
    if shutil.which("cargo") is None:
        parser.error("Cargo not found. Install Rust/Cargo from https://rustup.rs/ and rerun.")
    global LOG
    state = (args.test_state if args.test_state is not None else args.state).resolve()
    state.mkdir(mode=0o700, parents=True, exist_ok=True)
    LOG = state / "server.log"
    with LOG.open("a") as log:
        try:
            try:
                with socket.socket() as probe:
                    probe.bind(("127.0.0.1", args.port))
            except OSError as error:
                raise RuntimeError(f"Cannot listen on 127.0.0.1:{args.port}: {error}") from error
            revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
            dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True).strip())
            guard = str(Path(__file__).with_name('cargo.py'))
            if args.local_client is not None:
                client = args.local_client.resolve()
            else:
                client = prepare_client(args.client_manifest) if args.client_manifest is not None else prepare_client()
            prepare = client / "scripts/run.py"
            if not prepare.is_file():
                raise RuntimeError(f"Missing client launcher: {prepare}. Use a complete Open-RotMG-Client checkout.")
            receipt = f"Local source: {revision} dirty={dirty}; port={args.port}; state={state}; client={client}"
            print(receipt, flush=True)
            log.write(receipt + "\n")
            log.flush()
            print(f"Preparing local client. Logs: {LOG}", flush=True)
            code = run([sys.executable, str(prepare), "--prepare-only"], client, log)
            if code:
                raise RuntimeError(f"Client preparation exited with status {code}. See the error above and {LOG}.")
            print(f"Building and starting your local game. Logs: {LOG}", flush=True)
            binary = state / 'server-build' / 'openrotmg'
            build = [sys.executable, guard, '--output', str(binary), 'build', '--locked']
            if args.release:
                build.append('--release')
            code = run(build, ROOT, log)
            if code:
                raise RuntimeError(f'Server build exited with status {code}. See {LOG}.')
            command = [str(binary), "--client", str(client), "--port", str(args.port), "--test-state" if args.test_state is not None else "--state", str(state)]
            if args.dev_control:
                command.append("--dev-control")
            if args.shore_bots:
                command.append("--shore-bots")
            if args.no_shore_bots:
                command.append("--no-shore-bots")
            if args.bot_llm:
                command.append("--bot-llm")
            code = run(command, ROOT, log)
            if code:
                raise RuntimeError(f"Server exited with status {code}. See the error above and {LOG}.")
            return 0
        except KeyboardInterrupt:
            print("Local server startup interrupted.", file=sys.stderr)
            return 130
        except Exception as error:
            message = f"Local server failed: {type(error).__name__}: {error}"
            log.write(message + "\n")
            print(message, file=sys.stderr)
            print(f"Log: {LOG}", file=sys.stderr)
            return 1


if __name__ == "__main__":
    raise SystemExit(main())

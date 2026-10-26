#!/usr/bin/env python3
import argparse
from collections import deque
import hashlib
import fcntl
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import time


def emit(event, **fields):
    print(json.dumps({'event': event, **fields}), flush=True)


def write(path, value):
    with path.open('x') as output:
        json.dump(value, output)
        output.flush()
        os.fsync(output.fileno())


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def retire(root, pattern, keep):
    paths = sorted((p for p in root.glob(pattern) if p.is_dir()), key=lambda p: p.name)
    for path in paths[:-keep]:
        if (path / 'complete.json').is_file() and not (path / 'retain').exists():
            if not re.fullmatch(r'(session-\d+-\d+|incident-\d+)', path.name):
                raise RuntimeError(f'Unrecognized profile cache entry: {path}')
            if path.is_symlink() or any(p.is_symlink() and not p.resolve().is_relative_to(path) for p in path.rglob('*')):
                raise RuntimeError(f'External symlink in profile cache: {path}')
            shutil.rmtree(path)


def incident(line, pid):
    envelope = json.loads(line)
    if envelope.get('_PID') != str(pid):
        return None
    try:
        record = json.loads(envelope['MESSAGE'])
    except json.JSONDecodeError:
        emit('unstructured_target_log', pid=pid)
        return None
    fields = record['fields']
    if fields['message'] != 'latency incident':
        return None
    if fields['pid'] != pid or fields['clock'] != 'CLOCK_MONOTONIC':
        raise RuntimeError('Incident process or clock does not match recorder')
    sample = json.loads(fields['sample'])
    span = sample['execution']
    if not (span['tid'] > 0 and 0 < span['start_ns'] <= span['end_ns'] <= time.monotonic_ns()):
        raise RuntimeError('Invalid incident execution interval')
    return {'logged_at': record['timestamp'], 'world': fields.get('world'), 'sample': sample}


def journal_command(service):
    # Without --all, journalctl replaces JSON fields larger than 4 KiB with null.
    return ['journalctl', '--all', '-u', service, '-f', '--since', 'now', '-o', 'json', '--no-pager']


def cache_binary(directory):
    subprocess.run(['perf', '--buildid-dir', str(directory / 'buildids'),
                    'buildid-cache', '--add', str(directory / 'binary')], check=True, timeout=15)


def run(service, root):
    os.umask(0o077)
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    if not root.is_absolute() or root.resolve() != root or root.stat().st_uid != os.geteuid() or root.stat().st_mode & 0o077:
        raise RuntimeError('Profile directory must be private, owned and free of symlink parents')
    marker = root / 'owner.json'
    if marker.exists():
        if json.loads(marker.read_text()) != {'format': 1, 'owner': 'openrotmg-profiler'}:
            raise RuntimeError('Unrecognized profile directory ownership')
    elif list(root.iterdir()):
        raise RuntimeError('Refusing to adopt a nonempty profile directory')
    else:
        write(marker, {'format': 1, 'owner': 'openrotmg-profiler'})
    lock_fd = os.open(root / '.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    retire(root, 'session-*', 1)
    retire(root, 'incident-*', 8)
    pid = int(subprocess.check_output(['systemctl', 'show', service, '-p', 'MainPID', '--value'], text=True))
    if pid <= 0:
        raise RuntimeError('Profiling requires a running service process')
    proc = Path(f'/proc/{pid}')
    identity = proc.joinpath('stat').read_text().rsplit(')', 1)[1].split()[19]
    executable = proc.joinpath('exe').resolve(strict=True)
    work = root / f'session-{time.time_ns()}-{pid}'
    work.mkdir(mode=0o700)
    shutil.copyfile(proc / 'exe', work / 'binary')
    work.joinpath('binary').chmod(0o400)
    metadata = {'pid': pid, 'start_ticks': identity, 'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(), 'binary_sha256': digest(work / 'binary'),
                'revision': executable.with_name('REVISION').read_text().strip(),
                'clock': 'CLOCK_MONOTONIC', 'sample_hz': 99, 'stack_bytes': 8192,
                'perf_version': subprocess.check_output(['perf', '--version'], text=True).strip()}
    if metadata['binary_sha256'] != digest(proc / 'exe'):
        raise RuntimeError('Executable changed during profiling setup')
    write(work / 'identity.json', metadata)
    cache_binary(work)
    control, ack = work / 'control', work / 'ack'
    for path in [control, ack]:
        os.mkfifo(path, 0o600)
    control_fd = os.open(control, os.O_RDWR | os.O_NONBLOCK)
    ack_fd = os.open(ack, os.O_RDWR | os.O_NONBLOCK)
    args = ['perf', 'record', '-e', 'cpu-clock:u', '-F', '99', '--clockid', 'mono',
            '--call-graph', 'dwarf,8192', '-m', '1M', '--switch-output=signal', '--switch-max-files=12',
            '--no-no-buildid', '--buildid-all', '--no-buildid-cache',
            '--control', f'fifo:{control},{ack}', '-p', str(pid), '-o', str(work / 'perf.data')]
    stopped = False
    stop_deadline = None
    def stop(signum, frame):
        nonlocal stopped, stop_deadline
        stopped = True
        if stop_deadline is None:
            stop_deadline = time.monotonic() + 5
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    perf = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    journal = subprocess.Popen(journal_command(service), stdout=subprocess.PIPE, stderr=None)
    selector = selectors.DefaultSelector()
    buffers = {}
    for stream, name in [(perf.stderr, 'perf'), (journal.stdout, 'journal')]:
        os.set_blocking(stream.fileno(), False)
        selector.register(stream, selectors.EVENT_READ, name)
        buffers[name] = b''
    selector.register(ack_fd, selectors.EVENT_READ, 'ack')
    os.write(control_fd, b'ping\n')
    ready = False
    deadline = time.monotonic() + 15
    next_rotation = time.monotonic() + 30
    last_capture = float('-inf')
    pending = None
    recent = deque(maxlen=12)
    try:
        with (work / 'perf.log').open('ab', buffering=0) as log:
            while not stopped or pending is not None:
                if stopped and time.monotonic() > stop_deadline:
                    raise RuntimeError('perf did not finish the pending incident before shutdown')
                if ready and not stopped and time.monotonic() >= next_rotation:
                    perf.send_signal(signal.SIGUSR2)
                    next_rotation = time.monotonic() + 30
                if perf.poll() is not None or journal.poll() is not None:
                    raise RuntimeError(f'Profiler transport exited: perf={perf.poll()}, journal={journal.poll()}; {work}')
                if not ready and time.monotonic() > deadline:
                    raise RuntimeError('perf did not acknowledge readiness')
                for key, _ in selector.select(1):
                    data = os.read(key.fd, 65536)
                    if key.data == 'ack':
                        if data != b'ack\n\x00':
                            raise RuntimeError(f'Unexpected perf control reply: {data!r}')
                        ready = True
                        emit('profiling_ready', **metadata, directory=str(work))
                        continue
                    name = key.data
                    buffers[name] += data
                    if len(buffers[name]) > 256 * 1024:
                        raise RuntimeError(f'Oversized profiler transport record: {name}')
                    while b'\n' in buffers[name]:
                        line, buffers[name] = buffers[name].split(b'\n', 1)
                        if name == 'perf':
                            log.write(line + b'\n')
                            if b'lost' in line.lower():
                                emit('profiling_data_loss', detail=os.fsdecode(line))
                            match = re.fullmatch(rb'\[ perf record: Dump (.+) \]', line)
                            if not match:
                                continue
                            path = Path(os.fsdecode(match[1]))
                            if path.parent != work or not re.fullmatch(r'perf\.data\.\d+', path.name) or path.stat().st_size == 0:
                                raise RuntimeError('Invalid completed perf segment')
                            recent.append(path)
                            if pending is not None:
                                capture = root / f'incident-{time.time_ns()}'
                                capture.mkdir(mode=0o700)
                                os.link(work / 'binary', capture / 'binary')
                                for segment in recent:
                                    if segment.exists():
                                        os.link(segment, capture / segment.name)
                                write(capture / 'identity.json', metadata)
                                write(capture / 'incident.json', pending)
                                cache_binary(capture)
                                write(capture / 'complete.json', {'retention': 'latest eight incidents; add retain file to pin'})
                                emit('profiling_incident_saved', directory=str(capture), **pending)
                                pending = None
                                retire(root, 'incident-*', 8)
                            size = int(subprocess.check_output(['du', '-sb', str(root)], text=True).split()[0])
                            if size > 2 * 1024**3:
                                raise RuntimeError(f'Profile storage exceeds 2 GiB; inspect retained evidence: {root}')
                        else:
                            event = incident(line, pid)
                            if event is not None and not stopped:
                                if not ready:
                                    emit('profiling_incident_not_ready', **event)
                                elif pending is not None or time.monotonic() - last_capture < 60:
                                    emit('profiling_incident_throttled', **event)
                                else:
                                    pending = event
                                    last_capture = time.monotonic()
                                    perf.send_signal(signal.SIGUSR2)
                                    next_rotation = time.monotonic() + 30
    except BaseException:
        work.joinpath('retain').touch()
        raise
    finally:
        journal.terminate()
        if perf.poll() is None:
            perf.send_signal(signal.SIGINT)
        journal.wait(timeout=10)
        perf.wait(timeout=15)
        remainder = perf.stderr.read()
        with (work / 'perf.log').open('ab') as log:
            if remainder:
                log.write(remainder)
        selector.close()
        os.close(control_fd)
        os.close(ack_fd)
    if perf.returncode not in [0, -signal.SIGINT] or not ready or pending is not None:
        work.joinpath('retain').touch()
        raise RuntimeError(f'Incomplete profiling shutdown; inspect {work}')
    write(work / 'complete.json', {'perf_status': perf.returncode})
    emit('profiling_stopped', directory=str(work))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--service', required=True)
    parser.add_argument('--output', required=True, type=Path)
    options = parser.parse_args()
    run(options.service, options.output)

import fcntl
import importlib.util
import os
from pathlib import Path
import signal
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

spec = importlib.util.spec_from_file_location('cargo_parallel', Path(__file__).with_name('cargo.py'))
cargo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cargo)


class ParallelBuildTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        for name, body in {
            'rustc': "print('fixture rustc')\n",
            'cargo': """import os,time
from pathlib import Path
root=Path(os.environ['FIXTURE_ROOT']); name=os.environ['FIXTURE_NAME']
(root/(name+'.ready')).write_text(str(os.getpid()))
(Path(os.environ['CARGO_TARGET_DIR'])/'compiler-output').write_text('regenerable')
while not (root/(name+'.release')).exists(): time.sleep(.02)
""",
        }.items():
            path = self.bin / name
            path.write_text('#!' + sys.executable + '\n' + body)
            path.chmod(0o755)
        self.env = {k: v for k, v in os.environ.items() if not k.startswith(('CARGO_', 'RUST'))}
        self.env.update(PATH=str(self.bin) + ':' + os.environ['PATH'], XDG_CACHE_HOME=str(self.root / 'cache'),
                        FIXTURE_ROOT=str(self.root))
        self.cache = self.root / 'cache/openrotmg/cargo'
        with cargo.locked(self.cache):
            pass
        self.processes = []
        self.addCleanup(self.stop)

    def stop(self):
        for name, process in self.processes:
            (self.root / (name + '.release')).touch()
        for name, process in self.processes:
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()

    def start(self, source, name):
        directory = self.root / source
        (directory / 'scripts').mkdir(parents=True, exist_ok=True)
        script = directory / 'scripts/cargo.py'
        if not script.exists():
            shutil.copyfile(cargo.__file__, script)
        with (self.root / (name + '.log')).open('w') as output:
            process = subprocess.Popen([sys.executable, str(script), 'build', '--locked'],
                                       env={**self.env, 'FIXTURE_NAME': name}, stdout=output, stderr=subprocess.STDOUT,
                                       start_new_session=True)
        self.processes.append((name, process))
        return process

    def wait_for(self, condition):
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if condition():
                return
            time.sleep(.02)
        logs = {name: (self.root / (name + '.log')).read_text() for name, _ in self.processes}
        self.fail(f'Fixture deadline exceeded: {logs}')

    def ready(self, name):
        self.wait_for(lambda: (self.root / (name + '.ready')).exists())

    def waiting(self, name, text):
        self.wait_for(lambda: text in (self.root / (name + '.log')).read_text())

    def release(self, name):
        (self.root / (name + '.release')).touch()

    def assert_locked(self, path):
        with path.open('a') as stream:
            with self.assertRaises(BlockingIOError):
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_distinct_targets_overlap_but_third_waits_and_cleanup_cannot_race(self):
        a = self.start('a', 'a')
        self.ready('a')
        b = self.start('b', 'b')
        self.ready('b')
        c = self.start('c', 'c')
        self.waiting('c', 'Waiting for one of 2 Cargo build slots')
        self.assertFalse((self.root / 'c.ready').exists())
        self.assert_locked(self.cache / '.lock')
        result = subprocess.run([sys.executable, str(self.root / 'a/scripts/cargo.py'), 'clean'],
                                env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn('Cannot clean an active build cache', result.stderr)
        self.release('a')
        self.ready('c')
        self.assertEqual(a.wait(timeout=5), 0)
        self.release('b')
        self.release('c')
        self.assertEqual(b.wait(timeout=5), 0)
        self.assertEqual(c.wait(timeout=5), 0)
        with cargo.locked(self.cache, cleaning=True):
            pass

    def test_same_target_waiter_does_not_consume_another_build_slot(self):
        a = self.start('same', 'a')
        self.ready('a')
        b = self.start('same', 'b')
        self.waiting('b', 'Waiting for Cargo lease:')
        c = self.start('other', 'c')
        self.ready('c')
        self.assertFalse((self.root / 'b.ready').exists())
        self.release('a')
        self.ready('b')
        self.release('b')
        self.release('c')
        self.assertTrue(all(p.wait(timeout=5) == 0 for p in (a, b, c)))

    def test_legacy_exclusive_lease_still_blocks_new_builds(self):
        with cargo.locked(self.cache):
            process = self.start('a', 'a')
            self.waiting('a', 'Waiting for artifact store lock:')
            self.assertFalse((self.root / 'a.ready').exists())
        self.ready('a')
        self.release('a')
        self.assertEqual(process.wait(timeout=5), 0)

    def test_maintenance_gate_drains_readers_before_admitting_new_builds(self):
        a = self.start('a', 'a')
        self.ready('a')
        code = """import fcntl,os,time
from pathlib import Path
r=Path(os.environ['FIXTURE_ROOT']); cache=r/'cache/openrotmg/cargo'
q=(cache/'.admission.lock').open(); fcntl.flock(q,fcntl.LOCK_EX)
(r/'clean.gated').touch()
g=(cache/'.lock').open(); fcntl.flock(g,fcntl.LOCK_EX)
(r/'clean.ready').touch()
while not (r/'clean.release').exists(): time.sleep(.02)
"""
        with (self.root / 'clean.log').open('w') as output:
            cleaner = subprocess.Popen([sys.executable, '-c', code], env=self.env, stdout=output,
                                       stderr=subprocess.STDOUT, start_new_session=True)
        self.processes.append(('clean', cleaner))
        self.wait_for(lambda: (self.root / 'clean.gated').exists())
        b = self.start('b', 'b')
        self.waiting('b', '.admission.lock')
        self.release('a')
        self.ready('clean')
        self.assertFalse((self.root / 'b.ready').exists())
        self.release('clean')
        self.ready('b')
        self.release('b')
        self.assertTrue(all(p.wait(timeout=5) == 0 for p in (a, b, cleaner)))

    def test_compiler_keeps_all_three_leases_when_wrapper_dies(self):
        process = self.start('a', 'a')
        self.ready('a')
        target = next(self.cache.glob('*/.source.json')).parent
        process.kill()
        process.wait(timeout=5)
        self.assert_locked(self.cache / '.lock')
        self.assert_locked(target / '.build.lock')
        self.assert_locked(self.cache / '.build-slot-0.lock')
        b = self.start('b', 'b')
        self.ready('b')
        c = self.start('c', 'c')
        self.waiting('c', 'Waiting for one of 2 Cargo build slots')
        self.release('a')
        self.ready('c')
        self.release('b')
        self.release('c')
        self.assertEqual(b.wait(timeout=5), 0)
        self.assertEqual(c.wait(timeout=5), 0)
        self.wait_for(lambda: not self.lock_busy(self.cache / '.lock'))

    @staticmethod
    def lock_busy(path):
        with path.open('a') as stream:
            try:
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return True
        return False


if __name__ == '__main__':
    unittest.main()

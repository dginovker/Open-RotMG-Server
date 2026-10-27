import concurrent.futures
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest
import uuid
from unittest.mock import patch

SCRIPT = Path(__file__).with_name('qa.py').resolve()
spec = importlib.util.spec_from_file_location('qa', SCRIPT)
qa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(qa)


def eventually(check, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        value = check()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError('Timed out waiting for owned QA job')


def space(directory):
    return qa.scratch_path(directory, qa.load(directory))


class JobTests(unittest.TestCase):
    def setUp(self):
        self.base = Path(tempfile.mkdtemp(prefix='lifecycle-', dir=os.environ.get('QA_RESULTS_DIR')))
        self.short = Path(tempfile.mkdtemp(prefix='oq-', dir='/var/tmp'))
        self.env = {**os.environ, 'XDG_STATE_HOME': str(self.base), 'OPENROTMG_QA_SCRATCH_ROOT': str(self.short / 'q')}
        self.jobs = self.base / 'openrotmg/qa-jobs'
        self.addCleanup(self.stop_jobs)

    def stop_jobs(self):
        if self.jobs.exists():
            for directory in self.jobs.iterdir():
                if qa.ID.fullmatch(directory.name) and (directory / 'job.json').exists():
                    row = json.loads((directory / 'job.json').read_text())
                    if qa.unit_state(row['unit'])['ActiveState'] in ('activating', 'active'):
                        subprocess.run(['systemctl', '--user', 'stop', row['unit']], check=True, capture_output=True)
                    if 'cgroup' in row:
                        eventually(lambda: not qa.group_processes(row['cgroup']))
        shutil.rmtree(self.short)

    def invoke(self, *args, **kwargs):
        return subprocess.run([sys.executable, str(SCRIPT), *args], env=self.env, capture_output=True, text=True, timeout=25, **kwargs)

    def launch(self, code, *options):
        result = self.invoke(*options, 'exec', '--', sys.executable, '-c', code)
        self.assertIn('QA job: ', result.stdout, result.stderr)
        directory = Path(result.stdout.split('QA job: ', 1)[1].splitlines()[0])
        return result, directory

    def receipt(self, directory):
        path = directory / 'collection.json'
        eventually(path.exists)
        return json.loads(path.read_text())

    def test_non_git_source_does_not_publish_unattributed_job(self):
        source = self.base / 'not-a-checkout'
        source.mkdir()
        with patch.dict(os.environ, self.env), patch.object(qa, 'ROOT', source):
            with self.assertRaises(subprocess.CalledProcessError):
                qa.allocate('exec', ['true'])
        self.assertFalse(self.jobs.exists() and any(qa.ID.fullmatch(p.name) for p in self.jobs.iterdir()))

    def test_invalid_command_does_not_publish_unattributed_job(self):
        for kind, arguments in [('cargo', ['clean']), ('native', ['case.mjs', '--run-dir', '/tmp/not-owned']), ('exec', [])]:
            with self.subTest(kind=kind), patch.dict(os.environ, self.env):
                with self.assertRaises(RuntimeError):
                    qa.allocate(kind, arguments)
                self.assertFalse(self.jobs.exists() and any(qa.ID.fullmatch(p.name) for p in self.jobs.iterdir()))

    def test_receipt_published_during_terminal_state_query_is_read(self):
        with patch.dict(os.environ, self.env):
            directory = qa.allocate('exec', ['true'])
        def retire(unit):
            qa.save(directory / 'command.json', {'returncode': 0})
            qa.save(directory / 'collection.json', {'status': 'reclaimed', 'service_result': 'success'})
            return {'ActiveState': 'inactive'}
        with patch.object(qa, 'unit_state', side_effect=retire):
            self.assertEqual(qa.wait_job(directory), 0)

    def test_success_and_failure_keep_evidence_and_remove_only_scratch(self):
        for exit_code in (0, 37):
            with self.subTest(exit_code=exit_code):
                result, directory = self.launch("import os,pathlib,tempfile,sys; pathlib.Path(os.environ['QA_RESULTS_DIR'],'evidence.db').write_bytes(b'durable'); print(tempfile.mkstemp()[1]); print('error-stream',file=sys.stderr); sys.exit(" + str(exit_code) + ")")
                self.assertEqual(result.returncode, exit_code, result.stderr)
                self.assertEqual((directory / 'results/evidence.db').read_bytes(), b'durable')
                self.assertIn('error-stream', (directory / 'results/stderr.log').read_text())
                self.assertEqual(self.receipt(directory)['status'], 'reclaimed')
                self.assertFalse(space(directory).exists())
                self.assertEqual(qa.hashlib.sha256((directory / 'runner.py').read_bytes()).hexdigest(), json.loads((directory / 'job.json').read_text())['runner_sha256'])

    def waiting_child(self):
        return "import os,pathlib,time; p=pathlib.Path(os.environ['QA_RESULTS_DIR']); pathlib.Path(os.environ['QA_SCRATCH_DIR'],'alive').write_text('working'); (p/'ready').touch();\nwhile not (p/'release').exists(): time.sleep(.03)\n(p/'child-finished').touch()"

    def test_surviving_detached_child_holds_scratch_without_inheriting_fds(self):
        code = "import subprocess,sys; subprocess.Popen([sys.executable,'-c'," + repr(self.waiting_child()) + "],start_new_session=True,close_fds=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)"
        result, directory = self.launch(code, '--background')
        self.assertEqual(result.returncode, 0, result.stderr)
        eventually((directory / 'results/ready').exists)
        eventually((directory / 'command.json').exists)
        time.sleep(0.3)
        self.assertTrue((space(directory) / 'alive').exists())
        self.assertFalse((directory / 'collection.json').exists())
        self.assertEqual(qa.unit_state(json.loads((directory / 'job.json').read_text())['unit'])['ActiveState'], 'active')
        (directory / 'results/release').touch()
        self.assertEqual(self.receipt(directory)['status'], 'reclaimed')
        self.assertTrue((directory / 'results/child-finished').exists())
        self.assertFalse(space(directory).exists())

    def test_killed_foreground_runner_does_not_lose_job_or_evidence(self):
        process = subprocess.Popen([sys.executable, str(SCRIPT), 'exec', '--', sys.executable, '-c', self.waiting_child()], env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        self.addCleanup(lambda: process.poll() is None and process.kill())
        first = process.stdout.readline()
        directory = Path(first.removeprefix('QA job: ').strip())
        eventually((directory / 'results/ready').exists)
        process.kill()
        process.communicate(timeout=5)
        self.assertTrue((space(directory) / 'alive').exists())
        self.assertFalse((directory / 'collection.json').exists())
        (directory / 'results/release').touch()
        self.assertEqual(self.receipt(directory)['status'], 'reclaimed')
        self.assertTrue((directory / 'results/child-finished').exists())

    def test_cancel_and_timeout_retire_owned_processes_before_collection(self):
        result, directory = self.launch(self.waiting_child(), '--background')
        self.assertEqual(result.returncode, 0, result.stderr)
        eventually((directory / 'results/ready').exists)
        self.assertEqual(self.invoke('cancel', str(directory)).returncode, 0)
        self.assertEqual(self.receipt(directory)['status'], 'reclaimed')
        self.assertFalse(space(directory).exists())
        result, directory = self.launch(self.waiting_child(), '--timeout', '1')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.receipt(directory)['service_result'], 'timeout')
        self.assertFalse(space(directory).exists())

    def test_deadline_cannot_report_success_after_main_command_exits(self):
        code = "import subprocess,sys; subprocess.Popen([sys.executable,'-c'," + repr(self.waiting_child()) + "],start_new_session=True,close_fds=True)"
        result, directory = self.launch(code, '--timeout', '1')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(json.loads((directory / 'command.json').read_text())['returncode'], 0)
        self.assertEqual(self.receipt(directory)['service_result'], 'timeout')
        self.assertFalse(space(directory).exists())

    def test_concurrent_jobs_have_independent_lifetimes(self):
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            pairs = list(pool.map(lambda _: self.launch(self.waiting_child(), '--background'), range(2)))
        for result, directory in pairs:
            self.assertEqual(result.returncode, 0, result.stderr)
            eventually((directory / 'results/ready').exists)
        first, second = [directory for _, directory in pairs]
        (first / 'results/release').touch()
        self.assertEqual(self.receipt(first)['status'], 'reclaimed')
        self.assertTrue((space(second) / 'alive').exists())
        (second / 'results/release').touch()
        self.assertEqual(self.receipt(second)['status'], 'reclaimed')

    def test_explicit_hold_survives_future_jobs(self):
        result, directory = self.launch("import os,pathlib; pathlib.Path(os.environ['QA_JOB_DIR'],'preserve-scratch').touch(); pathlib.Path(os.environ['QA_SCRATCH_DIR'],'held').write_text('keep')")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.receipt(directory)['status'], 'preserved')
        result, _ = self.launch('pass')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((space(directory) / 'held').read_text(), 'keep')

    def test_symlink_inside_scratch_never_deletes_retained_target(self):
        result, directory = self.launch("import os,pathlib; p=pathlib.Path(os.environ['QA_RESULTS_DIR'],'keep.db'); p.write_text('keep'); pathlib.Path(os.environ['QA_SCRATCH_DIR'],'link').symlink_to(p)")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((directory / 'results/keep.db').read_text(), 'keep')
        self.assertFalse(space(directory).exists())

    def test_replaced_scratch_root_is_preserved_with_visible_error(self):
        result, directory = self.launch("import os,pathlib; p=pathlib.Path(os.environ['QA_SCRATCH_DIR']); (p/'.owner').unlink(); p.rmdir(); p.symlink_to(os.environ['QA_RESULTS_DIR']); pathlib.Path(os.environ['QA_RESULTS_DIR'],'keep.db').write_text('keep')")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.receipt(directory)['status'], 'error')
        self.assertEqual((directory / 'results/keep.db').read_text(), 'keep')
        self.assertTrue(space(directory).is_symlink())

    def test_unknown_existing_root_is_never_adopted(self):
        self.jobs.mkdir(parents=True)
        keep = self.jobs / 'keep.db'
        keep.write_text('original')
        result = self.invoke('exec', '--', 'true')
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(keep.read_text(), 'original')
        self.assertFalse((self.jobs / '.owner').exists())

    def test_uncertain_process_proof_preserves_orphan_scratch(self):
        _, directory = self.launch('pass')
        space(directory).mkdir(mode=0o700)
        (space(directory) / '.owner').write_text(str(directory))
        (space(directory) / 'keep').write_text('keep')
        (directory / 'collection.json').unlink()
        with patch.object(qa, 'group_processes', side_effect=PermissionError('cannot inspect cgroup')):
            with self.assertRaisesRegex(PermissionError, 'cannot inspect'):
                qa.collect(directory)
        self.assertEqual((space(directory) / 'keep').read_text(), 'keep')
        self.assertEqual(self.receipt(directory)['status'], 'error')
        # The next ordinary job automatically recovers a proven-dead orphan.
        result, _ = self.launch('pass')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.receipt(directory)['status'], 'reclaimed')
        self.assertFalse(space(directory).exists())

    def test_recovery_does_not_adopt_unknown_job_directories(self):
        _, _ = self.launch('pass')
        unknown = self.jobs / ('f' * 32)
        unknown.mkdir(mode=0o700)
        (unknown / 'keep.db').write_text('original')
        result, _ = self.launch('pass')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((unknown / 'keep.db').read_text(), 'original')
        self.assertFalse((unknown / 'collection.json').exists())

    def test_host_policy_preserves_a_held_descendant_without_releasing_it(self):
        policy = self.base / 'holds.json'
        policy.write_text(json.dumps({'version': 1, 'protectedPaths': [], 'protectedWords': ['held-sample']}))
        self.env['OPENROTMG_QA_PRESERVATION_FILE'] = str(policy)
        result, directory = self.launch("import os,pathlib; pathlib.Path(os.environ['QA_SCRATCH_DIR'],'HELD-sample').write_text('keep')")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.receipt(directory)['status'], 'preserved')
        self.assertEqual((space(directory) / 'HELD-sample').read_text(), 'keep')
        policy.write_text(json.dumps({'version': 1, 'protectedPaths': [str(space(directory) / 'HELD-sample')], 'protectedWords': []}))
        result, _ = self.launch('pass')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((space(directory) / 'HELD-sample').read_text(), 'keep')

    def test_invalid_or_missing_required_policy_preserves_scratch_but_does_not_gate_work(self):
        policy = self.base / 'holds.json'
        self.env['OPENROTMG_QA_PRESERVATION_FILE'] = str(policy)
        for contents in (None, '{}'):
            if contents is not None:
                policy.write_text(contents)
            result, directory = self.launch("import os,pathlib; pathlib.Path(os.environ['QA_RESULTS_DIR'],'ran').touch()")
            self.assertNotEqual(result.returncode, 0)
            self.assertTrue((directory / 'results/ran').exists())
            self.assertTrue(space(directory).exists())
            self.assertEqual(self.receipt(directory)['status'], 'error')

    def test_killed_service_worker_still_collects_after_its_children_retire(self):
        result, directory = self.launch(self.waiting_child(), '--background')
        self.assertEqual(result.returncode, 0, result.stderr)
        eventually((directory / 'results/ready').exists)
        row = json.loads((directory / 'job.json').read_text())
        subprocess.run(['systemctl', '--user', 'kill', '--kill-whom=main', '--signal=SIGKILL', row['unit']], check=True, capture_output=True)
        time.sleep(0.2)
        self.assertTrue((space(directory) / 'alive').exists())
        self.assertFalse((directory / 'collection.json').exists())
        (directory / 'results/release').touch()
        self.assertEqual(self.receipt(directory)['status'], 'reclaimed')
        eventually(lambda: not qa.group_processes(row['cgroup']))
        self.assertFalse(space(directory).exists())
        self.assertTrue((directory / 'results/ready').exists())

    def test_shared_cache_daemon_has_its_own_scratch_and_lifetime(self):
        name = 'test-' + uuid.uuid4().hex
        unit = 'openrotmg-cache-' + name + '.service'
        daemon = "import subprocess,sys; subprocess.Popen([sys.executable,'-c'," + repr(self.waiting_child()) + "],start_new_session=True,close_fds=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)"
        result = subprocess.run(['systemd-run', '--user', '--quiet', '--collect', '--service-type=notify', '--unit=' + unit,
                                 '--property=ExitType=cgroup', '--property=TimeoutStartSec=15', '--property=RuntimeMaxSec=30',
                                 '--property=ExecStopPost=' + sys.executable + ' ' + str(SCRIPT) + ' __service-collect',
                                 '--setenv=XDG_STATE_HOME', sys.executable, str(SCRIPT), '__service', name,
                                 sys.executable, '-c', daemon], env=self.env, capture_output=True, text=True, timeout=20)
        self.assertEqual(result.returncode, 0, result.stderr)
        backend = self.jobs / qa.unit_state(unit)['InvocationID']
        eventually((backend / 'results/ready').exists)
        config = self.base / 'config/openrotmg'
        config.mkdir(parents=True)
        (config / 'qa-services.json').write_text(json.dumps([unit]))
        self.env['XDG_CONFIG_HOME'] = str(config.parent)
        result, client = self.launch('pass')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.receipt(client)['status'], 'reclaimed')
        self.assertFalse(space(client).exists())
        self.assertTrue((space(backend) / 'alive').exists())
        self.assertFalse((backend / 'collection.json').exists())
        (backend / 'results/release').touch()
        self.assertEqual(self.receipt(backend)['status'], 'reclaimed')
        self.assertFalse(space(backend).exists())

    def test_scratch_supports_chromium_unix_sockets_on_disk(self):
        result, directory = self.launch("import os,pathlib,socket; p=pathlib.Path(os.environ['QA_SCRATCH_DIR'],'org.chromium.Chromium.ABCDEF'); p.mkdir(); s=socket.socket(socket.AF_UNIX); s.bind(str(p/'SingletonSocket')); s.close()")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.receipt(directory)['status'], 'reclaimed')
        self.assertFalse(space(directory).exists())

    def test_existing_scratch_directory_is_not_adopted_on_creation_collision(self):
        with patch.dict(os.environ, self.env, clear=True):
            directory = qa.allocate('exec', ['true'])
        space(directory).mkdir(mode=0o700)
        (space(directory) / 'original.db').write_text('original')
        group = '/nonexistent/' + qa.load(directory)['unit']
        with patch.object(qa, 'cgroup', return_value=group), patch.dict(os.environ, INVOCATION_ID='f' * 32):
            with self.assertRaises(FileExistsError):
                qa.run_job(directory)
        with self.assertRaises(FileNotFoundError):
            qa.collect(directory)
        self.assertEqual((space(directory) / 'original.db').read_text(), 'original')
        self.assertEqual(self.receipt(directory)['status'], 'error')

    def test_native_owns_allocation_and_forwards_scenario_flags(self):
        command = qa.command_for('native', ['case.mjs', '--client', '/client', '--timeout-ms', '900000'], qa.ROOT, self.base)
        self.assertEqual(command, ['node', str(qa.ROOT / 'scripts/native-test.mjs'), 'case.mjs', '--client', '/client', '--timeout-ms', '900000', '--run-dir', str(self.base / 'native')])
        with self.assertRaisesRegex(RuntimeError, 'owns --run-dir'):
            qa.command_for('native', ['case.mjs', '--run-dir=/old'], qa.ROOT, self.base)
        with self.assertRaisesRegex(RuntimeError, 'not global clean'):
            qa.command_for('cargo', ['clean'], qa.ROOT, self.base)


if __name__ == '__main__':
    unittest.main()

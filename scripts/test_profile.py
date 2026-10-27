import importlib.util
import json
import os
from pathlib import Path
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('profile_recorder', Path(__file__).with_name('profile.py'))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


class ProfileTests(unittest.TestCase):
    def event(self):
        now = time.monotonic_ns()
        return {'_PID': '42', 'MESSAGE': json.dumps({'timestamp': '2026-10-26T00:00:00Z', 'fields': {
            'message': 'latency incident', 'pid': 42, 'clock': 'CLOCK_MONOTONIC', 'world': 1,
            'sample': json.dumps({'execution': {'tid': 43, 'start_ns': now - 1000, 'end_ns': now}})}})}

    def test_storage_counts_unique_inodes_without_following_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'binary').write_bytes(b'x' * 4096)
            (root / 'capture').mkdir()
            os.link(root / 'binary', root / 'capture' / 'binary')
            os.symlink('binary', root / 'link')
            (root / 'dangling').symlink_to('absent')
            expected = sum(path.lstat().st_size for path in [
                root, root / 'capture', root / 'binary', root / 'link', root / 'dangling'])
            self.assertEqual(profile.storage_bytes(root), (expected, 0))

    def test_storage_survives_real_segment_deletion_during_stat(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            session = root / 'session-1-42'
            session.mkdir()
            segment = session / 'perf.data.123'
            segment.write_bytes(b'old segment')
            (session / 'binary').write_bytes(b'retained')
            original = Path.lstat
            def rotate(path, *args, **kwargs):
                if path == segment:
                    path.unlink()
                return original(path, *args, **kwargs)
            with patch.object(Path, 'lstat', rotate):
                size, vanished = profile.storage_bytes(root)
            self.assertGreaterEqual(size, len(b'retained'))
            self.assertEqual(vanished, 1)
            self.assertTrue((session / 'binary').is_file())

    def test_storage_keeps_unexpected_disappearance_and_permission_errors_fatal(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            identity = root / 'identity.json'
            identity.write_text('{}')
            original = Path.lstat
            def disappear(path, *args, **kwargs):
                if path == identity:
                    path.unlink()
                return original(path, *args, **kwargs)
            with patch.object(Path, 'lstat', disappear), self.assertRaises(FileNotFoundError):
                profile.storage_bytes(root)
            with patch.object(Path, 'lstat', side_effect=PermissionError('denied')):
                with self.assertRaises(PermissionError):
                    profile.storage_bytes(root)

    def test_storage_still_accounts_for_oversized_sparse_profiles(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (root / 'perf.data').open('wb') as output:
                output.truncate(2 * 1024**3 + 1)
            size, vanished = profile.storage_bytes(root)
            self.assertGreater(size, 2 * 1024**3)
            self.assertEqual(vanished, 0)

    def test_byte_budget_retires_oldest_eligible_incidents_using_unique_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            active = root / 'session-99-42'
            active.mkdir()
            (active / 'perf.data.123').write_bytes(b'x' * 50000)
            for n, size in [(0, 40000), (1, 0), (2, 80000), (3, 70000)]:
                capture = root / f'incident-{n}'
                capture.mkdir()
                profile.write(capture / 'complete.json', {})
                (capture / 'payload').write_bytes(b'x' * size)
            (root / 'incident-0' / 'retain').touch()
            os.link(active / 'perf.data.123', root / 'incident-1' / 'perf.data.123')
            limit = profile.storage_bytes(root)[0] - 60000
            with patch.object(profile, 'emit') as emit:
                self.assertLessEqual(profile.enforce_storage_limit(root, limit), limit)
            self.assertEqual([Path(call.kwargs['directory']).name for call in emit.call_args_list],
                             ['incident-1', 'incident-2'])
            self.assertTrue(all(call.args == ('profiling_incident_retired',)
                                and call.kwargs['reason'] == 'byte_budget' for call in emit.call_args_list))
            self.assertEqual(sorted(p.name for p in root.iterdir()),
                             ['incident-0', 'incident-3', 'session-99-42'])
            self.assertEqual((active / 'perf.data.123').stat().st_size, 50000)
            with patch.object(profile, 'emit') as emit:
                profile.enforce_storage_limit(root, limit)
                emit.assert_not_called()

    def test_byte_budget_never_discards_pinned_or_unfinished_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, size in [('session-9-42', 80000), ('incident-1', 40000), ('incident-2', 20000)]:
                path = root / name
                path.mkdir()
                (path / 'payload').write_bytes(b'x' * size)
                if name.startswith('incident'):
                    profile.write(path / 'complete.json', {})
            (root / 'incident-1' / 'retain').touch()
            with patch.object(profile, 'emit'), self.assertRaisesRegex(RuntimeError, 'pinned or active'):
                profile.enforce_storage_limit(root, 50000)
            self.assertEqual(sorted(p.name for p in root.iterdir()), ['incident-1', 'session-9-42'])
            self.assertEqual((root / 'incident-1' / 'payload').stat().st_size, 40000)
            self.assertEqual((root / 'session-9-42' / 'payload').stat().st_size, 80000)

    def test_byte_budget_refuses_external_links_before_retirement(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            capture = root / 'incident-1'
            capture.mkdir()
            profile.write(capture / 'complete.json', {})
            (capture / 'external').symlink_to('/etc/passwd')
            with self.assertRaisesRegex(RuntimeError, 'External symlink'):
                profile.enforce_storage_limit(root, 0)
            self.assertTrue((capture / 'complete.json').is_file())

    def test_exact_process_clock_and_execution_interval(self):
        event = self.event()
        self.assertEqual(profile.incident(json.dumps(event), 42)['sample']['execution']['tid'], 43)
        self.assertIsNone(profile.incident(json.dumps(event), 41))
        record = json.loads(event['MESSAGE'])
        record['fields']['clock'] = 'CLOCK_REALTIME'
        event['MESSAGE'] = json.dumps(record)
        with self.assertRaisesRegex(RuntimeError, 'clock'):
            profile.incident(json.dumps(event), 42)

    def test_journal_transport_preserves_large_normal_game_records(self):
        command = profile.journal_command('openrotmg')
        self.assertIn('--all', command)
        self.assertEqual(command[command.index('-o') + 1], 'json')
        event = self.event()
        record = json.loads(event['MESSAGE'])
        record['fields'] = {'message': 'world performance', 'timings': 'x' * 6000}
        event['MESSAGE'] = json.dumps(record)
        self.assertIsNone(profile.incident(json.dumps(event), 42))

    def test_unstructured_panic_is_explicit_and_not_a_fake_incident(self):
        with patch.object(profile, 'emit') as emit:
            self.assertIsNone(profile.incident(json.dumps({'_PID': '42', 'MESSAGE': 'thread panicked'}), 42))
            emit.assert_called_once_with('unstructured_target_log', pid=42)

    def test_retention_preserves_failed_pinned_and_current_sessions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for n in range(5):
                p = root / f'session-{n}-42'
                p.mkdir()
                if n != 1:
                    profile.write(p / 'complete.json', {})
            (root / 'session-0-42' / 'retain').touch()
            profile.retire(root, 'session-*', 1)
            self.assertEqual(sorted(p.name for p in root.iterdir()), ['session-0-42', 'session-1-42', 'session-4-42'])

    def test_retention_refuses_external_links(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for n in range(2):
                p = root / f'session-{n}-42'
                p.mkdir()
                profile.write(p / 'complete.json', {})
            os.symlink('/etc/passwd', root / 'session-0-42' / 'external')
            with self.assertRaisesRegex(RuntimeError, 'External symlink'):
                profile.retire(root, 'session-*', 1)
            self.assertTrue((root / 'session-0-42' / 'complete.json').exists())


if __name__ == '__main__':
    unittest.main()

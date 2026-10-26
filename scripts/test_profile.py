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

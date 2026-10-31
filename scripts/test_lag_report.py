import importlib.util
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('lag_report', Path(__file__).with_name('lag_report.py'))
lag_report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lag_report)


class LagReportTests(unittest.TestCase):
    def test_targets_causes_and_stalls_are_joined_by_world_and_time(self):
        t = datetime(2026, 10, 31, 6, 0, tzinfo=timezone.utc)
        miss = {'cause': 'contention', 'lateness_ms': 5.0, 'wall_ms': 88.0, 'cpu_ms': 27.0, 'cpu_wait_ms': 55.0, 'blocked_ms': 6.0}
        lines = [
            (t, {'message': 'world tick summary', 'world_id': 1, 'world': 'Realm', 'humans': 2, 'bots': 40, 'summary': json.dumps(
                {'step_wall': {'count': 100}, 'missed_by_cause': {'missed_contention': 2}})}),
            (t + timedelta(seconds=1), {'message': 'tick deadline missed', 'world': 1, 'miss': json.dumps(miss),
                                        'phases': json.dumps([{'phase': 'npc', 'wall_ms': 63.0}])}),
            (t + timedelta(seconds=1, milliseconds=200), {'message': 'player update stall', 'player': 7, 'world': 1, 'gap_ms': 695.0}),
            (t + timedelta(seconds=30), {'message': 'player update stall', 'player': 7, 'world': 1, 'gap_ms': 400.0}),
            (t + timedelta(seconds=5), {'message': 'host snapshot', 'host': json.dumps({
                'free_disk_bytes': 25 * 10**9, 'pressure_some_share': {'cpu': 0.3},
                'game_threads': {'world_cpu': {'running_cores': 0.8, 'cpu_wait_cores': 0.3}},
                'other_processes': [{'name': 'systemd-journal', 'cores': 0.03}]})}),
        ]
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, 'lag-0000493000.jsonl').write_text(''.join(
                json.dumps({'timestamp': at.isoformat().replace('+00:00', 'Z'), 'fields': f}) + '\n' for at, f in lines))
            out = lag_report.report(lag_report.records(directory, t, t + timedelta(minutes=1)))
        print(out)
        self.assertIn('tick misses: 2/100 = 2.0000% (target < 0.1%) BREACHED', out)
        self.assertIn('gaps over 250 ms: 2/~100 = 2.000% (target < 1%, i.e. p99 < 250 ms) BREACHED', out)
        self.assertIn('minimum free disk: 25.0 GB (target > 10 GB) OK', out)
        self.assertIn('gap 695 ms -> world tick contention (1 missed ticks)', out)
        self.assertIn('gap 400 ms -> beyond simulation', out)
        self.assertIn('world_cpu: running 0.80 cores, waiting for CPU 0.30 cores', out)
        self.assertIn('world 1 contention: late 5, wall 88, cpu 27, cpu_wait 55, blocked 6 ms; npc 63', out)


if __name__ == '__main__':
    unittest.main()

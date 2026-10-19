import hashlib
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import patch

from test_update import update


class RetentionTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.attempts = self.root / 'attempts'
        self.attempts.mkdir()
        self.live = self.root / 'server'
        self.tools = self.root / 'backups'
        (self.tools / 'releases').mkdir(parents=True)
        self.write_release(self.live, 8)
        self.patch = patch.multiple(update, ROOT=self.attempts, LIVE=self.live, BACKUPS=self.tools)
        self.patch.start()
        self.addCleanup(self.patch.stop)
        self.running = patch.object(update, 'active_paths', return_value=set()).start()
        self.addCleanup(patch.stopall)
        self.work(0, 'failed', stage=False)
        for n in range(1, 9):
            self.work(n)
        for n in range(9):
            contents = self.contents(n)
            bundle = self.tools / 'releases' / self.rev(n)
            for target, source in {'backup.py': 'scripts/backups/backup.py', 'openrotmg': 'openrotmg',
                                   'REVISION': 'REVISION', 'openrotmg-backup.sh': 'scripts/backups/openrotmg-backup.sh',
                                   'data/map.json': 'data/map.json'}.items():
                path = bundle / target
                path.parent.mkdir(exist_ok=True, parents=True)
                path.write_bytes(contents[source])
        (self.tools / 'current').symlink_to(self.tools / 'releases' / self.rev(8))
        self.data = self.root / 'state'
        self.data.mkdir()
        (self.data / 'game.db').write_bytes(b'live database')
        (self.data / 'server.pem').write_bytes(b'live key')

    def rev(self, n):
        return f'{n:040x}'

    def contents(self, n):
        return {'openrotmg': f'binary{n}'.encode(), 'REVISION': (self.rev(n) + '\n').encode(),
                'scripts/backups/backup.py': b'backup', 'scripts/backups/openrotmg-backup.sh': b'wrapper',
                'data/map.json': b'map'}

    def write_release(self, path, n):
        path.mkdir()
        for name, data in self.contents(n).items():
            target = path / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)

    def work(self, n, state='success', stage=True):
        work = self.attempts / (self.rev(n) + '-' + str(100 + n))
        work.mkdir()
        metadata = {'format': 1, 'revision': self.rev(n), 'manifest': {
            k: hashlib.sha256(v).hexdigest() for k, v in self.contents(n).items()}}
        (work / 'release.json').write_text(json.dumps(metadata))
        (work / 'server.tar.gz').write_bytes(b'archive')
        (work / 'shadow.log').write_text('diagnostics')
        source = None
        if stage:
            source = self.root / ('server-stage-' + str(1000 + n))
            self.write_release(source, n - 1 if state == 'success' else n)
        if state == 'success':
            (work / 'previous-server-path').write_text(str(source))
            (work / 'cutover').mkdir()
            (work / 'cutover/game.db').write_bytes(b'frozen data')
            (work / 'cutover/server.pem').write_bytes(b'frozen key')
        update.outcome(work, state, source)
        return work, source

    def test_successes_keep_current_and_two_verified_rollback_trees(self):
        result = update.retain_deployments()
        self.assertTrue(self.live.is_dir())
        self.assertEqual(sorted(p.name for p in self.root.glob('server-stage-*')), ['server-stage-1007', 'server-stage-1008'])
        self.assertEqual((self.root / 'server-stage-1007/REVISION').read_text().strip(), self.rev(6))
        self.assertEqual((self.root / 'server-stage-1008/REVISION').read_text().strip(), self.rev(7))
        self.assertEqual(result['kept_attempts'], 3)
        self.assertTrue((self.tools / 'releases' / self.rev(8)).exists())
        self.assertTrue((self.tools / 'releases' / self.rev(6)).exists())
        self.assertFalse((self.tools / 'releases' / self.rev(5)).exists())
        self.assertEqual((self.data / 'game.db').read_bytes(), b'live database')
        self.assertEqual((self.data / 'server.pem').read_bytes(), b'live key')

    def test_only_latest_three_pre_cutover_failure_sets_survive(self):
        rows = [self.work(n, 'failed') for n in range(10, 16)]
        update.retain_deployments()
        for i, (work, stage) in enumerate(rows):
            self.assertEqual(work.exists(), i >= 3)
            self.assertEqual(stage.exists(), i >= 3)

    def test_cutover_unknown_manual_pin_and_unconfirmed_child_are_preserved(self):
        rows = [self.work(n, state) for n, state in [(10, 'cutover'), (11, 'pinned'), (12, 'preparing'), (13, 'failed')]]
        (rows[3][0] / 'rehearsal-running').touch()
        unknown, _ = self.work(14, 'preparing')
        (unknown / 'outcome.json').unlink()
        manual = self.attempts / (self.rev(2) + '-102')
        (manual / 'retain').touch()
        for n in range(15, 20):
            self.work(n, 'failed')
        result = update.retain_deployments()
        self.assertEqual(result['unresolved_attempts'], 6)
        for work, stage in rows:
            self.assertTrue(work.exists() and stage.exists())
        self.assertTrue(unknown.exists() and manual.exists())

    def test_changed_stage_or_replaced_directory_is_pinned(self):
        stage = self.root / 'server-stage-1002'
        (stage / 'openrotmg').write_bytes(b'changed')
        renewed = self.root / 'server-stage-1003'
        shutil.move(renewed, self.root / 'original-stage')
        self.write_release(renewed, 2)
        result = update.retain_deployments()
        self.assertEqual(len(result['review']), 2)
        for n in [2, 3]:
            self.assertTrue((self.attempts / (self.rev(n) + '-' + str(100 + n))).exists())
            self.assertTrue((self.root / ('server-stage-' + str(1000 + n))).exists())

    def test_running_source_workspace_and_tool_are_preserved(self):
        work = self.attempts / (self.rev(4) + '-104')
        self.running.return_value = {self.root / 'server-stage-1002/openrotmg', work / 'shadow.log',
                                     self.tools / 'releases' / self.rev(5) / 'openrotmg'}
        update.retain_deployments()
        self.assertTrue((self.root / 'server-stage-1002').exists())
        self.assertTrue(work.exists())
        self.assertTrue((self.tools / 'releases' / self.rev(5)).exists())

    def test_kept_unknown_attempt_can_pin_an_expiring_attempts_stage(self):
        work, stage = self.work(10, 'pinned')
        shared = self.root / 'server-stage-1002'
        (work / 'previous-server-path').write_text(str(shared))
        update.retain_deployments()
        self.assertTrue(shared.exists())

    def test_invalid_stage_path_or_symlink_aborts_before_any_delete(self):
        work = self.attempts / (self.rev(1) + '-101')
        for target in [self.live, self.root / 'server-stage-9999']:
            with self.subTest(target=target):
                if target != self.live:
                    target.symlink_to(self.live, target_is_directory=True)
                update.outcome(work, 'success', target)
                (work / 'previous-server-path').write_text(str(target))
                with self.assertRaisesRegex(RuntimeError, 'Unsafe retained stage'):
                    update.retain_deployments()
                self.assertTrue((self.attempts / (self.rev(2) + '-102')).exists())
                self.assertTrue(self.live.exists())

    def test_dry_run_is_non_mutating_and_repeated_cleanup_is_idempotent(self):
        before = update.files(self.root)
        planned = update.retain_deployments(apply=False)
        self.assertEqual(update.files(self.root), before)
        applied = update.retain_deployments()
        self.assertEqual(planned['remove'], applied['remove'])
        self.assertEqual(update.retain_deployments()['remove'], [])

    def test_invalid_receipt_or_backup_pointer_aborts_before_any_delete(self):
        receipt = self.attempts / (self.rev(1) + '-101/outcome.json')
        original = receipt.read_text()
        receipt.write_text('{"format":99,"state":"success"}')
        with self.assertRaisesRegex(RuntimeError, 'Invalid deployment outcome'):
            update.retain_deployments()
        receipt.write_text(original)
        (self.tools / 'current').unlink()
        with self.assertRaisesRegex(RuntimeError, 'Backup tool pointer is invalid'):
            update.retain_deployments()
        self.assertTrue((self.attempts / (self.rev(1) + '-101')).exists())

    def test_artifact_only_legacy_directories_expire_but_extra_evidence_blocks_pruning(self):
        work, _ = self.work(20, 'artifact_only', stage=False)
        (work / 'shadow.log').unlink()
        update.retain_deployments()
        self.assertFalse(work.exists())
        work, _ = self.work(21, 'artifact_only', stage=False)
        with self.assertRaisesRegex(RuntimeError, 'unclassified evidence'):
            update.retain_deployments()
        self.assertTrue(work.exists())

    def test_success_receipt_requires_the_frozen_save_and_previous_pointer(self):
        work = self.attempts / (self.rev(2) + '-102')
        (work / 'cutover/server.pem').unlink()
        with self.assertRaisesRegex(RuntimeError, 'Invalid successful cutover receipt'):
            update.retain_deployments()
        self.assertTrue(work.exists())

    def test_changed_or_unknown_backup_tool_bundle_is_reported_not_deleted(self):
        changed = self.tools / 'releases' / self.rev(2)
        (changed / 'openrotmg').write_bytes(b'changed')
        unknown = self.tools / 'releases' / self.rev(99)
        unknown.mkdir()
        (unknown / 'game.db').write_bytes(b'unknown data')
        result = update.retain_deployments()
        self.assertEqual(len(result['review']), 2)
        self.assertTrue(changed.exists() and unknown.exists())


if __name__ == '__main__':
    unittest.main()

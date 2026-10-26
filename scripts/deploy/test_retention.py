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
        process_patch = patch.object(update, 'active_paths', return_value=set())
        self.running = process_patch.start()
        self.addCleanup(process_patch.stop)
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









if __name__ == '__main__':
    unittest.main()

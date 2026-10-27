import hashlib
import importlib.util
import io
import contextlib
import json
import os
import socket
import subprocess
import sys
import sqlite3
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import unittest.mock
import urllib.error
from contextlib import closing, contextmanager

spec = importlib.util.spec_from_file_location('update', Path(__file__).with_name('update.py'))
update = importlib.util.module_from_spec(spec)
spec.loader.exec_module(update)


class UpdateTest(unittest.TestCase):

    def test_restart_warning_announces_then_waits_ninety_seconds(self):
        config = {'bind': '127.0.0.1:18923'}
        notice = 'Server restarting in 90 seconds to apply an update.'
        events = []
        with patch.object(update.subprocess, 'check_output', return_value='active\n'), \
                patch.object(update, 'announce', side_effect=lambda c, text: events.append(('announce', c, text)) or {'players': 2}), \
                patch.object(update.time, 'sleep', side_effect=lambda seconds: events.append(('sleep', seconds))), \
                contextlib.redirect_stdout(io.StringIO()) as output:
            self.assertTrue(update.warn_restart(config))
        self.assertEqual(events, [('announce', config, notice), ('sleep', 90)])
        self.assertEqual(json.loads(output.getvalue()), {'restart_warning': notice, 'seconds': 90, 'players': 2})


    def test_helper_install_tracks_release_and_rejects_invalid_source(self):
        with tempfile.TemporaryDirectory() as directory:
            installed, source = Path(directory) / 'update.py', Path(directory) / 'source.py'
            installed.write_text('old = 1\n')
            source.write_text('old = 1\n')
            with patch.object(update, 'HELPER', installed), contextlib.redirect_stdout(io.StringIO()):
                self.assertFalse(update.install_helper(source))
                source.write_text('new = 2\n')
                self.assertTrue(update.install_helper(source))
                self.assertEqual(installed.read_text(), 'new = 2\n')
                self.assertEqual(installed.stat().st_mode & 0o777, 0o755)
                source.write_text('def broken(:\n')
                with self.assertRaises(SyntaxError):
                    update.install_helper(source)
                self.assertEqual(installed.read_text(), 'new = 2\n')

    def test_caddy_install_reloads_changes_and_restores_on_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            live, source = Path(directory) / 'Caddyfile', Path(directory) / 'source'
            live.write_text('old')
            source.write_text('old')
            calls = []
            with patch.object(update, 'CADDY', live), patch.object(update, 'run', side_effect=lambda *a: calls.append(a)):
                self.assertFalse(update.install_caddy(source))
                self.assertEqual(calls, [])
                source.write_text('new')
                ok = unittest.mock.MagicMock(status=200)
                ok.__enter__.return_value = ok
                with patch.object(update.urllib.request, 'urlopen', return_value=ok), contextlib.redirect_stdout(io.StringIO()):
                    self.assertTrue(update.install_caddy(source))
                self.assertEqual(live.read_text(), 'new')
                self.assertEqual([c[0] for c in calls], ['caddy', 'systemctl'])
                source.write_text('broken')
                with patch.object(update.urllib.request, 'urlopen', side_effect=urllib.error.URLError('down')), \
                        self.assertRaises(urllib.error.URLError):
                    update.install_caddy(source)
                self.assertEqual(live.read_text(), 'new')

    def test_vault_migration_preserves_saved_rows_with_unread_accounts_before_between_and_after(self):
        before = {'accounts': [{'id': account} for account in range(1, 6)], 'vaults': [
            {'account': 2, 'items': '[2594,-1,-1,-1,-1,-1,-1,-1]', 'revision': 37},
            {'account': 4, 'items': '[-1,2595,-1,-1,-1,-1,-1,-1]', 'revision': 9}]}
        after = {'accounts': [{**row, 'vault_mask': 1} for row in before['accounts']], 'vaults': [
            {'account': 1, 'chest': 0, 'items': '[-1,-1,-1,-1,-1,-1,-1,-1]', 'revision': 0},
            {**before['vaults'][0], 'chest': 0},
            {'account': 3, 'chest': 0, 'items': '[-1,-1,-1,-1,-1,-1,-1,-1]', 'revision': 0},
            {**before['vaults'][1], 'chest': 0},
            {'account': 5, 'chest': 0, 'items': '[-1,-1,-1,-1,-1,-1,-1,-1]', 'revision': 0}]}
        update.verify_migration(before, after)
        for rows in [after['vaults'][:1] + after['vaults'][2:],
                     after['vaults'][:2] + after['vaults'][3:],
                     [{**row, 'items': '[-1,-1,-1,-1,-1,-1,-1,-1]'} if row['account'] == 2 else row for row in after['vaults']],
                     [{**row, 'revision': 0} if row['account'] == 4 else row for row in after['vaults']],
                     after['vaults'] + [{'account': 2, 'chest': 1, 'items': '[-1,-1,-1,-1,-1,-1,-1,-1]', 'revision': 0}]]:
            with self.subTest(rows=rows), self.assertRaisesRegex(RuntimeError, 'Vault migration'):
                update.verify_migration(before, {**after, 'vaults': rows})

    def test_all_death_fame_phase2_migration_preserves_paid_bytes_and_known_only_history(self):
        from copy import deepcopy
        payload = {'schema': 1, 'class': 782, 'level': 20, 'fame': 19, 'tiles': 4000001, 'hits': 999999, 'quests': 999999}
        raw = json.dumps(payload, indent=2)
        lifetime = {'version': 1, 'completeness': 'known_only', 'shots': 0, 'projectile_hits': 0,
                    'kills': 0, 'god_kills': 0, 'cube_kills': 0, 'oryx_kills': 0,
                    'inventory_potion_used': False, 'ability_used': False, 'teleported': False,
                    'tiles': 4000001, 'level_assists': 0, 'qualified_quests': 0, 'realm_exits': []}
        before = {'accounts': [{'id': 1}], 'characters': [{'id': 3, 'account': 1, 'stats': raw}],
                  'deaths': [{'character': 3, 'snapshot': raw, 'equipment_fame': 1, 'died_at': 1700000000}],
                  'account_fame': [{'account': 1, 'balance': 20, 'earned': 20}]}
        update.verify_migration(before, before)
        for partial in [{**before, 'legacy_claims': []}, {**before, 'level_credits': []}]:
            with self.subTest(partial=list(partial)), self.assertRaises(RuntimeError):
                update.verify_migration(before, partial)
        after = deepcopy(before)
        after['characters'][0]['stats'] = json.dumps(dict(payload, lifetime=lifetime))
        after['deaths'][0]['receipt'] = json.dumps({'version': 1, 'policy': 'legacy_paid', 'base': 19,
                                                  'rows': [{'kind': 'well_equipped', 'amount': 1}], 'total': 20})
        after['legacy_claims'], after['level_credits'] = [], []
        update.verify_migration(before, after)
        update.verify_migration(after, after)
        for case in range(9):
            bad = deepcopy(after)
            if case == 0:
                bad['characters'] = []
            elif case == 1:
                bad['characters'][0]['stats'] = json.dumps(dict(payload, hits=0, lifetime=lifetime))
            elif case == 2:
                bad['characters'][0]['stats'] = json.dumps(dict(payload, lifetime=dict(lifetime, completeness='complete')))
            elif case == 3:
                bad['characters'][0]['stats'] = json.dumps(dict(payload, lifetime=dict(lifetime, projectile_hits=999999)))
            elif case == 4:
                bad['deaths'][0]['snapshot'] += ' '
            elif case == 5:
                bad['deaths'][0]['receipt'] = after['deaths'][0]['receipt'].replace('"total": 20', '"total": 21')
            elif case == 6:
                bad['account_fame'][0]['earned'] += 1
            elif case == 7:
                bad['legacy_claims'] = [{'account': 1, 'class': 782, 'character': 3}]
            else:
                bad['level_credits'] = [{'recipient': 3, 'reached_level': 20, 'giver': 4}]
            with self.subTest(case=case), self.assertRaises(RuntimeError):
                update.verify_migration(before, bad)
        legacy_zero = deepcopy(before)
        legacy_payload = dict(payload, level=1, exp=0)
        del legacy_payload['schema'], legacy_payload['class']
        legacy_zero['characters'][0]['stats'] = json.dumps(legacy_payload)
        migrated_zero = deepcopy(after)
        migrated_zero['characters'][0]['stats'] = json.dumps(dict(legacy_payload, schema=1, **{'class': 782}, base=[100, 100, 12, 0, 10, 15, 12, 12], lifetime=lifetime))
        update.verify_migration(legacy_zero, migrated_zero)
        bad_zero = deepcopy(migrated_zero)
        bad_zero['characters'][0]['stats'] = bad_zero['characters'][0]['stats'].replace('782', '784')
        with self.assertRaisesRegex(RuntimeError, 'Fame migration'):
            update.verify_migration(legacy_zero, bad_zero)
        bad = deepcopy(after)
        bad['deaths'][0]['receipt'] = after['deaths'][0]['receipt'].replace('20', '21')
        with self.assertRaisesRegex(RuntimeError, 'Migration changed existing records'):
            update.verify_migration(after, bad)



    def delivery(self, transform=lambda entries: entries):
        contents = {'openrotmg': b'binary', 'REVISION': b'a' * 40 + b'\n'}
        inner = io.BytesIO()
        with tarfile.open(fileobj=inner, mode='w:gz') as archive:
            for name, value in contents.items():
                entry = tarfile.TarInfo(name)
                entry.size = len(value)
                archive.addfile(entry, io.BytesIO(value))
        metadata = {'format': 1, 'revision': 'a' * 40, 'binary_sha256': hashlib.sha256(b'binary').hexdigest(),
                    'manifest': {k: hashlib.sha256(v).hexdigest() for k, v in contents.items()},
                    'archive_sha256': hashlib.sha256(inner.getvalue()).hexdigest()}
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode='w:') as archive:
            for name, data in transform([('release.json', json.dumps(metadata).encode()), ('server.tar.gz', inner.getvalue())]):
                entry = tarfile.TarInfo(name)
                entry.size = len(data)
                archive.addfile(entry, io.BytesIO(data))
        return output.getvalue(), metadata

    @contextmanager
    def deployment(self, current='b' * 40):
        raw, metadata = self.delivery()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            live = root / 'server'
            live.mkdir()
            (live / 'REVISION').write_text(current + '\n')
            (live / 'openrotmg').write_bytes(b'binary')
            work = root / 'attempts'
            (work / 'incident').mkdir(parents=True)
            (work / 'incident/frozen.db').write_bytes(b'preserve incident')
            config = root / 'config.json'
            config.write_text(json.dumps({'bind': '127.0.0.1:18923', 'state': str(root)}))
            (root / 'backup.lock').touch()
            (root / 'receipt.json').write_text('{"restored":true}')
            with patch.multiple(update, ROOT=work, LIVE=live, CONFIG=config, LAST_BACKUP=root / 'receipt.json'), \
                    patch.object(update, 'main_revision', return_value='a' * 40):
                yield raw, metadata, live, work
            self.assertEqual((work / 'incident/frozen.db').read_bytes(), b'preserve incident')








    def test_delivery_rejects_stale_main_unsafe_paths_corruption_and_limits(self):
        raw, _ = self.delivery()
        with patch.object(update, 'main_revision', return_value='b' * 40), patch.object(update, 'deploy') as deploy:
            with self.assertRaisesRegex(RuntimeError, 'Main advanced'):
                update.main('a' * 40, raw)
            deploy.assert_not_called()
        for transform, error in [
                (lambda entries: entries + [('../escape', b'bad')], 'Expected only'),
                (lambda entries: entries + [entries[0]], 'Expected only'),
                (lambda entries: [(n, b'corrupt' if n == 'server.tar.gz' else d) for n, d in entries], 'archive mismatch')]:
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                (root / 'REVISION').write_text('b' * 40)
                raw, _ = self.delivery(transform)
                with patch.object(update, 'ROOT', root / 'state'), patch.object(update, 'LIVE', root), \
                        patch.object(update, 'main_revision', return_value='a' * 40), patch.object(update, 'deploy') as deploy:
                    with self.assertRaisesRegex(RuntimeError, error):
                        update.main('a' * 40, raw)
                    deploy.assert_not_called()
                    self.assertEqual(json.loads((root / 'state/status.json').read_text())['state'], 'failed')
        with patch.object(update, 'main_revision', return_value='a' * 40), patch.object(update, 'LIMIT', 1):
            with self.assertRaisesRegex(RuntimeError, 'exceeds'):
                update.main('a' * 40, raw)











    def test_uploaded_backup_requires_fresh_restore_and_actual_running_revision(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'REVISION').write_text('a' * 40)
            receipt = root / 'receipt.json'
            proof = {'uploaded_at': '2026-10-15T12:00:00+00:00', 'restored': True, 'revisions': {'server': 'a' * 40}}
            with patch.object(update, 'LIVE', root), patch.object(update, 'LAST_BACKUP', receipt):
                receipt.write_text(json.dumps(proof))
                stamp = update.datetime.datetime.fromisoformat(proof['uploaded_at']).timestamp()
                update.verify_backup(stamp - 1)
                with self.assertRaisesRegex(RuntimeError, 'Fresh uploaded backup'):
                    update.verify_backup(stamp + 1)
                for change in [{'restored': False}, {'revisions': {'server': 'b' * 40}}]:
                    receipt.write_text(json.dumps({**proof, **change}))
                    with self.assertRaisesRegex(RuntimeError, 'Fresh uploaded backup'):
                        update.verify_backup(stamp - 1)







class DrainTest(unittest.TestCase):
    process = {'pid': 123, 'start_ticks': 456, 'binary_sha256': 'e' * 64}
    receipt = {'instance': 'a' * 64, 'generation': 1}

    def result(self, state='ready', **fields):
        return {'receipt': self.receipt, 'state': state, 'errors': [], **fields}

    def environment(self, root, request):
        stack = contextlib.ExitStack()
        stack.enter_context(patch.object(update, 'drain_process', return_value=self.process))
        stack.enter_context(patch.object(update, 'drain_request', side_effect=request))
        stack.enter_context(patch.object(update.time, 'sleep'))
        run = stack.enter_context(patch.object(update, 'run'))
        return stack, run





















if __name__ == '__main__':
    unittest.main()

from contextlib import closing
import os
import gzip
import json
from copy import deepcopy
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest

import backup
from test_release import legacy_copy


class DeathSchema(unittest.TestCase):
    def test_export_rejects_noncanonical_current_metadata_without_writing(self):
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            subprocess.run([os.environ['OPENROTMG_BACKUP_BINARY'], '--backup-fixture',
                            str(directory / 'written'), str(root / 'data')], check=True)
            with closing(sqlite3.connect(directory / 'levels.sqlite')) as db, db:
                db.execute('CREATE TABLE levels(guild_id TEXT,user_id TEXT,username TEXT,xp INTEGER,last_xp_at INTEGER,last_message_id TEXT,PRIMARY KEY(guild_id,user_id))')
            revision = directory / 'REVISION'
            revision.write_text('a' * 40)
            original = backup.export(directory / 'written/game.db', directory / 'levels.sqlite', directory / 'current.gz', revision, revision, revision)
            legacy = directory / 'legacy.gz'
            legacy.write_bytes(gzip.compress(json.dumps(legacy_copy(original)).encode()))
            baseline = directory / 'baseline'
            backup.restore(legacy, baseline)
            subprocess.run([os.environ['OPENROTMG_BACKUP_BINARY'], '--backup-load', str(baseline / 'game.db'), str(root / 'data')], check=True)
            valid = backup.export(baseline / 'game.db', baseline / 'levels.sqlite', directory / 'valid.gz', revision, revision, revision)
            self.assertEqual(valid['schema'], 18)
            self.assertEqual(valid['game']['legacy_claims'], [])
            for case, (old, new) in enumerate([
                ('character INTEGER PRIMARY KEY', 'character INTEGER'),
                ('character INTEGER PRIMARY KEY', 'character TEXT PRIMARY KEY'),
                ('character INTEGER PRIMARY KEY', 'character INTEGER NOT NULL PRIMARY KEY'),
                ('character INTEGER PRIMARY KEY', 'character INTEGER PRIMARY KEY DEFAULT 0'),
                ('snapshot TEXT NOT NULL', 'snapshot BLOB NOT NULL'),
                ('snapshot TEXT NOT NULL', 'snapshot TEXT'),
                ('snapshot TEXT NOT NULL', "snapshot TEXT NOT NULL DEFAULT '{}'"),
                ('killed_by TEXT NOT NULL', 'killed_by BLOB NOT NULL'),
                ('killed_by TEXT NOT NULL', 'killed_by TEXT'),
                ('killed_by TEXT NOT NULL', "killed_by TEXT NOT NULL DEFAULT ''"),
                ('died_at INTEGER NOT NULL DEFAULT(unixepoch())', 'died_at TEXT NOT NULL DEFAULT(unixepoch())'),
                ('died_at INTEGER NOT NULL DEFAULT(unixepoch())', 'died_at INTEGER DEFAULT(unixepoch())'),
                ('died_at INTEGER NOT NULL DEFAULT(unixepoch())', 'died_at INTEGER NOT NULL'),
                ('died_at INTEGER NOT NULL DEFAULT(unixepoch())', 'died_at INTEGER NOT NULL DEFAULT 0'),
                ('equipment_fame INTEGER NOT NULL', 'equipment_fame TEXT NOT NULL'),
                ('equipment_fame INTEGER NOT NULL', 'equipment_fame INTEGER'),
                ('equipment_fame INTEGER NOT NULL', 'equipment_fame INTEGER NOT NULL DEFAULT 0'),
                ('equipment_fame INTEGER NOT NULL', 'equipment_fame INTEGER NOT NULL GENERATED ALWAYS AS (0) STORED'),
            ]):
                with self.subTest(metadata=new):
                    database = directory / 'malformed.db'
                    with closing(sqlite3.connect(baseline / 'game.db')) as source, closing(sqlite3.connect(database)) as db:
                        source.backup(db)
                        claims_sql = db.execute("SELECT sql FROM sqlite_schema WHERE name='legacy_claims'").fetchone()[0]
                        db.execute('PRAGMA foreign_keys=OFF')
                        db.execute('PRAGMA legacy_alter_table=ON')
                        sql = db.execute("SELECT sql FROM sqlite_schema WHERE name='deaths'").fetchone()[0]
                        changed = sql.replace(old, new)
                        if new == 'equipment_fame TEXT NOT NULL':
                            changed = changed.replace(" CHECK(typeof(equipment_fame)='integer' AND equipment_fame BETWEEN 0 AND 2147483647)", '')
                        self.assertNotEqual(sql, changed)
                        columns = 'character,snapshot,killed_by,died_at' + ('' if 'GENERATED' in new else ',equipment_fame') + ',receipt'
                        db.executescript(f'ALTER TABLE deaths RENAME TO preserved_deaths; {changed}; INSERT INTO deaths({columns}) SELECT {columns} FROM preserved_deaths; DROP TABLE preserved_deaths; CREATE INDEX deaths_by_time ON deaths(died_at);')
                        if new == 'character INTEGER':
                            db.execute('CREATE UNIQUE INDEX fixture_death_parent ON deaths(character)')
                        self.assertEqual(db.execute("SELECT sql FROM sqlite_schema WHERE name='legacy_claims'").fetchone()[0], claims_sql)
                        self.assertEqual(db.execute("SELECT \"table\" FROM pragma_foreign_key_list('legacy_claims') WHERE \"from\"='character'").fetchall(), [('deaths',)])
                        self.assertFalse(db.execute("SELECT 1 FROM sqlite_schema WHERE sql LIKE '%preserved_deaths%'").fetchall())
                        self.assertFalse(db.execute('PRAGMA foreign_key_check').fetchall())
                    before = database.read_bytes()
                    with self.assertRaisesRegex(ValueError, 'death.*metadata'):
                        backup.export(database, directory / 'levels.sqlite', directory / f'rejected-{case}.gz',
                                      revision, revision, revision)
                    self.assertEqual(database.read_bytes(), before)
                    self.assertFalse((directory / f'rejected-{case}.gz').exists())

    def test_all_death_fame_phase2_paid_receipts_claims_credits_roundtrip_corruption(self):
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            binary = os.environ['OPENROTMG_BACKUP_BINARY']
            written = directory / 'written'
            subprocess.run([binary, '--backup-fixture', str(written), str(root / 'data')], check=True)
            with closing(sqlite3.connect(directory / 'levels.sqlite')) as db, db:
                db.execute('CREATE TABLE levels(guild_id TEXT,user_id TEXT,username TEXT,xp INTEGER,last_xp_at INTEGER,last_message_id TEXT,PRIMARY KEY(guild_id,user_id))')
            revision = directory / 'REVISION'
            revision.write_text('a' * 40)
            # The full-policy writer itself now provides the paid Legacy/claim binding.
            with closing(sqlite3.connect(written / 'game.db')) as db:
                character, raw = db.execute('SELECT character,receipt FROM deaths ORDER BY character LIMIT 1').fetchone()
                receipt = json.loads(raw)
                self.assertEqual(receipt['policy'], 'historical')
                self.assertTrue(any(row['kind'] == 'legacy_builder' for row in receipt['rows']))
                self.assertEqual(db.execute('SELECT character FROM legacy_claims').fetchall(), [(character,)])
            exercise = subprocess.run([binary, '--backup-exercise-death', str(written / 'game.db'), str(root / 'data')], capture_output=True, text=True, check=True)
            proof = json.loads(exercise.stdout)
            self.assertEqual(len(proof['level_credits']), 2)
            archive = directory / 'paid.gz'
            bundle = backup.export(written / 'game.db', directory / 'levels.sqlite', archive, revision, revision, revision)
            self.assertEqual(bundle['schema'], 18)
            self.assertEqual(len(bundle['game']['bot_minds']), 2)
            self.assertEqual(len(bundle['game']['trade_transactions']), 1)
            for case in ('revision', 'unknown_mind', 'owners', 'revisions', 'market_key'):
                bad = deepcopy(bundle)
                if case == 'revision':
                    bad['game']['bot_minds'][0]['revision'] += 1
                elif case == 'unknown_mind':
                    mind = json.loads(bad['game']['bot_minds'][0]['mind'])
                    mind['credential'] = 'must-not-export'
                    bad['game']['bot_minds'][0]['mind'] = json.dumps(mind)
                elif case == 'owners':
                    bad['game']['trade_transactions'][0]['owners'] = '[2147483647]'
                elif case == 'revisions':
                    bad['game']['trade_transactions'][0]['revisions'] = '[2147483647]'
                else:
                    bad['game']['trade_transactions'][0]['market_key'] = 'unbound:0'
                with self.subTest(community=case), self.assertRaises(ValueError):
                    backup.validate(bad)

            self.assertEqual(len(bundle['game']['legacy_claims']), 1)
            self.assertEqual(len(bundle['game']['level_credits']), 2)
            restored = directory / 'restored'
            backup.restore(archive, restored)
            subprocess.run([binary, '--backup-load', str(restored / 'game.db'), str(root / 'data')], check=True)
            again = backup.export(restored / 'game.db', restored / 'levels.sqlite', directory / 'again.gz', revision, revision, revision)
            self.assertEqual(again['game'], bundle['game'])
            self.assertEqual(again['discord'], bundle['discord'])
            for case in range(17):
                bad = deepcopy(bundle)
                paid = json.loads(bad['game']['deaths'][-1]['receipt'])
                if case == 0:
                    paid['version'] = 99
                elif case == 1:
                    paid['policy'] = 'unknown'
                elif case == 2:
                    paid['rows'][0]['kind'] = 'unknown'
                elif case == 3:
                    paid['rows'][0]['amount'] = -1
                elif case == 4:
                    paid['rows'] *= 2
                elif case == 5:
                    paid['total'] += 1
                elif case == 6:
                    paid['rows'] = [{'kind': 'first_born', 'amount': 0}, {'kind': 'ancestor', 'amount': 0}]
                elif case == 7:
                    bad['game']['legacy_claims'][0]['class'] += 1
                elif case == 8:
                    bad['game']['legacy_claims'][0]['account'] = 999999
                elif case == 9:
                    bad['game']['level_credits'][0]['giver'] = 999999
                elif case == 10:
                    bad['game']['level_credits'][0]['reached_level'] = 20
                elif case == 11:
                    bad['game']['level_credits'][0]['giver'] = bad['game']['level_credits'][0]['recipient']
                elif case == 12:
                    bad['game']['level_credits'] *= 2
                elif case == 13:
                    row = bad['game']['characters'][0]
                    payload = json.loads(row['stats'])
                    del payload['lifetime']
                    row['stats'] = json.dumps(payload)
                elif case == 14:
                    row = bad['game']['deaths'][-1]
                    payload = json.loads(row['snapshot'])
                    del payload['lifetime']
                    row['snapshot'] = json.dumps(payload)
                elif case == 15:
                    paid['version'] = True
                elif case == 16:
                    paid['rows'][0]['amount'] = 2147483648
                bad['game']['deaths'][-1]['receipt'] = json.dumps(paid)
                rejected = directory / f'rejected-{case}'
                invalid = directory / f'invalid-{case}.gz'
                invalid.write_bytes(gzip.compress(json.dumps(bad).encode()))
                with self.subTest(case=case), self.assertRaises(ValueError):
                    backup.restore(invalid, rejected)
                self.assertFalse(rejected.exists())
            bad = deepcopy(bundle)
            raw = bad['game']['deaths'][-1]['receipt']
            bad['game']['deaths'][-1]['receipt'] = raw.replace('{', '{"version": 1,', 1)
            with self.assertRaisesRegex(ValueError, 'Duplicate death receipt'):
                backup.validate(bad)

    def test_all_death_fame_phase2_schema15_and16_old_paid_bytes_no_retrocredit(self):
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            binary = os.environ['OPENROTMG_BACKUP_BINARY']
            subprocess.run([binary, '--backup-fixture', str(directory / 'written'), str(root / 'data')], check=True)
            with closing(sqlite3.connect(directory / 'levels.sqlite')) as db, db:
                db.execute('CREATE TABLE levels(guild_id TEXT,user_id TEXT,username TEXT,xp INTEGER,last_xp_at INTEGER,last_message_id TEXT,PRIMARY KEY(guild_id,user_id))')
            revision = directory / 'REVISION'
            revision.write_text('a' * 40)
            original = backup.export(directory / 'written/game.db', directory / 'levels.sqlite', directory / 'original.gz', revision, revision, revision)
            for version in (15, 16):
                old = legacy_copy(original)
                if version == 16:
                    old['schema'] = 16
                    for table in ('bot_minds', 'trade_transactions'):
                        old['game'][table] = deepcopy(original['game'][table])
                for table, field in [('characters', 'stats'), ('deaths', 'snapshot')]:
                    for row in old['game'][table]:
                        payload = json.loads(row[field])
                        payload['tiles'] = 4_000_001
                        row[field] = json.dumps(payload, indent=2)
                archive = directory / f'schema{version}.gz'
                archive.write_bytes(gzip.compress(json.dumps(old).encode()))
                restored = directory / f'schema{version}'
                backup.restore(archive, restored)
                for _ in range(2):
                    subprocess.run([binary, '--backup-load', str(restored / 'game.db'), str(root / 'data')], check=True)
                    migrated = backup.snapshot(restored / 'game.db', backup.RELOADED_GAME)
                    self.assertEqual(migrated['account_fame'], old['game']['account_fame'])
                    for table in ('bot_minds', 'trade_transactions'):
                        self.assertEqual(migrated[table], old['game'][table] if version == 16 else [])
                    self.assertEqual(migrated['guilds'], old['game']['guilds'])
                    self.assertEqual(migrated['guild_members'], old['game']['guild_members'])
                    self.assertEqual(migrated['picture_sequence'], old['game']['picture_sequence'])
                    self.assertEqual(migrated['legacy_claims'], [])
                    self.assertEqual(migrated['level_credits'], [])
                    for row, prior in zip(migrated['deaths'], old['game']['deaths']):
                        self.assertEqual({k: v for k, v in row.items() if k != 'receipt'}, prior)
                        receipt = backup.paid_receipt(row['receipt'])
                        self.assertEqual(receipt['policy'], 'legacy_paid')
                        self.assertEqual(receipt['total'], json.loads(prior['snapshot'])['fame'] + prior['equipment_fame'])
                        self.assertFalse(any(r['kind'] == 'explorer' for r in receipt['rows']))
                    for row in migrated['characters']:
                        lifetime = json.loads(row['stats'])['lifetime']
                        self.assertEqual((lifetime['completeness'], lifetime['tiles'], lifetime['projectile_hits'], lifetime['qualified_quests']), ('known_only', 4_000_001, 0, 0))
            for version in [0, 2, 255]:
                bad = deepcopy(old)
                payload = json.loads(bad['game']['characters'][0]['stats'])
                payload['lifetime'] = dict(json.loads(migrated['characters'][0]['stats'])['lifetime'], version=version)
                bad['game']['characters'][0]['stats'] = json.dumps(payload)
                invalid = directory / f'bad-legacy-{version}.gz'
                invalid.write_bytes(gzip.compress(json.dumps(bad).encode()))
                target = directory / f'bad-legacy-{version}'
                with self.assertRaises(ValueError):
                    backup.restore(invalid, target)
                self.assertFalse(target.exists())

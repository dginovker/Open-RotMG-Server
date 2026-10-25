from contextlib import closing
from copy import deepcopy
import gzip
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import backup
import unittest
from release_contract import check


def legacy_copy(bundle):
    old = deepcopy(bundle)
    old['schema'] = 15
    del old['game']['legacy_claims'], old['game']['level_credits'], old['game']['bot_minds'], old['game']['trade_transactions']
    # This copied fixture represents the old base+equipment writer, preserving its recorded spends.
    old_totals = {}
    characters = {row['id']: row for row in old['game']['characters']}
    for row in old['game']['deaths']:
        account = characters[row['character']]['account']
        old_totals[account] = old_totals.get(account, 0) + json.loads(row['snapshot'])['fame'] + row['equipment_fame']
        del row['receipt']
    for wallet in old['game']['account_fame']:
        spent = wallet['earned'] - wallet['balance']
        wallet['earned'] = old_totals.get(wallet['account'], 0)
        wallet['balance'] = wallet['earned'] - spent
    for table, field in [('characters', 'stats'), ('deaths', 'snapshot')]:
        for row in old['game'][table]:
            payload = json.loads(row[field])
            del payload['lifetime']
            row[field] = json.dumps(payload)
    return old


class ReleaseContract(unittest.TestCase):
    def check_fame_migration(self, migrated, original):
        for table in ['bot_minds', 'trade_transactions']:
            self.assertEqual(migrated[table], original.get(table, []))
            if table not in original:
                del migrated[table]
        self.assertEqual(migrated.pop('legacy_claims'), [])
        self.assertEqual(migrated.pop('level_credits'), [])
        for row, old in zip(migrated['deaths'], original['deaths']):
            base = json.loads(old['snapshot'])['fame']
            equipment = old.get('equipment_fame', 0)
            self.assertEqual(json.loads(row.pop('receipt')), {'version': 1, 'policy': 'legacy_paid',
                'base': base, 'rows': [{'kind': 'well_equipped', 'amount': equipment}] if equipment else [],
                'total': base + equipment})
        for row, old in zip(migrated['characters'], original['characters']):
            payload = json.loads(row['stats'])
            lifetime = payload.pop('lifetime')
            self.assertEqual(lifetime, {'version': 1, 'completeness': 'known_only', 'shots': 0,
                'projectile_hits': 0, 'kills': 0, 'god_kills': 0, 'cube_kills': 0, 'oryx_kills': 0,
                'inventory_potion_used': False, 'ability_used': False, 'teleported': False,
                'tiles': json.loads(old['stats']).get('tiles', 0), 'level_assists': 0,
                'qualified_quests': 0, 'realm_exits': []})
            self.assertEqual(payload, json.loads(old['stats']))
            row['stats'] = old['stats']

    def test_rank_gold_schema18_roundtrip_and_schema17_compatibility(self):
        root = Path(__file__).resolve().parents[2]
        binary = os.environ['OPENROTMG_BACKUP_BINARY']
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            subprocess.run([binary, '--backup-fixture', str(work / 'written'), str(root / 'data')], check=True)
            game = work / 'written/game.db'
            discord = work / 'levels.sqlite'
            with closing(sqlite3.connect(discord)) as db, db:
                db.execute('CREATE TABLE levels(guild_id TEXT,user_id TEXT,username TEXT,xp INTEGER,last_xp_at INTEGER,last_message_id TEXT,PRIMARY KEY(guild_id,user_id))')
            revision = work / 'REVISION'
            revision.write_text('a' * 40)
            with closing(sqlite3.connect(game)) as db, db:
                owner = db.execute('SELECT min(id) FROM accounts').fetchone()[0]
                db.execute('UPDATE accounts SET credits=credits+3725 WHERE id=?', (owner,))
                for index, reward in enumerate(backup.gold_reward_types(18)[3:], 1):
                    db.execute('INSERT INTO gold_rewards VALUES(?,?,?,?)', (owner, reward, index, index if index % 2 else None))
            bundle = backup.export(game, discord, work / 'new.gz', revision, revision, revision)
            self.assertEqual(bundle['schema'], 18)
            restored = work / 'new'
            backup.restore(work / 'new.gz', restored)
            subprocess.run([binary, '--backup-load', str(restored / 'game.db'), str(root / 'data')], check=True)
            self.assertEqual(backup.snapshot(restored / 'game.db', backup.RELOADED_GAME), bundle['game'])
            old = deepcopy(bundle)
            old['schema'] = 17
            with self.assertRaisesRegex(ValueError, 'Invalid gold reward'):
                backup.validate(old)
            old['game']['gold_rewards'] = [r for r in old['game']['gold_rewards'] if not r['reward'].startswith('class_star_')]
            next(r for r in old['game']['accounts'] if r['id'] == owner)['credits'] -= 3725
            archive = work / 'old.gz'
            archive.write_bytes(gzip.compress(json.dumps(old).encode()))
            backup.restore(archive, work / 'old')
            old_game = work / 'old/game.db'
            exported = backup.export(old_game, work / 'old/levels.sqlite', work / 'old-export.gz', revision, revision, revision)
            self.assertEqual(exported['schema'], 17)
            self.assertEqual(exported['game'], old['game'])
            subprocess.run([binary, '--backup-load', str(old_game), str(root / 'data')], check=True)
            migrated = backup.export(old_game, work / 'old/levels.sqlite', work / 'migrated.gz', revision, revision, revision)
            self.assertEqual(migrated['schema'], 18)
            self.assertEqual(migrated['game'], old['game'])
            with closing(sqlite3.connect(old_game)) as db, db:
                db.executescript('ALTER TABLE gold_rewards RENAME TO preserved_gold; ' + backup.gold_reward_sql(17) + '; INSERT INTO gold_rewards SELECT * FROM preserved_gold; DROP TABLE preserved_gold;')
            before = old_game.read_bytes()
            with self.assertRaisesRegex(ValueError, 'gold reward metadata mismatch'):
                backup.export(old_game, work / 'old/levels.sqlite', work / 'rejected.gz', revision, revision, revision)
            self.assertEqual(old_game.read_bytes(), before)
            self.assertFalse((work / 'rejected.gz').exists())

    def test_actual_writer_backup_restore_and_reload(self):
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as temporary:
            check(os.environ['OPENROTMG_BACKUP_BINARY'], root / 'data',
                  Path(temporary) / 'contract', 'a' * 40)
            with closing(sqlite3.connect(Path(temporary) / 'contract/written/game.db')) as db:
                value = json.loads(db.execute('SELECT stats FROM characters WHERE alive=1').fetchone()[0])
            self.assertEqual(value['dungeons'], [1, 2, 3])
            self.assertEqual(value['abysses'], 5)
            self.assertEqual(value['tombs'], 9)
            bundle = json.loads(gzip.decompress((Path(temporary) / 'contract/state.json.gz').read_bytes()))
            exported = next(row for row in bundle['game']['characters'] if row['id'] == value['id'])
            self.assertEqual(json.loads(exported['stats'])['tombs'], 9)
            self.assertEqual(json.loads(next(row for row in bundle['game']['deaths']
                if row['killed_by'] == 'Backup contract fixture')['snapshot'])['tombs'], 9)
            with closing(sqlite3.connect(Path(temporary) / 'contract/restored/game.db')) as db:
                restored = json.loads(db.execute('SELECT stats FROM characters WHERE id=?', (value['id'],)).fetchone()[0])
                self.assertEqual(restored['tombs'], 9)
                self.assertEqual(db.execute('SELECT name,level,fame,earned,board FROM guilds').fetchone(),
                                 ('Backup Guild', 2, 3270, 13270, 'Welcome & prepare!\n日本語 🧙'))
                self.assertEqual(db.execute('SELECT rank,fame FROM guild_members').fetchone(), (40, 13270))
                rewards = db.execute('SELECT reward,notified_at FROM gold_rewards ORDER BY account,reward').fetchall()
                self.assertEqual([row[0] for row in rewards], ['discord_link', 'first_bug_report', 'discord_link'])
                self.assertIsNone(rewards[0][1])
                self.assertIsNone(rewards[1][1])
                self.assertGreater(rewards[2][1], 0)
                self.assertEqual(db.execute('PRAGMA user_version').fetchone()[0], 18)
                for table, column, default in [('characters', 'revision', '0'), ('characters', 'alive', '1'), ('deaths', 'died_at', 'unixepoch()')]:
                    with self.subTest(default=table + '.' + column):
                        self.assertEqual(db.execute('SELECT dflt_value FROM pragma_table_xinfo(?) WHERE name=?', (table, column)).fetchone(), (default,))
                buyer = db.execute('SELECT account FROM character_slot_purchases').fetchone()[0]
                self.assertEqual(db.execute('SELECT character_slots,credits FROM accounts WHERE id=?', (buyer,)).fetchone(), (2, 0))
                self.assertEqual(db.execute('SELECT count(*) FROM characters WHERE account=? AND alive=1', (buyer,)).fetchone()[0], 2)
                self.assertEqual(db.execute('SELECT timer,slots FROM character_slot_purchases WHERE account=?', (buyer,)).fetchone(), (-2147483648, 2))
                self.assertFalse(db.execute("SELECT 1 FROM sqlite_schema WHERE name='one_living_character'").fetchall())
                db.execute('PRAGMA foreign_keys=ON')
                owner = db.execute('SELECT min(id) FROM accounts').fetchone()[0]
                for row in [(owner, 'discord_link', 123, None), (owner, 'invalid', 123, None), (owner, 'collection_top_half', 0, None), (owner, 'collection_top_half', 123, 0), (99999, 'discord_link', 123, None)]:
                    with self.subTest(row=row), self.assertRaises(sqlite3.IntegrityError):
                        db.execute('INSERT INTO gold_rewards VALUES(?,?,?,?)', row)
            self.assertEqual(bundle['schema'], 18)
            canonical16 = legacy_copy(bundle)
            canonical16['schema'] = 16
            for table in ['bot_minds', 'trade_transactions']:
                canonical16['game'][table] = deepcopy(bundle['game'][table])
            archive16 = Path(temporary) / 'canonical16.gz'
            archive16.write_bytes(gzip.compress(json.dumps(canonical16).encode()))
            restored16 = Path(temporary) / 'canonical16'
            backup.restore(archive16, restored16)
            subprocess.run([os.environ['OPENROTMG_BACKUP_BINARY'], '--backup-load', str(restored16 / 'game.db'), str(root / 'data')], check=True)
            migrated16 = backup.snapshot(restored16 / 'game.db', backup.RELOADED_GAME)
            self.check_fame_migration(migrated16, canonical16['game'])
            self.assertEqual(migrated16, canonical16['game'])
            for field, invalid in [('tombs', -1), ('unknown_tombs', 9)]:
                malformed = deepcopy(bundle)
                stats = json.loads(malformed['game']['characters'][0]['stats'])
                stats[field] = invalid
                malformed['game']['characters'][0]['stats'] = json.dumps(stats)
                with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'character payload'):
                    backup.validate(malformed)
            self.assertEqual([r['equipment_fame'] for r in bundle['game']['deaths']], [0, 750])
            for award in [None, True, -1, 1.5, '750', 2147483648, 751]:
                malformed = deepcopy(bundle)
                malformed['game']['deaths'][1]['equipment_fame'] = award
                with self.subTest(award=award), self.assertRaisesRegex(ValueError, 'death|Death'):
                    backup.validate(malformed)
            for version in [13, 14]:
                malformed = legacy_copy(bundle)
                malformed['schema'] = version
                del malformed['game']['pictures'], malformed['game']['picture_sequence']
                del malformed['game']['deaths'][1]['equipment_fame']
                with self.subTest(missing_award_version=version), self.assertRaisesRegex(ValueError, 'fields'):
                    backup.validate(malformed)
            disguised = legacy_copy(bundle)
            disguised['schema'] = 13
            del disguised['game']['pictures'], disguised['game']['picture_sequence']
            for row in disguised['game']['deaths']:
                del row['equipment_fame']
            with self.assertRaisesRegex(ValueError, 'earned fame'):
                backup.validate(disguised)
            malformed = deepcopy(bundle)
            malformed['game']['guilds'][0]['earned'] = sum(row['earned'] for row in bundle['game']['account_fame']) + 1
            with self.assertRaisesRegex(ValueError, 'Guild'):
                backup.validate(malformed)
            malformed = deepcopy(bundle)
            malformed['game']['deaths'][1]['snapshot'] = json.dumps(dict(json.loads(malformed['game']['deaths'][1]['snapshot']), account=99999))
            bad_archive = Path(temporary) / 'foreign-death.gz'
            bad_archive.write_bytes(gzip.compress(json.dumps(malformed).encode()))
            with self.assertRaisesRegex(ValueError, 'ownership'):
                backup.restore(bad_archive, Path(temporary) / 'foreign-death')
            self.assertFalse((Path(temporary) / 'foreign-death').exists())
            for field, values in [('vault_mask', [True, 0, 2, 1 << 20, '3'])]:
                for invalid in values:
                    malformed = deepcopy(bundle)
                    malformed['game']['accounts'][0][field] = invalid
                    with self.subTest(field=field, invalid=invalid), self.assertRaises(ValueError):
                        backup.validate(malformed)
            for field, values in [('account', [True, 0, 99999]), ('chest', [True, -1, 20, '1']), ('revision', [True, -1, 2147483648]), ('items', ['[]', '[2594,2595,-1,-1,-1,-1,-1,65536]'])]:
                for invalid in values:
                    malformed = deepcopy(bundle)
                    malformed['game']['vaults'][0][field] = invalid
                    with self.subTest(field=field, invalid=invalid), self.assertRaises(ValueError):
                        backup.validate(malformed)
            for rows in [[], bundle['game']['vaults'][:-1], bundle['game']['vaults'] * 2]:
                malformed = deepcopy(bundle)
                malformed['game']['vaults'] = rows
                with self.assertRaises(ValueError):
                    backup.validate(malformed)
            malformed = legacy_copy(bundle)
            malformed['schema'] = 12
            del malformed['game']['pictures'], malformed['game']['picture_sequence']
            with self.assertRaisesRegex(ValueError, 'fields'):
                backup.validate(malformed)
            for field, values in [('character_slots', [True, 0, 15, '2'])]:
                for invalid in values:
                    malformed = deepcopy(bundle)
                    next(r for r in malformed['game']['accounts'] if r['id'] == buyer)[field] = invalid
                    with self.subTest(field=field, invalid=invalid), self.assertRaises(ValueError):
                        backup.validate(malformed)
            for field, values in [('account', [True, 0, 99999]), ('timer', [True, -2147483649, 2147483648, '1']), ('slots', [True, 1, 3, 15])]:
                for invalid in values:
                    malformed = deepcopy(bundle)
                    malformed['game']['character_slot_purchases'][0][field] = invalid
                    with self.subTest(field=field, invalid=invalid), self.assertRaises(ValueError):
                        backup.validate(malformed)
            for receipts in [[], bundle['game']['character_slot_purchases'] * 2]:
                malformed = deepcopy(bundle)
                malformed['game']['character_slot_purchases'] = receipts
                with self.assertRaises(ValueError):
                    backup.validate(malformed)
            malformed = deepcopy(bundle)
            live = next(r for r in malformed['game']['characters'] if r['alive'])
            malformed['game']['characters'].append({**live, 'id': 99999})
            with self.assertRaisesRegex(ValueError, 'character slot'):
                backup.validate(malformed)
            revision = Path(temporary) / 'contract/REVISION'
            for original, changed in [
                ('REFERENCES accounts(id)', ''),
                ('PRIMARY KEY(account,chest)', 'PRIMARY KEY(chest,account)'),
                ('items TEXT NOT NULL', 'items BLOB NOT NULL'),
                ('items TEXT NOT NULL', 'items TEXT'),
                ('DEFAULT 0', ''),
            ]:
                malformed_db = Path(temporary) / 'malformed-metadata.db'
                with closing(sqlite3.connect(Path(temporary) / 'contract/restored/game.db')) as source, closing(sqlite3.connect(malformed_db)) as target:
                    source.backup(target)
                    sql = target.execute("SELECT sql FROM sqlite_schema WHERE name='vaults'").fetchone()[0]
                    changed_sql = sql.replace(original, changed)
                    self.assertNotEqual(sql, changed_sql)
                    target.executescript('PRAGMA foreign_keys=OFF; ALTER TABLE vaults RENAME TO preserved_vaults; ' + changed_sql + '; INSERT INTO vaults SELECT * FROM preserved_vaults; DROP TABLE preserved_vaults;')
                before = malformed_db.read_bytes()
                with self.subTest(metadata=original), self.assertRaisesRegex(ValueError, 'metadata|foreign key'):
                    backup.export(malformed_db, Path(temporary) / 'contract/levels.sqlite', Path(temporary) / 'malformed.gz', revision, revision, revision)
                self.assertEqual(malformed_db.read_bytes(), before)
                self.assertFalse((Path(temporary) / 'malformed.gz').exists())
            for definition in ['equipment_fame INTEGER', 'equipment_fame INTEGER NOT NULL DEFAULT 0', 'equipment_fame TEXT NOT NULL DEFAULT \'0\'']:
                malformed_db = Path(temporary) / 'malformed-death-metadata.db'
                with closing(sqlite3.connect(Path(temporary) / 'contract/restored/game.db')) as source, closing(sqlite3.connect(malformed_db)) as target:
                    source.backup(target)
                    target.executescript('ALTER TABLE deaths DROP COLUMN equipment_fame; ALTER TABLE deaths ADD COLUMN ' + definition)
                original = malformed_db.read_bytes()
                with self.subTest(death_metadata=definition), self.assertRaisesRegex(ValueError, 'metadata'):
                    backup.export(malformed_db, Path(temporary) / 'contract/levels.sqlite', Path(temporary) / 'malformed-death.gz', revision, revision, revision)
                self.assertEqual(malformed_db.read_bytes(), original)
                self.assertFalse((Path(temporary) / 'malformed-death.gz').exists())
            for version in [0, 11, 12, 13, 14, 15, 16, 19]:
                with closing(sqlite3.connect(Path(temporary) / 'contract/restored/game.db')) as source, closing(sqlite3.connect(Path(temporary) / 'future.db')) as future:
                    source.backup(future)
                    future.execute(f'PRAGMA user_version={version}')
                with self.subTest(version=version), self.assertRaisesRegex(ValueError, 'schema version'):
                    backup.export(Path(temporary) / 'future.db', Path(temporary) / 'contract/levels.sqlite', Path(temporary) / 'future.gz', revision, revision, revision)
                self.assertFalse((Path(temporary) / 'future.gz').exists())
            legacy = legacy_copy(bundle)
            del legacy['game']['pictures'], legacy['game']['picture_sequence']
            paid = sum(row.pop('equipment_fame') for row in legacy['game']['deaths'])
            self.assertEqual(paid, 750)
            owner = legacy['game']['guild_members'][0]['account']
            wallet = next(row for row in legacy['game']['account_fame'] if row['account'] == owner)
            wallet['balance'] -= paid
            wallet['earned'] -= paid
            legacy['game']['guild_members'][0]['fame'] -= paid
            legacy['game']['guilds'][0]['fame'] -= paid
            legacy['game']['guilds'][0]['earned'] -= paid
            legacy['schema'] = 13
            archive13 = Path(temporary) / 'legacy13.gz'
            archive13.write_bytes(gzip.compress(json.dumps(legacy).encode()))
            restored13 = Path(temporary) / 'legacy13'
            backup.restore(archive13, restored13)
            subprocess.run([os.environ['OPENROTMG_BACKUP_BINARY'], '--backup-load', str(restored13 / 'game.db'), str(root / 'data')], check=True)
            migrated13 = backup.snapshot(restored13 / 'game.db', backup.RELOADED_GAME)
            self.check_fame_migration(migrated13, legacy['game'])
            self.assertEqual(migrated13.pop('pictures'), [])
            self.assertEqual(migrated13.pop('picture_sequence'), [{'id': 1, 'next_id': 1}])
            for row in migrated13['deaths']:
                self.assertEqual(row.pop('equipment_fame'), 0)
            self.assertEqual(migrated13, legacy['game'])
            for row in legacy['game']['accounts']:
                del row['vault_mask']
            # Build an explicit old one-chest fixture without discarding either prepared item.
            initial = {r['account']: r for r in legacy['game']['vaults'] if r['chest'] == 0}
            for row in legacy['game']['vaults']:
                if row['chest'] != 0:
                    target = initial[row['account']]
                    items = json.loads(target['items'])
                    for item in json.loads(row['items']):
                        if item != -1:
                            items[items.index(-1)] = item
                    target['items'] = json.dumps(items)
            legacy['game']['vaults'] = list(initial.values())
            for row in legacy['game']['vaults']:
                del row['chest']
            legacy['schema'] = 12
            archive12 = Path(temporary) / 'legacy12.gz'
            archive12.write_bytes(gzip.compress(json.dumps(legacy).encode()))
            restored12 = Path(temporary) / 'legacy12'
            backup.restore(archive12, restored12)
            subprocess.run([os.environ['OPENROTMG_BACKUP_BINARY'], '--backup-load', str(restored12 / 'game.db'), str(root / 'data')], check=True)
            migrated12 = backup.snapshot(restored12 / 'game.db', backup.RELOADED_GAME)
            self.check_fame_migration(migrated12, legacy['game'])
            self.assertEqual(migrated12.pop('pictures'), [])
            self.assertEqual(migrated12.pop('picture_sequence'), [{'id': 1, 'next_id': 1}])
            for row in migrated12['deaths']:
                self.assertEqual(row.pop('equipment_fame'), 0)
            for row in migrated12['accounts']:
                self.assertEqual(row.pop('vault_mask'), 1)
            for row in migrated12['vaults']:
                self.assertEqual(row.pop('chest'), 0)
            self.assertEqual(migrated12, legacy['game'])
            legacy['schema'] = 10
            del legacy['game']['character_slot_purchases']
            for row in legacy['game']['accounts']:
                del row['character_slots']
            # Version10 cannot represent the new fixture's purchased second living slot.
            extra = max(r['id'] for r in legacy['game']['characters'] if r['alive'])
            legacy['game']['characters'] = [r for r in legacy['game']['characters'] if r['id'] != extra]
            with self.assertRaisesRegex(ValueError, 'gold reward'):
                backup.validate(legacy)
            legacy11 = dict(deepcopy(legacy), schema=11)
            archive11 = Path(temporary) / 'legacy11.gz'
            archive11.write_bytes(gzip.compress(json.dumps(legacy11).encode()))
            restored11 = Path(temporary) / 'legacy11'
            backup.restore(archive11, restored11)
            reexported11 = backup.export(restored11 / 'game.db', restored11 / 'levels.sqlite', Path(temporary) / 'reexport11.gz', revision, revision, revision)
            self.assertEqual(reexported11['schema'], 11)
            self.assertEqual(reexported11['game'], legacy11['game'])
            subprocess.run([os.environ['OPENROTMG_BACKUP_BINARY'], '--backup-load', str(restored11 / 'game.db'), str(root / 'data')], check=True)
            migrated = backup.snapshot(restored11 / 'game.db', backup.RELOADED_GAME)
            self.check_fame_migration(migrated, legacy11['game'])
            self.assertEqual(migrated.pop('pictures'), [])
            self.assertEqual(migrated.pop('picture_sequence'), [{'id': 1, 'next_id': 1}])
            for row in migrated['deaths']:
                self.assertEqual(row.pop('equipment_fame'), 0)
            self.assertEqual(migrated.pop('character_slot_purchases'), [])
            for row in migrated['accounts']:
                self.assertEqual(row.pop('character_slots'), 1)
                self.assertEqual(row.pop('vault_mask'), 1)
            for row in migrated['vaults']:
                self.assertEqual(row.pop('chest'), 0)
            self.assertEqual(migrated, legacy11['game'])
            legacy['game']['gold_rewards'] = [r for r in legacy['game']['gold_rewards'] if r['reward'] != 'first_bug_report']
            archive = Path(temporary) / 'legacy.gz'
            archive.write_bytes(gzip.compress(json.dumps(legacy).encode()))
            restored = Path(temporary) / 'legacy'
            backup.restore(archive, restored)
            revision = Path(temporary) / 'REVISION'
            revision.write_text('a' * 40)
            reexported = backup.export(restored / 'game.db', restored / 'levels.sqlite', Path(temporary) / 'reexport.gz', revision, revision, revision)
            self.assertEqual(reexported['schema'], 10)
            self.assertEqual(reexported['game'], legacy['game'])
            for field, values in [('account', [True, 0, 99999]), ('reward', ['invalid', 100]), ('awarded_at', [True, 0, -1]), ('notified_at', [True, 0, -1])]:
                for invalid in values:
                    malformed = deepcopy(bundle)
                    malformed['game']['gold_rewards'][0][field] = invalid
                    with self.subTest(field=field, invalid=invalid), self.assertRaises(ValueError):
                        backup.validate(malformed)
            malformed = deepcopy(bundle)
            malformed['game']['gold_rewards'].append(malformed['game']['gold_rewards'][0])
            with self.assertRaisesRegex(ValueError, 'Duplicate gold reward'):
                backup.validate(malformed)
            backup.characters([value])
            for field in ['password_hash', 'token', 'email']:
                invalid = {**value, field: 'NEVER_LOG_OR_EXPORT'}
                with self.assertRaises(ValueError) as error:
                    backup.characters([invalid])
                self.assertNotIn('NEVER_LOG_OR_EXPORT', str(error.exception))
            for counters in [[1, 2], [1, -1, 3], [1, True, 3]]:
                invalid = deepcopy(value)
                invalid['dungeons'] = counters
                with self.assertRaises(ValueError):
                    backup.characters([invalid])

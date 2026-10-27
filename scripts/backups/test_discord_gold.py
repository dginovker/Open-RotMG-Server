from contextlib import closing
from copy import deepcopy
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import unittest
import backup


class DiscordGold(unittest.TestCase):
    def test_grant_receipts_and_wallet_survive_export_restore_and_native_reload(self):
        root = Path(__file__).resolve().parents[2]
        binary = os.environ['OPENROTMG_BACKUP_BINARY']
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            subprocess.run([binary, '--backup-fixture', str(work / 'written'), str(root / 'data')], check=True)
            game = work / 'written/game.db'
            discord = work / 'levels.sqlite'
            with closing(sqlite3.connect(discord)) as db, db:
                db.execute('CREATE TABLE levels(guild_id TEXT,user_id TEXT,username TEXT,xp INTEGER,last_xp_at INTEGER,last_message_id TEXT,PRIMARY KEY(guild_id,user_id))')
            with closing(sqlite3.connect(game)) as db, db:
                owner = db.execute('SELECT min(id) FROM accounts').fetchone()[0]
                db.execute('UPDATE accounts SET credits=credits+123 WHERE id=?', (owner,))
                db.execute('INSERT INTO discord_gold_grants VALUES(?,?,?,?,123)', ('1558662335076376580', owner, '1558662335076376578', '1558662335076376579'))
            revision = work / 'REVISION'
            revision.write_text('a' * 40)
            bundle = backup.export(game, discord, work / 'state.gz', revision, revision, revision)
            self.assertEqual(bundle['schema'], 19)
            restored = work / 'restored'
            backup.restore(work / 'state.gz', restored)
            subprocess.run([binary, '--backup-load', str(restored / 'game.db'), str(root / 'data')], check=True)
            self.assertEqual(backup.snapshot(restored / 'game.db', backup.RELOADED_GAME), bundle['game'])
            for field, value in [('amount', 0), ('amount', True), ('amount', 2147483648), ('account', 99999), ('account', True), ('request_id', 'bad'), ('discord_id', None), ('actor_id', 1)]:
                invalid = deepcopy(bundle)
                invalid['game']['discord_gold_grants'][0][field] = value
                with self.subTest(field=field, value=value), self.assertRaisesRegex(ValueError, 'Discord gold'):
                    backup.validate(invalid)
            invalid = deepcopy(bundle)
            invalid['game']['discord_gold_grants'] *= 2
            with self.assertRaisesRegex(ValueError, 'Duplicate Discord gold'):
                backup.validate(invalid)

from contextlib import closing
from copy import deepcopy
import gzip
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import backup


def check(binary, data, directory, revision):
    binary, data, directory = Path(binary).resolve(), Path(data).resolve(), Path(directory)
    directory.mkdir(mode=0o700)
    os.environ['OPENROTMG_BACKUP_BINARY'] = str(binary)
    written = directory / 'written'
    subprocess.run([str(binary), '--backup-fixture', str(written), str(data)], check=True)
    discord = directory / 'levels.sqlite'
    with closing(sqlite3.connect(discord)) as db, db:
        db.execute('CREATE TABLE levels(guild_id TEXT,user_id TEXT,username TEXT,xp INTEGER,last_xp_at INTEGER,last_message_id TEXT,PRIMARY KEY(guild_id,user_id))')
    version = directory / 'REVISION'
    version.write_text(revision + '\n')
    archive = directory / 'state.json.gz'
    bundle = backup.export(written / 'game.db', discord, archive, version, version, version)
    if len(bundle['game']['bot_minds']) != 2 or len(bundle['game']['trade_transactions']) != 1:
        raise RuntimeError('Release community payload missing from backup')
    if bundle['schema'] != 19:
        raise RuntimeError('Release backup is not schema19')
    if b'BACKUP_FIXTURE_SECRET' in gzip.decompress(archive.read_bytes()):
        raise RuntimeError('Release backup exported a login identity')
    restored = directory / 'restored'
    backup.restore(archive, restored)
    subprocess.run([str(binary), '--backup-load', str(restored / 'game.db'), str(data)], check=True)
    if backup.snapshot(restored / 'game.db', backup.RELOADED_GAME) != bundle['game']:
        raise RuntimeError('Release loading changed restored gameplay records')
    result = subprocess.run([str(binary), '--backup-exercise-vault', str(restored / 'game.db'), str(data)], text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError('Release restored vault exercise failed: ' + result.stderr.strip())
    if json.loads(result.stdout) != {'exercised_vault_chests': 2, 'transfers': 1, 'withdrawals': 1, 'deposits': 1, 'items_conserved': True}:
        raise RuntimeError('Release restored vault exercise returned an invalid receipt')
    subprocess.run([str(binary), '--backup-load', str(restored / 'game.db'), str(data)], check=True)
    before_death = backup.snapshot(restored / 'game.db', backup.RELOADED_GAME)
    result = subprocess.run([str(binary), '--backup-exercise-death', str(restored / 'game.db'), str(data)], text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError('Release restored death exercise failed: ' + result.stderr.strip())
    receipt = json.loads(result.stdout)
    character = receipt.pop('exercised_death', None)
    paid_receipt = receipt.pop('receipt', None)
    level_credits = receipt.pop('level_credits', None)
    if paid_receipt != {'policy': 'historical', 'version': 1, 'base': 19, 'rows': [{'kind': 'well_equipped', 'amount': 1}], 'total': 20}:
        raise RuntimeError('Release immutable paid receipt mismatch')
    givers = [row['id'] for row in before_death['characters'] if row['alive'] == 1 and row['account'] != next(r['account'] for r in before_death['guild_members'] if r['rank'] == 40)][:2]
    expected_credits = [{'recipient': character, 'reached_level': level, 'giver': giver} for level, giver in zip((2, 3), givers)]
    if len(expected_credits) != 2 or level_credits != expected_credits:
        raise RuntimeError('Release bounded causal credit receipt mismatch')
    if type(character) is not int or receipt != {'base_fame': 19, 'equipment_fame': 1, 'total_fame': 20}:
        raise RuntimeError('Release restored death exercise returned an invalid receipt')
    subprocess.run([str(binary), '--backup-load', str(restored / 'game.db'), str(data)], check=True)
    paid = backup.snapshot(restored / 'game.db', backup.RELOADED_GAME)
    backup.validate(dict(bundle, game=paid))
    expected = deepcopy(before_death)
    created = next(row for row in paid['characters'] if row['id'] == character)
    expected['characters'].append(created)
    expected['deaths'].append(next(row for row in paid['deaths'] if row['character'] == character))
    expected['level_credits'].extend(expected_credits)
    account = created['account']
    member = next(row for row in expected['guild_members'] if row['account'] == account)
    wallet = next(row for row in expected['account_fame'] if row['account'] == account)
    guild = next(row for row in expected['guilds'] if row['id'] == member['guild'])
    for row, fields in [(wallet, ['balance', 'earned']), (guild, ['fame', 'earned']), (member, ['fame'])]:
        for field in fields:
            row[field] += 20
    if paid != expected:
        raise RuntimeError('Release restored death changed unrelated records or historical payouts')
    before_art = backup.snapshot(restored / 'game.db', backup.RELOADED_GAME)
    next_id = before_art['picture_sequence'][0]['next_id']
    if next_id <= max(row['id'] for row in before_art['pictures']) + 1:
        raise RuntimeError('Art fixture did not preserve a deleted high-water ID')
    result = subprocess.run([str(binary), '--backup-exercise-pictures', str(restored / 'game.db'), str(data)], text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError('Release restored art exercise failed: ' + result.stderr.strip())
    if json.loads(result.stdout) != {'exercised_pictures': True, 'deleted_id': next_id, 'next_id': next_id + 1}:
        raise RuntimeError('Release restored art exercise returned an invalid receipt')
    after_art = backup.snapshot(restored / 'game.db', backup.RELOADED_GAME)
    expected = deepcopy(before_art)
    expected['picture_sequence'][0]['next_id'] += 1
    if after_art != expected:
        raise RuntimeError('Restored art exercise changed existing artwork or unrelated rows')
    backup.validate(dict(bundle, game=after_art))
    print(json.dumps({'event': 'release_backup_contract_passed', 'revision': revision}))


if __name__ == '__main__':
    check(*sys.argv[1:])

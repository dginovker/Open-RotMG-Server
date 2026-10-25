#!/usr/bin/env python3
import argparse
import base64
from contextlib import closing
import datetime
import gzip
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess

LEGACY_GAME = {
    'accounts': ('id', 'guid', 'name'),
    'characters': ('id', 'account', 'stats', 'items', 'revision', 'alive'),
    'deaths': ('character', 'snapshot', 'killed_by', 'died_at'),
}
GAME = {**LEGACY_GAME, 'vaults': ('account', 'items', 'revision')}
SHORE_GAME = {**GAME, 'shore_bots': ('name', 'account')}
HISCORES_GAME = {**SHORE_GAME, 'competitors': ('account',), 'collections': ('account', 'item', 'found_at'), 'discord_links': ('account', 'discord_id')}
ACTIONS_GAME = {**HISCORES_GAME, 'account_fame': ('account', 'balance', 'earned'), 'discord_recovery': ('account', 'enabled_at', 'last_issued_at')}
SOCIAL_GAME = {**ACTIONS_GAME, 'account_lists': ('account', 'list_id', 'target')}
UPDATES_GAME = {**SOCIAL_GAME, 'update_feed': ('id', 'snapshot', 'synced_at')}
GOLD_GAME = {**UPDATES_GAME, 'accounts': (*GAME['accounts'], 'credits')}
GUILDS_GAME = {**GOLD_GAME, 'guilds': ('id', 'name', 'level', 'fame', 'earned', 'board'), 'guild_members': ('account', 'guild', 'rank', 'fame')}
REWARDS_GAME = {**GUILDS_GAME, 'gold_rewards': ('account', 'reward', 'awarded_at', 'notified_at')}
SLOTS_GAME = {**REWARDS_GAME, 'accounts': (*GOLD_GAME['accounts'], 'character_slots'), 'character_slot_purchases': ('account', 'timer', 'slots')}
VAULTS_GAME = {**SLOTS_GAME, 'accounts': (*SLOTS_GAME['accounts'], 'vault_mask'), 'vaults': ('account', 'chest', 'items', 'revision')}
FAME_GAME = {**VAULTS_GAME, 'deaths': (*LEGACY_GAME['deaths'], 'equipment_fame')}
PICTURES_GAME = {**FAME_GAME, 'pictures': ('id', 'account', 'name', 'data_type', 'tags', 'png'), 'picture_sequence': ('id', 'next_id')}
PICTURES_SQL = "CREATE TABLE pictures(id INTEGER PRIMARY KEY CHECK(id BETWEEN 1 AND 2147483647),account INTEGER NOT NULL REFERENCES accounts(id),name TEXT NOT NULL,data_type INTEGER NOT NULL CHECK(data_type BETWEEN 1 AND 7),tags TEXT NOT NULL,png BLOB NOT NULL CHECK(typeof(png)='blob' AND length(png)<=16384),UNIQUE(account,name))"
PICTURE_SEQUENCE_SQL = "CREATE TABLE picture_sequence(id INTEGER PRIMARY KEY CHECK(id=1),next_id INTEGER NOT NULL CHECK(typeof(next_id)='integer' AND next_id BETWEEN 1 AND 2147483648))"
COMMUNITY_GAME = {**PICTURES_GAME, 'trade_transactions': ('id', 'owners', 'writes', 'revisions', 'delivery', 'market_key'), 'bot_minds': ('account', 'revision', 'mind')}
COMMUNITY_SQL = {
    'trade_transactions': 'CREATE TABLE trade_transactions(id TEXT PRIMARY KEY,owners TEXT NOT NULL,writes TEXT NOT NULL,revisions TEXT NOT NULL,delivery TEXT,market_key TEXT UNIQUE)',
    'bot_minds': 'CREATE TABLE bot_minds(account INTEGER PRIMARY KEY REFERENCES accounts(id),revision INTEGER NOT NULL CHECK(revision>0),mind TEXT NOT NULL CHECK(json_valid(mind)))',
}
DEATH_FAME_GAME = {**COMMUNITY_GAME, 'deaths': (*FAME_GAME['deaths'], 'receipt'), 'legacy_claims': ('account', 'class', 'character'), 'level_credits': ('recipient', 'reached_level', 'giver')}
DEATHS_SQL = "CREATE TABLE deaths(character INTEGER PRIMARY KEY REFERENCES characters(id),snapshot TEXT NOT NULL,killed_by TEXT NOT NULL,died_at INTEGER NOT NULL DEFAULT(unixepoch()),equipment_fame INTEGER NOT NULL CHECK(typeof(equipment_fame)='integer' AND equipment_fame BETWEEN 0 AND 2147483647),receipt TEXT NOT NULL CHECK(json_valid(receipt)))"
CLAIMS_SQL = "CREATE TABLE legacy_claims(account INTEGER NOT NULL REFERENCES accounts(id),class INTEGER NOT NULL CHECK(typeof(class)='integer' AND class BETWEEN 0 AND 65535),character INTEGER NOT NULL UNIQUE REFERENCES deaths(character),PRIMARY KEY(account,class))"
CREDITS_SQL = "CREATE TABLE level_credits(recipient INTEGER NOT NULL REFERENCES characters(id),reached_level INTEGER NOT NULL CHECK(typeof(reached_level)='integer' AND reached_level BETWEEN 2 AND 20),giver INTEGER NOT NULL REFERENCES characters(id),PRIMARY KEY(recipient,reached_level),CHECK(recipient<>giver))"
CREDITS_INDEX_SQL = 'CREATE INDEX level_credits_by_giver ON level_credits(giver)'
GAME_SCHEMAS = {1: LEGACY_GAME, 2: GAME, 3: SHORE_GAME, 4: HISCORES_GAME, 5: ACTIONS_GAME, 6: SOCIAL_GAME, 7: UPDATES_GAME, 8: GOLD_GAME, 9: GUILDS_GAME, 10: REWARDS_GAME, 11: REWARDS_GAME, 12: SLOTS_GAME, 13: VAULTS_GAME, 14: FAME_GAME, 15: PICTURES_GAME, 16: COMMUNITY_GAME, 17: DEATH_FAME_GAME, 18: DEATH_FAME_GAME}
AUTH_GAME = {**GAME, 'credentials': ('account', 'email', 'password_hash')}
AUTH_SHORE_GAME = {**SHORE_GAME, 'credentials': AUTH_GAME['credentials']}
AUTH_HISCORES_GAME = {**HISCORES_GAME, 'credentials': AUTH_GAME['credentials'], 'discord_codes': ('account', 'hash', 'issued_at', 'expires_at')}
AUTH_ACTIONS_GAME = {**ACTIONS_GAME, 'credentials': AUTH_GAME['credentials'], **{t: AUTH_HISCORES_GAME['discord_codes'] for t in ['discord_codes', 'recovery_codes', 'password_resets']}}
AUTH_SOCIAL_GAME = {**AUTH_ACTIONS_GAME, 'account_lists': SOCIAL_GAME['account_lists']}
AUTH_UPDATES_GAME = {**AUTH_SOCIAL_GAME, 'update_feed': UPDATES_GAME['update_feed']}
# Loading a restore with the release binary recreates its empty credential tables; the deploy helper and release contract share this.
AUTH_GOLD_GAME = {**AUTH_UPDATES_GAME, 'accounts': GOLD_GAME['accounts']}
AUTH_GUILDS_GAME = {**AUTH_GOLD_GAME, 'guilds': GUILDS_GAME['guilds'], 'guild_members': GUILDS_GAME['guild_members']}
AUTH_REWARDS_GAME = {**AUTH_GUILDS_GAME, 'gold_rewards': REWARDS_GAME['gold_rewards']}
AUTH_SLOTS_GAME = {**AUTH_REWARDS_GAME, 'accounts': SLOTS_GAME['accounts'], 'character_slot_purchases': SLOTS_GAME['character_slot_purchases']}
AUTH_VAULTS_GAME = {**AUTH_SLOTS_GAME, 'accounts': VAULTS_GAME['accounts'], 'vaults': VAULTS_GAME['vaults']}
RELOADED_FAME_GAME = {**AUTH_VAULTS_GAME, 'deaths': FAME_GAME['deaths']}
RELOADED_PICTURES_GAME = {**RELOADED_FAME_GAME, 'pictures': PICTURES_GAME['pictures'], 'picture_sequence': PICTURES_GAME['picture_sequence']}
RELOADED_COMMUNITY_GAME = {**RELOADED_PICTURES_GAME, 'trade_transactions': COMMUNITY_GAME['trade_transactions'], 'bot_minds': COMMUNITY_GAME['bot_minds']}
RELOADED_GAME = {**RELOADED_COMMUNITY_GAME, 'deaths': DEATH_FAME_GAME['deaths'], 'legacy_claims': DEATH_FAME_GAME['legacy_claims'], 'level_credits': DEATH_FAME_GAME['level_credits']}
DISCORD = {'levels': ('guild_id', 'user_id', 'username', 'xp', 'last_xp_at', 'last_message_id')}


def characters(values, *, legacy=False):
    binary = os.environ.get('OPENROTMG_BACKUP_BINARY')
    if not binary:
        raise RuntimeError('OPENROTMG_BACKUP_BINARY must name the matching release binary')
    result = subprocess.run([binary, '--backup-validate'] + (['legacy'] if legacy else []), input=json.dumps(values),
                            text=True, capture_output=True, timeout=30)
    if result.returncode:
        raise ValueError('Release character validation failed: ' + result.stderr.strip())
    if json.loads(result.stdout) != {'validated_characters': len(values)}:
        raise ValueError('Release character validator returned an invalid receipt')


def update_snapshot(raw):
    binary = os.environ.get('OPENROTMG_BACKUP_BINARY')
    if not binary:
        raise RuntimeError('OPENROTMG_BACKUP_BINARY must name the matching release binary')
    result = subprocess.run([binary, '--backup-validate-updates'], input=raw,
                            text=True, capture_output=True, timeout=30)
    if result.returncode:
        raise ValueError('Release update validation failed: ' + result.stderr.strip())
    if json.loads(result.stdout) != {'validated_updates': True}:
        raise ValueError('Release update validator returned an invalid receipt')


def validate_pictures(game):
    accounts = {row['id'] for row in game['accounts']}
    ids, names = set(), set()
    for row in game['pictures']:
        if (type(row['id']) is not int or not 0 < row['id'] <= 2147483647 or row['id'] in ids
                or type(row['account']) is not int or row['account'] not in accounts
                or type(row['name']) is not str or (row['account'], row['name']) in names
                or type(row['data_type']) is not int or not 1 <= row['data_type'] <= 7
                or type(row['tags']) is not str or type(row['png']) is not str or len(row['png']) > 21848):
            raise ValueError('Invalid picture identity, ownership, metadata or size')
        ids.add(row['id'])
        names.add((row['account'], row['name']))
        try:
            png = base64.b64decode(row['png'], validate=True)
        except ValueError as error:
            raise ValueError('Invalid picture base64') from error
        if base64.b64encode(png).decode() != row['png']:
            raise ValueError('Noncanonical picture base64')
    sequence = game['picture_sequence']
    if (len(sequence) != 1 or type(sequence[0]['id']) is not int or sequence[0]['id'] != 1
            or type(sequence[0]['next_id']) is not int or not max(ids, default=0) < sequence[0]['next_id'] <= 2147483648):
        raise ValueError('Invalid picture sequence high-water mark')
    binary = os.environ.get('OPENROTMG_BACKUP_BINARY')
    if not binary:
        raise RuntimeError('OPENROTMG_BACKUP_BINARY must name the matching release binary')
    result = subprocess.run([binary, '--backup-validate-pictures'], input=json.dumps(game['pictures']),
                            text=True, capture_output=True, timeout=30)
    if result.returncode:
        raise ValueError('Release picture validation failed: ' + result.stderr.strip())
    if json.loads(result.stdout) != {'validated_pictures': len(ids)}:
        raise ValueError('Release picture validator returned an invalid receipt')


def snapshot(path, schema, *, include_schema=False):
    with closing(sqlite3.connect(Path(path).resolve().as_uri() + '?mode=ro', uri=True)) as db, db:
        db.execute('BEGIN')
        if db.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
            raise ValueError('Source SQLite integrity failed')
        if db.execute('PRAGMA foreign_key_check').fetchall():
            raise ValueError('Source SQLite foreign keys failed')
        tables = {r[0] for r in db.execute("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'")}
        schemas = schema if isinstance(schema, list) else [schema]
        matched = [candidate for candidate in schemas if tables == set(candidate) and all(
            tuple(r[1] for r in db.execute('PRAGMA table_xinfo(' + table + ')')) == columns
            for table, columns in candidate.items())]
        if not matched or any(candidate != matched[0] for candidate in matched):
            raise ValueError('Unsupported database tables or columns/metadata; update and test the export schema before backing up')
        schema = matched[0]
        if 'vaults' in schema:
            references = db.execute('SELECT "table","from","to",seq,on_update,on_delete FROM pragma_foreign_key_list(\'vaults\') ORDER BY id,seq').fetchall()
            if references != [('accounts', 'account', 'id', 0, 'NO ACTION', 'NO ACTION')]:
                raise ValueError('Unsupported source Vault account foreign key')
            if schema['vaults'] == VAULTS_GAME['vaults']:
                for table, column, kind, default, key in [
                    ('vaults', 'account', 'INTEGER', None, 1),
                    ('vaults', 'chest', 'INTEGER', None, 2),
                    ('vaults', 'items', 'TEXT', None, 0),
                    ('vaults', 'revision', 'INTEGER', '0', 0),
                    ('accounts', 'vault_mask', 'INTEGER', '1', 0),
                ]:
                    actual = db.execute('SELECT type,"notnull",dflt_value,pk,hidden FROM pragma_table_xinfo(?) WHERE name=?', (table, column)).fetchone()
                    if actual != (kind, 1, default, key, 0):
                        raise ValueError('Unsupported canonical Vault metadata: ' + table + '.' + column)
        if schema.get('deaths') in (FAME_GAME['deaths'], DEATH_FAME_GAME['deaths']):
            references = db.execute('SELECT "table","from","to",seq,on_update,on_delete FROM pragma_foreign_key_list(\'deaths\') ORDER BY id,seq').fetchall()
            if references != [('characters', 'character', 'id', 0, 'NO ACTION', 'NO ACTION')]:
                raise ValueError('Unsupported death character foreign key')
            for column, kind, not_null, default, key in [
                ('character', 'INTEGER', 0, None, 1),
                ('snapshot', 'TEXT', 1, None, 0),
                ('killed_by', 'TEXT', 1, None, 0),
                ('died_at', 'INTEGER', 1, 'unixepoch()', 0),
                ('equipment_fame', 'INTEGER', 1, None, 0),
            ]:
                metadata = db.execute('SELECT type,"notnull",dflt_value,pk,hidden FROM pragma_table_xinfo(\'deaths\') WHERE name=?', (column,)).fetchone()
                if metadata != (kind, not_null, default, key, 0):
                    raise ValueError('Unsupported canonical death metadata: ' + column)
        if 'legacy_claims' in schema:
            for table, expected in [('deaths', DEATHS_SQL), ('legacy_claims', CLAIMS_SQL), ('level_credits', CREDITS_SQL), ('level_credits_by_giver', CREDITS_INDEX_SQL)]:
                if db.execute('SELECT sql FROM sqlite_schema WHERE name=?', (table,)).fetchone() != (expected,):
                    raise ValueError('Unsupported canonical death fame metadata: ' + table)
        if 'pictures' in schema:
            for table, expected in [('pictures', PICTURES_SQL), ('picture_sequence', PICTURE_SEQUENCE_SQL)]:
                if db.execute("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?", (table,)).fetchone() != (expected,):
                    raise ValueError('Unsupported canonical picture metadata: ' + table)
        if 'bot_minds' in schema:
            for table, expected in COMMUNITY_SQL.items():
                if db.execute("SELECT sql FROM sqlite_schema WHERE type='table' AND name=?", (table,)).fetchone() != (expected,):
                    raise ValueError('Unsupported canonical community metadata: ' + table)
        result = {}
        for table, columns in schema.items():
            if table in {'credentials', 'discord_codes', 'recovery_codes', 'password_resets'}:
                continue
            selected = tuple(c for c in columns if c != 'guid')
            rows = db.execute('SELECT ' + ','.join(selected) + ' FROM ' + table + ' ORDER BY ' + ','.join(selected[:2])).fetchall()
            result[table] = [dict(zip(selected, row)) for row in rows]
            if table == 'pictures':
                for row in result[table]:
                    if type(row['png']) is not bytes:
                        raise ValueError('Invalid stored picture PNG type')
                    row['png'] = base64.b64encode(row['png']).decode()
        return (result, schema, db.execute('PRAGMA user_version').fetchone()[0]) if include_schema else result


def validate_guilds(game, accounts):
    guilds, names = {}, set()
    for row in game.get('guilds', []):
        if (type(row['id']) is not int or not 0 < row['id'] <= 2147483647
                or type(row['name']) is not str or not re.fullmatch('[A-Za-z ]{1,20}', row['name'])
                or row['name'] != ' '.join(row['name'].split())
                or type(row['level']) is not int or not 1 <= row['level'] <= 4
                or any(type(row[k]) is not int for k in ['fame', 'earned'])
                or not 0 <= row['fame'] <= row['earned'] <= 2147483647
                or type(row['board']) is not str or len(row['board'].encode('utf-16-le')) // 2 > 4096
                or any((ord(c) < 32 or 127 <= ord(c) <= 159) and c not in '\r\n\t' for c in row['board'])):
            raise ValueError('Invalid guild record')
        if row['id'] in guilds or row['name'].lower() in names:
            raise ValueError('Duplicate guild identity')
        guilds[row['id']] = row
        names.add(row['name'].lower())
    members, counts, founders, contributions = set(), {}, {}, {}
    for row in game.get('guild_members', []):
        if (any(type(row[k]) is not int for k in ['account', 'guild', 'rank', 'fame'])
                or row['account'] not in accounts or row['guild'] not in guilds
                or row['rank'] not in [0, 10, 20, 30, 40] or not 0 <= row['fame'] <= 2147483647):
            raise ValueError('Invalid guild member')
        if row['account'] in members:
            raise ValueError('Account belongs to multiple guilds')
        members.add(row['account'])
        g = row['guild']
        counts[g] = counts.get(g, 0) + 1
        founders[g] = founders.get(g, 0) + (row['rank'] == 40)
        contributions[g] = contributions.get(g, 0) + row['fame']
    if any(not 1 <= counts.get(g, 0) <= 50 or founders[g] > 1 or contributions[g] > guilds[g]['earned'] for g in guilds):
        raise ValueError('Invalid guild roster, founder count or contributions')


def validate_slots(game):
    capacities, purchases, timers, living = {}, {}, set(), {}
    for row in game['accounts']:
        account, capacity = row['id'], row['character_slots']
        if (type(account) is not int or not 0 < account <= 2147483647 or account in capacities
                or type(capacity) is not int or not 1 <= capacity <= 14):
            raise ValueError('Invalid character slot capacity')
        capacities[account], purchases[account], living[account] = capacity, [], 0
    for row in game['character_slot_purchases']:
        account, timer, slots = row['account'], row['timer'], row['slots']
        if (type(account) is not int or account not in capacities
                or type(timer) is not int or not -2147483648 <= timer <= 2147483647
                or type(slots) is not int or not 2 <= slots <= 14 or (account, timer) in timers):
            raise ValueError('Invalid character slot purchase')
        timers.add((account, timer))
        purchases[account].append(slots)
    for row in game['characters']:
        if (type(row['account']) is not int or row['account'] not in capacities
                or type(row['alive']) is not int or row['alive'] not in (0, 1)):
            raise ValueError('Invalid character slot owner or living state')
        living[row['account']] += row['alive']
    for account, capacity in capacities.items():
        if sorted(purchases[account]) != list(range(2, capacity + 1)) or living[account] > capacity:
            raise ValueError('Inconsistent character slot purchase history or living capacity')


def validate_vaults(game):
    owners = {}
    for row in game['accounts']:
        account, mask = row['id'], row['vault_mask']
        if (type(account) is not int or not 0 < account <= 2147483647 or account in owners
                or type(mask) is not int or not 1 <= mask < (1 << 20) or not mask & 1):
            raise ValueError('Invalid vault ownership')
        owners[account] = mask
    found = {account: 0 for account in owners}
    for row in game['vaults']:
        account, chest, revision = row['account'], row['chest'], row['revision']
        if (type(account) is not int or account not in owners or type(chest) is not int or not 0 <= chest < 20
                or type(revision) is not int or not 0 <= revision <= 2147483647 or found[account] & (1 << chest)):
            raise ValueError('Invalid or duplicate vault chest identity/revision')
        found[account] |= 1 << chest
    if found != owners:
        raise ValueError('Vault ownership/count/rows disagree')


BONUS_KINDS = ('ancestor', 'legacy_builder', 'pacifist', 'thirsty', 'mundane', 'boots_on_the_ground', 'tunnel_rat', 'enemy_of_the_gods', 'slayer_of_the_gods', 'oryx_slayer', 'accurate', 'sharpshooter', 'sniper', 'explorer', 'cartographer', 'team_player', 'leader_of_men', 'doer_of_deeds', 'friend_of_the_cubes', 'well_equipped', 'first_born')


def paid_receipt(raw):
    if type(raw) is not str:
        raise ValueError('Invalid death receipt JSON')
    def unique_fields(fields):
        result = dict(fields)
        if len(result) != len(fields):
            raise ValueError('Duplicate death receipt JSON field')
        return result
    receipt = json.loads(raw, object_pairs_hook=unique_fields)
    if (type(receipt) is not dict or set(receipt) != {'version', 'policy', 'base', 'rows', 'total'}
            or type(receipt['version']) is not int or receipt['version'] != 1
            or receipt['policy'] not in ('legacy_paid', 'equipment_only', 'historical')
            or any(type(receipt[k]) is not int or not 0 <= receipt[k] <= 2147483647 for k in ('base', 'total'))
            or type(receipt['rows']) is not list or len(receipt['rows']) > 21):
        raise ValueError('Invalid death receipt version, policy or bounds')
    previous, total = -1, receipt['base']
    for row in receipt['rows']:
        if (type(row) is not dict or set(row) != {'kind', 'amount'}
                or row['kind'] not in BONUS_KINDS or BONUS_KINDS.index(row['kind']) <= previous
                or type(row['amount']) is not int or not 0 <= row['amount'] <= 2147483647
                or (receipt['policy'] != 'historical' and (row['kind'] != 'well_equipped' or row['amount'] == 0))):
            raise ValueError('Invalid death receipt kind, order or amount')
        previous = BONUS_KINDS.index(row['kind'])
        total += row['amount']
    if total != receipt['total'] or total > 2147483647:
        raise ValueError('Death receipt base plus rows disagrees with total')
    return receipt


def validate_fame_records(game, saved):
    required, seen = set(), set()
    for row in game['deaths']:
        receipt = paid_receipt(row['receipt'])
        if any(r['kind'] == 'legacy_builder' for r in receipt['rows']):
            ch = json.loads(row['snapshot'])
            required.add((ch['account'], ch['class'], row['character']))
    for row in game['legacy_claims']:
        if (any(type(row[k]) is not int for k in ('account', 'class', 'character'))
                or not 0 <= row['class'] <= 65535 or (row['account'], row['class']) in seen
                or (row['account'], row['class'], row['character']) not in required):
            raise ValueError('Invalid Legacy claim ownership or receipt binding')
        seen.add((row['account'], row['class']))
        required.remove((row['account'], row['class'], row['character']))
    if required:
        raise ValueError('Paid Legacy receipt lacks its claim')
    seen = set()
    for row in game['level_credits']:
        recipient, level, giver = row['recipient'], row['reached_level'], row['giver']
        if (any(type(v) is not int for v in (recipient, level, giver)) or recipient not in saved
                or giver not in saved or recipient == giver or not 2 <= level <= 20
                or level > json.loads(saved[recipient]['stats'])['level'] or (recipient, level) in seen):
            raise ValueError('Invalid level credit identity, binding or reached level')
        seen.add((recipient, level))


def validate_deaths(game, version):
    accounts = {row['id'] for row in game['accounts']}
    saved = {row['id']: row for row in game['characters']}
    if len(saved) != len(game['characters']):
        raise ValueError('Duplicate character identity')
    for row in saved.values():
        payload = json.loads(row['stats'])
        # Creation serializes stats before SQLite assigns the canonical character row ID.
        if (type(row['id']) is not int or row['id'] <= 0 or row['account'] not in accounts
                or payload['id'] not in (0, row['id']) or payload['account'] != row['account']):
            raise ValueError('Invalid character ownership')
    seen, earned = set(), {account: 0 for account in accounts}
    for row in game['deaths']:
        character = row['character']
        snapshot = json.loads(row['snapshot'])
        bonus = row['equipment_fame'] if version >= 14 else 0
        # The explicit legacy decoder recognizes the version-zero Wizard-only writer.
        if (type(character) is not int or character not in saved or character in seen
                or saved[character]['alive'] != 0 or snapshot['id'] != character
                or (snapshot['class'] if 'schema' in snapshot else 782) != (json.loads(saved[character]['stats'])['class'] if 'schema' in json.loads(saved[character]['stats']) else 782)
                or snapshot['account'] != saved[character]['account'] or snapshot['alive'] is not False
                or snapshot['hp'] != 0 or type(snapshot['fame']) is not int or snapshot['fame'] < 0
                or type(bonus) is not int or bonus < 0 or snapshot['fame'] + bonus > 2147483647):
            raise ValueError('Invalid archived death award or ownership')
        total = snapshot['fame'] + bonus
        if version >= 17:
            receipt = paid_receipt(row['receipt'])
            equipment = next((r['amount'] for r in receipt['rows'] if r['kind'] == 'well_equipped'), 0)
            if receipt['base'] != snapshot['fame'] or equipment != bonus:
                raise ValueError('Death receipt disagrees with archived base or equipment')
            total = receipt['total']
        seen.add(character)
        earned[snapshot['account']] += total
    if version >= 17:
        validate_fame_records(game, saved)
    if version >= 5:
        for row in game['account_fame']:
            if earned[row['account']] != row['earned']:
                raise ValueError('Death awards disagree with account earned fame')
    if version >= 9:
        wallets = {row['account']: row['earned'] for row in game['account_fame']}
        if (any(row['fame'] > wallets[row['account']] for row in game['guild_members'])
                or sum(row['earned'] for row in game['guilds']) > sum(wallets.values())):
            raise ValueError('Guild contribution or earnings exceed account earned fame')


def validate_community(game):
    accounts = {row['id'] for row in game['accounts']}
    saved = {row['id']: row['account'] for row in game['characters']}
    ids, keys, minds = set(), set(), set()
    for row in game['bot_minds']:
        if (type(row['account']) is not int or row['account'] not in accounts or row['account'] in minds
                or type(row['revision']) is not int or row['revision'] <= 0 or type(row['mind']) is not str):
            raise ValueError('Invalid community mind ownership or revision')
        minds.add(row['account'])
    for row in game['trade_transactions']:
        if (type(row['id']) is not str or row['id'] in ids
                or any(type(row[k]) is not str for k in ('owners', 'writes', 'revisions'))
                or (row['delivery'] is not None and type(row['delivery']) is not str)
                or (row['market_key'] is not None and (type(row['market_key']) is not str or row['market_key'] in keys))):
            raise ValueError('Invalid community trade identity or payload')
        ids.add(row['id'])
        if row['market_key'] is not None:
            keys.add(row['market_key'])
        writes = json.loads(row['writes'])
        if type(writes) is not list or any(type(w) is not dict or type(w.get('id')) is not int
                or type(w.get('account')) is not int or w['account'] not in accounts
                or saved.get(w['id']) != w['account'] for w in writes):
            raise ValueError('Invalid community trade character ownership')
    binary = os.environ.get('OPENROTMG_BACKUP_BINARY')
    if not binary:
        raise RuntimeError('OPENROTMG_BACKUP_BINARY must name the matching release binary')
    result = subprocess.run([binary, '--backup-validate-community'],
                            input=json.dumps({k: game[k] for k in ('bot_minds', 'trade_transactions')}),
                            text=True, capture_output=True, timeout=30)
    if result.returncode:
        raise ValueError('Release community validation failed: ' + result.stderr.strip())
    if json.loads(result.stdout) != {'validated_minds': len(minds), 'validated_trade_receipts': len(ids)}:
        raise ValueError('Release community validator returned an invalid receipt')


def validate(bundle):
    if set(bundle) != {'schema', 'created_at', 'revisions', 'game', 'discord'} or type(bundle['schema']) is not int or bundle['schema'] not in GAME_SCHEMAS:
        raise ValueError('Unsupported backup envelope')
    datetime.datetime.strptime(bundle['created_at'], '%Y-%m-%dT%H:%M:%SZ')
    if set(bundle['revisions']) not in ({'server', 'discord_bot'}, {'server', 'discord_bot', 'validator'}) or any(not re.fullmatch('[0-9a-f]{40}', v) for v in bundle['revisions'].values()):
        raise ValueError('Invalid source revisions')
    for name, schema in [('game', GAME_SCHEMAS[bundle['schema']]), ('discord', DISCORD)]:
        data = bundle[name]
        if set(data) != set(schema):
            raise ValueError('Unsupported backup tables: ' + name)
        for table, columns in schema.items():
            allowed = set(columns) - {'guid'}
            if not isinstance(data[table], list) or any(not isinstance(r, dict) or set(r) != allowed for r in data[table]):
                raise ValueError('Unsupported backup fields: ' + table)
    current = [json.loads(row['stats']) for row in bundle['game']['characters']]
    legacy = []
    if bundle['schema'] < 17:
        legacy = current + [json.loads(row['snapshot']) for row in bundle['game']['deaths']]
        current = []
    else:
        for row in bundle['game']['deaths']:
            receipt = paid_receipt(row['receipt'])
            (legacy if receipt['policy'] == 'legacy_paid' else current).append(json.loads(row['snapshot']))
    characters(current)
    if legacy:
        characters(legacy, legacy=True)
    for row in bundle['game']['characters']:
        items = json.loads(row['items'])
        if not isinstance(items, list) or len(items) != 12 or any(type(i) is not int for i in items):
            raise ValueError('Invalid inventory')
    for row in bundle['game'].get('vaults', []):
        items = json.loads(row['items'])
        if type(row['account']) is not int or row['account'] <= 0 or type(row['revision']) is not int or row['revision'] < 0:
            raise ValueError('Invalid vault account/revision')
        if not isinstance(items, list) or len(items) != 8 or any(type(i) is not int or not -1 <= i <= 65535 for i in items):
            raise ValueError('Invalid eight-slot vault inventory')
    bots = bundle['game'].get('shore_bots', [])
    accounts = {row['id'] for row in bundle['game']['accounts']}
    for row in bundle['game']['accounts']:
        if bundle['schema'] >= 8 and (type(row['credits']) is not int or not 0 <= row['credits'] <= 2147483647):
            raise ValueError('Invalid account gold balance')
    for row in bots:
        if not isinstance(row['name'], str) or not row['name'] or type(row['account']) is not int or row['account'] <= 0 or row['account'] not in accounts:
            raise ValueError('Invalid shore bot name/account')
    if len({row['name'] for row in bots}) != len(bots) or len({row['account'] for row in bots}) != len(bots):
        raise ValueError('Duplicate shore bot name/account')
    for table in ['competitors', 'collections', 'discord_links', 'account_fame', 'discord_recovery']:
        rows = bundle['game'].get(table, [])
        for row in rows:
            if type(row['account']) is not int or row['account'] <= 0 or row['account'] not in accounts:
                raise ValueError('Invalid hiscores owner: ' + table)
            if table == 'account_fame' and (type(row['balance']) is not int or type(row['earned']) is not int or not 0 <= row['balance'] <= row['earned'] <= 2147483647):
                raise ValueError('Invalid account fame wallet')
            if table == 'discord_recovery' and (row['account'] not in {r['account'] for r in bundle['game']['discord_links']} or type(row['enabled_at']) is not int or row['enabled_at'] <= 0 or type(row['last_issued_at']) is not int or row['last_issued_at'] < 0):
                raise ValueError('Invalid recovery permission')
            if table == 'collections' and (type(row['item']) is not int or not 0 <= row['item'] <= 65535 or type(row['found_at']) is not int or row['found_at'] <= 0):
                raise ValueError('Invalid collection record')
            if table == 'discord_links' and (not isinstance(row['discord_id'], str) or not re.fullmatch(r'[0-9]{17,20}', row['discord_id'])):
                raise ValueError('Invalid linked Discord identity')
        identities = [(row['account'], row['item']) if table == 'collections' else row['account'] for row in rows]
        if len(set(identities)) != len(rows) or (table == 'discord_links' and len({r['discord_id'] for r in rows}) != len(rows)):
            raise ValueError('Duplicate hiscores identity: ' + table)
    rewards = bundle['game'].get('gold_rewards', [])
    reward_types = gold_reward_types(bundle['schema'])
    for row in rewards:
        if type(row['account']) is not int or row['account'] not in accounts or row['reward'] not in reward_types or type(row['awarded_at']) is not int or row['awarded_at'] <= 0 or (row['notified_at'] is not None and (type(row['notified_at']) is not int or row['notified_at'] <= 0)):
            raise ValueError('Invalid gold reward')
    if len({(row['account'], row['reward']) for row in rewards}) != len(rewards):
        raise ValueError('Duplicate gold reward')
    lists = {}
    for row in bundle['game'].get('account_lists', []):
        if any(type(row[key]) is not int or not 0 < row[key] <= 2147483647 or row[key] not in accounts for key in ['account', 'target']) or type(row['list_id']) is not int or row['list_id'] not in (0, 1) or row['account'] == row['target']:
            raise ValueError('Invalid account list entry')
        targets = lists.setdefault((row['account'], row['list_id']), set())
        if row['target'] in targets:
            raise ValueError('Duplicate account list entry')
        targets.add(row['target'])
        if len(targets) > 32767:
            raise ValueError('Account list exceeds native count limit')
    updates = bundle['game'].get('update_feed', [])
    if len(updates) > 1:
        raise ValueError('Invalid update feed row count')
    for row in updates:
        if type(row['id']) is not int or row['id'] != 1 or type(row['synced_at']) is not int or row['synced_at'] <= 0 or not isinstance(row['snapshot'], str):
            raise ValueError('Invalid update feed record')
        update_snapshot(row['snapshot'])
    if bundle['schema'] >= 5 and {r['account'] for r in bundle['game']['account_fame']} != accounts:
        raise ValueError('Every account requires a fame wallet')
    validate_guilds(bundle['game'], accounts)
    if bundle['schema'] >= 12:
        validate_slots(bundle['game'])
    if bundle['schema'] >= 13:
        validate_vaults(bundle['game'])
    if bundle['schema'] >= 15:
        validate_pictures(bundle['game'])
    if bundle['schema'] >= 16:
        validate_community(bundle['game'])
    validate_deaths(bundle['game'], bundle['schema'])
    return bundle


def gold_reward_types(version):
    return ('discord_link', 'collection_top_half') + (('first_bug_report',) if version >= 11 else ()) + (('class_star_blue', 'class_star_red', 'class_star_orange', 'class_star_yellow', 'class_star_white') if version >= 18 else ())


def gold_reward_sql(version):
    rewards = ','.join(repr(r) for r in gold_reward_types(version))
    return f"CREATE TABLE gold_rewards(account INTEGER NOT NULL REFERENCES accounts(id),reward TEXT NOT NULL CHECK(reward IN ({rewards})),awarded_at INTEGER NOT NULL CHECK(awarded_at>0),notified_at INTEGER CHECK(notified_at>0),PRIMARY KEY(account,reward))"


def export(game, discord, output, server_revision, bot_revision, validator_revision):
    game_data, matched, stored_version = snapshot(game, [*GAME_SCHEMAS.values(), AUTH_GAME, AUTH_SHORE_GAME, AUTH_HISCORES_GAME, AUTH_ACTIONS_GAME, AUTH_SOCIAL_GAME, AUTH_UPDATES_GAME, AUTH_GOLD_GAME, AUTH_GUILDS_GAME, AUTH_REWARDS_GAME, AUTH_SLOTS_GAME, AUTH_VAULTS_GAME, RELOADED_FAME_GAME, RELOADED_PICTURES_GAME, RELOADED_COMMUNITY_GAME, RELOADED_GAME], include_schema=True)
    versions = [v for v, schema in GAME_SCHEMAS.items() if schema == {k: c for k, c in matched.items() if k in game_data}]
    if (stored_version > max(GAME_SCHEMAS) or
            ((max(versions) >= 12 or stored_version >= 12) and stored_version not in versions)):
        raise ValueError('Unsupported database schema version')
    if stored_version >= 18:
        with closing(sqlite3.connect(game)) as db:
            if db.execute("SELECT sql FROM sqlite_schema WHERE name='gold_rewards'").fetchone() != (gold_reward_sql(stored_version),):
                raise ValueError('Schema-18 gold reward metadata mismatch')
    if len(versions) > 1 and stored_version not in versions:
        raise ValueError('Unsupported gold reward schema version')
    version = stored_version if len(versions) > 1 else versions[0]
    bundle = validate({
        'schema': version, 'created_at': datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ'),
        'revisions': {'server': Path(server_revision).read_text().strip(), 'discord_bot': Path(bot_revision).read_text().strip(),
                      'validator': Path(validator_revision).read_text().strip()},
        'game': game_data, 'discord': snapshot(discord, DISCORD),
    })
    data = gzip.compress(json.dumps(bundle, sort_keys=True, separators=(',', ':')).encode(), mtime=0)
    output = Path(output)
    temporary = output.with_name(output.name + '.new')
    with temporary.open('xb') as file:
        file.write(data)
    temporary.replace(output)
    return bundle


def restore(archive, directory):
    bundle = validate(json.loads(gzip.decompress(Path(archive).read_bytes())))
    directory = Path(directory)
    directory.mkdir(mode=0o700)
    # NULL identities cannot authenticate; ownership must be reverified separately after a disaster.
    definitions = {
        'game': """CREATE TABLE accounts(id INTEGER PRIMARY KEY,guid TEXT UNIQUE,name TEXT NOT NULL);
CREATE TABLE characters(id INTEGER PRIMARY KEY,account INTEGER NOT NULL REFERENCES accounts(id),stats TEXT NOT NULL,items TEXT NOT NULL,revision INTEGER NOT NULL DEFAULT 0,alive INTEGER NOT NULL DEFAULT 1);
CREATE UNIQUE INDEX one_living_character ON characters(account) WHERE alive=1;
CREATE TABLE deaths(character INTEGER PRIMARY KEY REFERENCES characters(id),snapshot TEXT NOT NULL,killed_by TEXT NOT NULL,died_at INTEGER NOT NULL DEFAULT(unixepoch()));""",
        'discord': """CREATE TABLE levels(guild_id TEXT NOT NULL,user_id TEXT NOT NULL,username TEXT NOT NULL,xp INTEGER NOT NULL CHECK(xp>=0 AND xp<=9007199254740991),last_xp_at INTEGER NOT NULL,last_message_id TEXT NOT NULL,PRIMARY KEY(guild_id,user_id)) STRICT;""",
    }
    if 2 <= bundle['schema'] < 13:
        definitions['game'] += '\nCREATE TABLE vaults(account INTEGER PRIMARY KEY REFERENCES accounts(id),items TEXT NOT NULL CHECK(json_array_length(items)=8),revision INTEGER NOT NULL DEFAULT 0);'
    if bundle['schema'] >= 3:
        definitions['game'] += '\nCREATE TABLE shore_bots(name TEXT PRIMARY KEY,account INTEGER NOT NULL UNIQUE REFERENCES accounts(id));'
    if bundle['schema'] >= 4:
        definitions['game'] += '''
CREATE TABLE competitors(account INTEGER PRIMARY KEY REFERENCES accounts(id));
CREATE TABLE collections(account INTEGER NOT NULL REFERENCES accounts(id),item INTEGER NOT NULL CHECK(item BETWEEN 0 AND 65535),found_at INTEGER NOT NULL,PRIMARY KEY(account,item));
CREATE TABLE discord_links(account INTEGER PRIMARY KEY REFERENCES accounts(id),discord_id TEXT NOT NULL UNIQUE);'''
    if bundle['schema'] >= 5:
        definitions['game'] += '''
CREATE TABLE account_fame(account INTEGER PRIMARY KEY REFERENCES accounts(id),balance INTEGER NOT NULL CHECK(balance BETWEEN 0 AND 2147483647),earned INTEGER NOT NULL CHECK(earned BETWEEN balance AND 2147483647));
CREATE TABLE discord_recovery(account INTEGER PRIMARY KEY REFERENCES discord_links(account),enabled_at INTEGER NOT NULL,last_issued_at INTEGER NOT NULL);'''
    if bundle['schema'] >= 6:
        definitions['game'] += '\nCREATE TABLE account_lists(account INTEGER NOT NULL REFERENCES accounts(id),list_id INTEGER NOT NULL CHECK(list_id IN (0,1)),target INTEGER NOT NULL REFERENCES accounts(id),PRIMARY KEY(account,list_id,target),CHECK(account<>target));'
    if bundle['schema'] >= 7:
        definitions['game'] += '\nCREATE TABLE update_feed(id INTEGER PRIMARY KEY CHECK(id=1),snapshot TEXT NOT NULL CHECK(json_valid(snapshot)),synced_at INTEGER NOT NULL);'
    if bundle['schema'] >= 8:
        definitions['game'] += '\nALTER TABLE accounts ADD COLUMN credits INTEGER NOT NULL DEFAULT 100 CHECK(credits BETWEEN 0 AND 2147483647);'
    if bundle['schema'] >= 9:
        definitions['game'] += '''
CREATE TABLE guilds(id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(name) BETWEEN 1 AND 20 AND name=trim(name) AND name NOT GLOB '*[^A-Za-z ]*'),level INTEGER NOT NULL DEFAULT 1 CHECK(level BETWEEN 1 AND 4),fame INTEGER NOT NULL DEFAULT 0 CHECK(fame BETWEEN 0 AND 2147483647),earned INTEGER NOT NULL DEFAULT 0 CHECK(earned BETWEEN fame AND 2147483647),board TEXT NOT NULL DEFAULT '');
CREATE TABLE guild_members(account INTEGER PRIMARY KEY REFERENCES accounts(id),guild INTEGER NOT NULL REFERENCES guilds(id),rank INTEGER NOT NULL CHECK(rank IN (0,10,20,30,40)),fame INTEGER NOT NULL DEFAULT 0 CHECK(fame BETWEEN 0 AND 2147483647));
CREATE UNIQUE INDEX one_guild_founder ON guild_members(guild) WHERE rank=40;
CREATE INDEX guild_roster ON guild_members(guild);
CREATE TRIGGER guild_capacity BEFORE INSERT ON guild_members WHEN (SELECT count(*) FROM guild_members WHERE guild=NEW.guild)>=50 BEGIN SELECT RAISE(ABORT,'Guild is full.'); END;'''
    if bundle['schema'] >= 10:
        definitions['game'] += '\n' + gold_reward_sql(bundle['schema']) + ';'
    if bundle['schema'] >= 12:
        definitions['game'] += '''
ALTER TABLE accounts ADD COLUMN character_slots INTEGER NOT NULL DEFAULT 1 CHECK(typeof(character_slots)='integer' AND character_slots BETWEEN 1 AND 14);
CREATE TABLE character_slot_purchases(account INTEGER NOT NULL REFERENCES accounts(id),timer INTEGER NOT NULL CHECK(typeof(timer)='integer' AND timer BETWEEN -2147483648 AND 2147483647),slots INTEGER NOT NULL CHECK(typeof(slots)='integer' AND slots BETWEEN 2 AND 14),PRIMARY KEY(account,timer),UNIQUE(account,slots));
DROP INDEX one_living_character;
CREATE INDEX living_characters_by_account ON characters(account) WHERE alive=1;
CREATE TRIGGER character_slot_insert BEFORE INSERT ON characters WHEN NEW.alive=1 AND NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account AND character_slots>(SELECT count(*) FROM characters WHERE account=NEW.account AND alive=1)) BEGIN SELECT RAISE(ABORT,'No free character slots.'); END;
CREATE TRIGGER character_slot_update BEFORE UPDATE OF account,alive ON characters WHEN NEW.alive=1 AND NOT EXISTS(SELECT 1 FROM accounts WHERE id=NEW.account AND character_slots>(SELECT count(*) FROM characters WHERE account=NEW.account AND alive=1 AND id<>OLD.id)) BEGIN SELECT RAISE(ABORT,'No free character slots.'); END;
CREATE TRIGGER character_slot_capacity BEFORE UPDATE OF character_slots ON accounts WHEN NEW.character_slots<(SELECT count(*) FROM characters WHERE account=NEW.id AND alive=1) BEGIN SELECT RAISE(ABORT,'Living characters exceed capacity.'); END;'''
    if bundle['schema'] >= 13:
        definitions['game'] += """
ALTER TABLE accounts ADD COLUMN vault_mask INTEGER NOT NULL DEFAULT 1 CHECK(typeof(vault_mask)='integer' AND vault_mask BETWEEN 1 AND 1048575 AND (vault_mask & 1)=1);
CREATE TABLE vaults(account INTEGER NOT NULL REFERENCES accounts(id),chest INTEGER NOT NULL CHECK(typeof(chest)='integer' AND chest BETWEEN 0 AND 19),items TEXT NOT NULL CHECK(json_array_length(items)=8),revision INTEGER NOT NULL DEFAULT 0 CHECK(typeof(revision)='integer' AND revision BETWEEN 0 AND 2147483647),PRIMARY KEY(account,chest));"""
    if bundle['schema'] >= 14:
        definitions['game'] += "\nALTER TABLE deaths ADD COLUMN equipment_fame INTEGER NOT NULL CHECK(typeof(equipment_fame)='integer' AND equipment_fame BETWEEN 0 AND 2147483647);"
    if bundle['schema'] >= 15:
        definitions['game'] += '\n' + PICTURES_SQL + ';\n' + PICTURE_SEQUENCE_SQL + ';'
    if bundle['schema'] >= 16:
        definitions['game'] += '\n' + ';\n'.join(COMMUNITY_SQL.values()) + ';'
    if bundle['schema'] >= 17:
        definitions['game'] += '\nDROP TABLE deaths;\n' + DEATHS_SQL + ';\n' + CLAIMS_SQL + ';\n' + CREDITS_SQL + ';\n' + CREDITS_INDEX_SQL + ';\nCREATE INDEX deaths_by_time ON deaths(died_at);'
    for name, filename in [('game', 'game.db'), ('discord', 'levels.sqlite')]:
        with closing(sqlite3.connect(':memory:')) as db, db:
            db.execute('PRAGMA foreign_keys=ON')
            db.executescript(definitions[name])
            if name == 'game' and bundle['schema'] >= 5:
                db.execute('PRAGMA user_version=' + str(bundle['schema']))
            for table, fields in (GAME_SCHEMAS[bundle['schema']] if name == 'game' else DISCORD).items():
                rows = bundle[name][table]
                columns = tuple(c for c in fields if c != 'guid')
                db.executemany('INSERT INTO ' + table + '(' + ','.join(columns) + ') VALUES(' + ','.join('?' for _ in columns) + ')', [tuple(base64.b64decode(row[c], validate=True) if table == 'pictures' and c == 'png' else row[c] for c in columns) for row in rows])
            db.commit()
            if db.execute('PRAGMA foreign_key_check').fetchall() or db.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
                raise ValueError('Restored database integrity failed')
            path = directory / filename
            with path.open('xb'):
                pass
            with closing(sqlite3.connect(path)) as target:
                db.backup(target)
    for name, filename in [('game', 'game.db'), ('discord', 'levels.sqlite')]:
        schema = GAME_SCHEMAS[bundle['schema']] if name == 'game' else DISCORD
        if snapshot(directory / filename, schema) != bundle[name]:
            raise ValueError('Restored fields differ from uploaded archive: ' + name)
    return bundle


if __name__ == '__main__':
    os.umask(0o077)
    parser = argparse.ArgumentParser(description='Credential-free persisted-state export and offline restore')
    commands = parser.add_subparsers(dest='command', required=True)
    exp = commands.add_parser('export')
    for option in ['game', 'discord', 'output', 'server-revision', 'bot-revision', 'validator-revision']:
        exp.add_argument('--' + option, required=True)
    res = commands.add_parser('restore')
    res.add_argument('--archive', required=True)
    res.add_argument('--directory', required=True)
    args = vars(parser.parse_args())
    command = args.pop('command')
    bundle = export(**args) if command == 'export' else restore(**args)
    print(json.dumps({'event': command + '_complete', 'created_at': bundle['created_at'], 'counts': {name: {table: len(rows) for table, rows in bundle[name].items()} for name in ['game', 'discord']}}))

#!/usr/bin/env python3
"""Explain lag from the server's capped lag log: deadline misses by cause, player stalls, host contention."""
import argparse
from collections import Counter, defaultdict
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import re

TARGET_MISS_RATE = 0.001
TARGET_STALL_RATE = 0.01
TARGET_FREE_DISK = 10 * 10**9
UPDATES_PER_SECOND = 10


def moment(text):
    relative = re.fullmatch(r'(\d+)([mhd])', text)
    if relative:
        unit = {'m': 'minutes', 'h': 'hours', 'd': 'days'}[relative[2]]
        return datetime.now(timezone.utc) - timedelta(**{unit: int(relative[1])})
    return datetime.fromisoformat(text).astimezone(timezone.utc)


def records(directory, since, until):
    paths = sorted(Path(directory).glob('lag-*.jsonl'))
    if not paths:
        raise SystemExit(f'No lag log files in {directory}')
    for path in paths:
        for line in path.read_text().splitlines():
            row = json.loads(line)
            at = datetime.fromisoformat(row['timestamp'].replace('Z', '+00:00'))
            if since <= at < until:
                yield at, row['fields']


def report(rows):
    ticks, misses, causes = Counter(), Counter(), defaultdict(Counter)
    update_intervals, stalls, missed, hosts = 0.0, [], [], []
    names = {}
    for at, f in rows:
        kind = f['message']
        if kind == 'world tick summary':
            s = json.loads(f['summary'])
            names[f['world_id']] = f['world']
            ticks[f['world_id']] += s['step_wall']['count'] or 0
            for counter, n in (s['missed_by_cause'] or {}).items():
                misses[f['world_id']] += n
                causes[f['world_id']][counter.removeprefix('missed_')] += n
            update_intervals += f['humans'] * 5 * UPDATES_PER_SECOND
        elif kind == 'tick deadline missed':
            missed.append((at, f['world'], json.loads(f['miss']), json.loads(f['phases'])))
        elif kind == 'player update stall':
            stalls.append((at, f.get('player'), f.get('world'), f['gap_ms']))
        elif kind == 'host snapshot':
            hosts.append((at, json.loads(f['host'])))
    out = ['TARGETS']
    total_ticks, total_misses = sum(ticks.values()), sum(misses.values())
    rate = total_misses / total_ticks if total_ticks else None
    out.append(f"  tick misses: {total_misses}/{total_ticks} = {rate:.4%} (target < {TARGET_MISS_RATE:.1%}) {'OK' if rate is not None and rate < TARGET_MISS_RATE else 'BREACHED'}" if rate is not None else '  tick misses: no world summaries')
    if update_intervals:
        stall_rate = len(stalls) / update_intervals
        out.append(f"  player update gaps over 250 ms: {len(stalls)}/~{update_intervals:.0f} = {stall_rate:.3%} (target < {TARGET_STALL_RATE:.0%}, i.e. p99 < 250 ms) {'OK' if stall_rate < TARGET_STALL_RATE else 'BREACHED'}; intervals estimated from human counts")
    else:
        out.append('  player update gaps: no human players in window')
    if hosts:
        free = min(h['free_disk_bytes'] for _, h in hosts)
        out.append(f"  minimum free disk: {free / 1e9:.1f} GB (target > {TARGET_FREE_DISK / 1e9:.0f} GB) {'OK' if free > TARGET_FREE_DISK else 'BREACHED'}")
    out.append('\nMISSED TICKS BY WORLD AND CAUSE')
    for world, n in misses.most_common():
        out.append(f"  {world} {names[world]}: {n}/{ticks[world]} ticks; " + ', '.join(f'{c} {k}' for c, k in ((v, k) for k, v in causes[world].most_common())))
    out.append('\nPLAYER STALLS')
    explained = Counter()
    for at, player, world, gap in stalls:
        start = at - timedelta(milliseconds=gap)
        overlap = [m for m in missed if m[1] == world and start <= m[0] <= at]
        cause = Counter(m[2]['cause'] for m in overlap).most_common(1)
        label = f"world tick {cause[0][0]}" if cause else 'beyond simulation (dispatch, proxy or network)'
        explained[label] += 1
        out.append(f"  {at:%H:%M:%S} player {player} world {world} gap {gap:.0f} ms -> {label}" + (f" ({len(overlap)} missed ticks)" if overlap else ''))
    out.append('  summary: ' + (', '.join(f'{n} {k}' for k, n in explained.most_common()) or 'none'))
    if hosts:
        out.append('\nHOST')
        cpu = [h['pressure_some_share']['cpu'] for _, h in hosts if h['pressure_some_share']]
        if cpu:
            out.append(f"  CPU pressure (share of time some task waited): mean {sum(cpu) / len(cpu):.0%}, max {max(cpu):.0%}")
        groups = defaultdict(lambda: [0.0, 0.0])
        for _, h in hosts:
            for name, g in h['game_threads'].items():
                groups[name][0] += g['running_cores'] / len(hosts)
                groups[name][1] += g['cpu_wait_cores'] / len(hosts)
        for name, (run, wait) in sorted(groups.items(), key=lambda kv: -kv[1][0])[:5]:
            out.append(f"  game {name}: running {run:.2f} cores, waiting for CPU {wait:.2f} cores")
        others = Counter()
        for _, h in hosts:
            for p in h['other_processes']:
                others[p['name']] += p['cores'] / len(hosts)
        out.append('  other processes: ' + ', '.join(f'{n} {c:.2f} cores' for n, c in others.most_common(3)))
    out.append('\nWORST MISSED TICKS')
    for at, world, miss, phases in sorted(missed, key=lambda m: -(m[2]['lateness_ms'] + m[2]['wall_ms']))[:10]:
        slow = ', '.join(f"{p['phase']} {p['wall_ms']:.0f}" for p in phases[:3])
        out.append(f"  {at:%H:%M:%S} world {world} {miss['cause']}: late {miss['lateness_ms']:.0f}, wall {miss['wall_ms']:.0f}, cpu {miss.get('cpu_ms', 0):.0f}, cpu_wait {miss.get('cpu_wait_ms', 0):.0f}, blocked {miss.get('blocked_ms', 0):.0f} ms; {slow}")
    return '\n'.join(out)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--dir', default='/var/lib/openrotmg/lag')
    parser.add_argument('--since', default='1h', help='ISO time or relative: 30m, 6h, 2d')
    parser.add_argument('--until', default='0m')
    args = parser.parse_args()
    print(report(records(args.dir, moment(args.since), moment(args.until))))

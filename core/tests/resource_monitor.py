#!/usr/bin/env python3
"""Sample one daemon's RSS without accessing clipboard or network."""
import argparse
import datetime
import json
import os
from pathlib import Path
import time


def snapshot(pid):
    stat = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
    rss = int(Path(f'/proc/{pid}/statm').read_text().split()[1]) * os.sysconf('SC_PAGE_SIZE')
    return {'start_ticks': int(stat[19]), 'cpu_ticks': int(stat[11]) + int(stat[12]), 'rss_bytes': rss}


def utc():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def save(path, data):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    temp = path.with_name(path.name + '.tmp')
    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, 'w') as stream:
        json.dump(data, stream, indent=2)
        stream.write('\n')
    os.replace(temp, path)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--pid', type=int, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--seconds', type=int, default=600)
    args = parser.parse_args()
    if args.seconds < 1:
        parser.error('--seconds must be positive')
    start = time.monotonic()
    initial = snapshot(args.pid)
    stats = {'state': 'running', 'pid': args.pid, 'started_utc': utc(),
             'elapsed_seconds': 0, 'samples': 1, 'initial_rss_bytes': initial['rss_bytes'],
             'last_rss_bytes': initial['rss_bytes'], 'peak_rss_bytes': initial['rss_bytes']}
    save(args.output, stats)
    try:
        while time.monotonic() - start < args.seconds:
            time.sleep(min(60, max(0, args.seconds - (time.monotonic() - start))))
            current = snapshot(args.pid)
            if current['start_ticks'] != initial['start_ticks']:
                raise RuntimeError('daemon restarted')
            elapsed = time.monotonic() - start
            stats.update(elapsed_seconds=round(elapsed, 1), samples=stats['samples'] + 1,
                         last_rss_bytes=current['rss_bytes'],
                         peak_rss_bytes=max(stats['peak_rss_bytes'], current['rss_bytes']),
                         average_cpu_percent=round((current['cpu_ticks'] - initial['cpu_ticks']) /
                                                   os.sysconf('SC_CLK_TCK') / elapsed * 100, 4))
            save(args.output, stats)
        stats.update(state='complete', completed_utc=utc(),
                     rss_growth_bytes=stats['last_rss_bytes'] - stats['initial_rss_bytes'])
        save(args.output, stats)
    except (FileNotFoundError, ProcessLookupError, RuntimeError) as error:
        stats.update(state='interrupted', completed_utc=utc(), reason=type(error).__name__)
        save(args.output, stats)
        raise


if __name__ == '__main__':
    main()

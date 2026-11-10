#!/usr/bin/env python3
"""Run only the bind fixture in a new owned user+mount namespace; denial is failure, not skip."""
import argparse
import os
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--test-binary', required=True)
args = parser.parse_args()
assert os.getuid() != 0, 'Use the ordinary checkout owner, never host root'
env = os.environ.copy()
env['STARTUP_BACKUP_ORIGINAL_MOUNT_NS'] = os.readlink('/proc/self/ns/mnt')
env['STARTUP_BACKUP_ORIGINAL_USER_NS'] = os.readlink('/proc/self/ns/user')
env['STARTUP_BACKUP_ORIGINAL_UID'] = str(os.getuid())
raise SystemExit(subprocess.run([
    'unshare', '--user', '--map-root-user', '--mount', '--propagation', 'private', '--fork',
    args.test_binary, '--exact',
    'account_save::startup_backup::tests::startup_backup_rejects_native_root_and_descendant_bind_mounts',
    '--ignored', '--nocapture',
], env=env).returncode)

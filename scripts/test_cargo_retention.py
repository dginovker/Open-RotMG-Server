import concurrent.futures
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('retention', Path(__file__).with_name('cargo.py'))
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class RetainedELFTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.pool = self.root / 'objects'
        self.binary = self.root / 'compiler-output'
        self.binary.write_bytes(b'\x7fELF-original-fixture')
        self.binary.chmod(0o755)
        self.env = patch.dict(os.environ, {'OPENROTMG_RETAINED_STORE': str(self.pool)})
        self.env.start()
        self.addCleanup(self.env.stop)
        self.source = patch.object(m, 'ROOT', self.root / 'source')
        self.source.start()
        self.addCleanup(self.source.stop)

    def retain(self, name):
        path = self.root / name / 'openrotmg'
        m.retain_binary(self.binary, path)
        return path

    def test_repeated_runs_share_immutable_object_not_compiler_inode(self):
        a, b = self.retain('run-a'), self.retain('run-b')
        self.assertEqual(a.stat().st_ino, b.stat().st_ino)
        self.assertNotEqual(a.stat().st_ino, self.binary.stat().st_ino)
        self.assertEqual(a.stat().st_mode & 0o777, 0o555)
        self.assertEqual(len(list(self.pool.glob('*.bin'))), 1)
        self.assertEqual(a.stat().st_nlink, 3)

    def test_provenance_records_every_output_and_source(self):
        a, b = self.retain('run-a'), self.retain('run-b')
        record = json.loads(next(self.pool.glob('*.json')).read_text())
        self.assertEqual(record, {'version': 1, 'sources': [str(m.ROOT)],
                                  'outputs': sorted([str(a), str(b)])})

    def test_legacy_object_is_not_falsely_given_complete_provenance(self):
        self.retain('run-a')
        next(self.pool.glob('*.json')).unlink()
        self.retain('run-b')
        self.assertEqual(list(self.pool.glob('*.json')), [])

    def test_source_is_attributed_before_any_compiler_artifact_exists(self):
        target = self.root / 'target'
        output = self.root / 'run/openrotmg'
        m.record_source(target, output)
        self.assertEqual(json.loads((target / '.source.json').read_text()),
                         {'version': 1, 'source': str(m.ROOT), 'outputs': [str(output)]})

    def test_source_identity_cannot_be_reassigned(self):
        target = self.root / 'target'
        m.record_source(target, None)
        with patch.object(m, 'ROOT', self.root / 'different-source'):
            with self.assertRaisesRegex(RuntimeError, 'does not match'):
                m.record_source(target, None)

    def test_runtime_stack_size_does_not_duplicate_compiler_targets(self):
        with patch.object(m, 'cache_root', return_value=self.root / 'cargo'), \
                patch.object(m.subprocess, 'check_output', return_value='fixture compiler'), \
                patch.object(m, 'execute', return_value=0) as execute:
            for size in ('16777216', '67108864'):
                with patch.dict(os.environ, {'RUST_MIN_STACK': size}):
                    self.assertEqual(m.main(['test']), 0)
            self.assertEqual(execute.call_args_list[0].args[1], execute.call_args_list[1].args[1])

    def test_compiler_overwrite_cannot_modify_retained_evidence(self):
        a = self.retain('run-a')
        before = a.read_bytes()
        self.binary.write_bytes(b'\x7fELF-new-compiler-fixture')
        b = self.retain('run-b')
        self.assertEqual(a.read_bytes(), before)
        self.assertNotEqual(a.stat().st_ino, b.stat().st_ino)
        self.assertEqual(b.read_bytes(), self.binary.read_bytes())

    def test_replacing_one_output_preserves_other_runs(self):
        a, b = self.retain('run-a'), self.retain('run-b')
        before = b.read_bytes()
        self.binary.write_bytes(b'\x7fELF-next-build')
        self.retain('run-a')
        self.assertEqual(b.read_bytes(), before)
        self.assertEqual(a.read_bytes(), self.binary.read_bytes())
        self.assertEqual(list(a.parent.glob('.openrotmg-*')), [])

    def test_corrupt_pool_fails_loudly_without_output(self):
        self.retain('run-a')
        obj = next(self.pool.glob('*.bin'))
        obj.chmod(0o755)
        obj.write_bytes(b'corruption')
        obj.chmod(0o555)
        with self.assertRaisesRegex(RuntimeError, 'content changed'):
            self.retain('run-b')
        self.assertFalse((self.root / 'run-b/openrotmg').exists())

    def test_permissions_never_expanded(self):
        self.binary.chmod(0o700)
        a = self.retain('run-a')
        self.assertEqual(a.stat().st_mode & 0o777, 0o500)

    def test_non_executable_rejected(self):
        self.binary.chmod(0o600)
        with self.assertRaisesRegex(RuntimeError, 'executable compiler artifact'):
            self.retain('run-a')

    def test_native_formats_are_not_assumed_to_be_elf(self):
        self.binary.write_bytes(b'\xcf\xfa\xed\xfe-native-fixture')
        out = self.retain('run-a')
        self.assertEqual(out.read_bytes(), self.binary.read_bytes())

    def test_marker_mismatch_fails_closed(self):
        self.retain('run-a')
        (self.pool / '.owner').write_text('other-data')
        with self.assertRaisesRegex(RuntimeError, 'ownership'):
            self.retain('run-b')

    def test_unowned_nonempty_pool_is_not_adopted(self):
        self.pool.mkdir(mode=0o700)
        original = self.pool / 'important-source'
        original.write_text('KEEP')
        with self.assertRaisesRegex(RuntimeError, 'nonempty'):
            self.retain('run-a')
        self.assertEqual(original.read_text(), 'KEEP')

    def test_marker_symlink_rejected(self):
        self.retain('run-a')
        owner = self.pool / '.owner'
        owner.unlink()
        owner.symlink_to(self.binary)
        with self.assertRaisesRegex(RuntimeError, 'symlink'):
            self.retain('run-b')

    def test_broken_object_symlink_is_not_silently_replaced(self):
        original = self.retain('run-a')
        obj = next(self.pool.glob('*.bin'))
        obj.unlink()
        obj.symlink_to(self.root / 'missing')
        with self.assertRaisesRegex(RuntimeError, 'must not be a symlink'):
            self.retain('run-b')
        self.assertEqual(original.read_bytes(), self.binary.read_bytes())

    def test_writable_by_others_store_rejected(self):
        self.pool.mkdir(mode=0o777)
        self.pool.chmod(0o777)
        with self.assertRaisesRegex(RuntimeError, 'not writable by others'):
            self.retain('run-a')

    def test_concurrent_retention_is_one_object(self):
        with concurrent.futures.ThreadPoolExecutor(4) as pool:
            outputs = list(pool.map(self.retain, [f'run-{i}' for i in range(8)]))
        self.assertEqual(len({p.stat().st_ino for p in outputs}), 1)
        self.assertEqual(len(list(self.pool.glob('*.bin'))), 1)
        self.assertEqual(list(self.pool.glob('.retain-*')), [])

    def test_output_cannot_replace_immutable_store_objects(self):
        with self.assertRaisesRegex(RuntimeError, 'outside the immutable object store'):
            m.retain_binary(self.binary, self.pool / 'object')
        self.assertFalse(self.pool.exists())

    def test_persistence_failure_is_not_reported_as_success(self):
        old = self.retain('run-a')
        data = old.read_bytes()
        self.binary.write_bytes(b'\x7fELF-next')
        with patch.object(m, 'sync_directory', side_effect=OSError('fsync failed')):
            with self.assertRaisesRegex(OSError, 'fsync failed'):
                self.retain('run-b')
        self.assertEqual(old.read_bytes(), data)
        self.assertFalse((self.root / 'run-b/openrotmg').exists())

    def test_cross_filesystem_requires_explicit_store(self):
        with tempfile.TemporaryDirectory(dir='/dev/shm') as other:
            if Path(other).stat().st_dev == self.root.stat().st_dev:
                self.skipTest('fixture requires separate filesystem')
            with self.assertRaisesRegex(RuntimeError, 'share the output filesystem'):
                m.retain_binary(self.binary, Path(other) / 'run/openrotmg')


if __name__ == '__main__':
    unittest.main()

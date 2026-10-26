import errno
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('build_storage', Path(__file__).with_name('cargo.py'))
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class BuildStorageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / 'source'
        (self.source / 'scripts').mkdir(parents=True)
        shutil.copyfile(Path(__file__).resolve().parents[1] / 'rust-toolchain.toml', self.source / 'rust-toolchain.toml')
        shutil.copyfile(guard.__file__, self.source / 'scripts/cargo.py')
        source = patch.object(guard, 'ROOT', self.source)
        source.start()
        self.addCleanup(source.stop)
        env = {key: value for key, value in os.environ.items() if not key.startswith(('CARGO_', 'RUSTFLAGS', 'RUSTUP_TOOLCHAIN'))}
        env.update(XDG_CACHE_HOME=str(self.root / 'cache'))
        self.env = patch.dict(os.environ, env, clear=True)
        self.env.start()
        self.addCleanup(self.env.stop)
        self.output = patch('sys.stdout', new_callable=io.StringIO)
        self.stdout = self.output.start()
        self.addCleanup(self.output.stop)
        self.errors = patch('sys.stderr', new_callable=io.StringIO)
        self.stderr = self.errors.start()
        self.addCleanup(self.errors.stop)

    def build(self, args=None):
        with patch.object(guard.subprocess, 'check_output', return_value='rustc test host'), \
                patch.object(guard, 'execute', return_value=0) as execute:
            code = guard.main(args or ['build', '--locked'])
        return code, execute








    def test_cleanup_refuses_foreign_contents_and_symlinks_without_deleting_original(self):
        root = guard.cache_root()
        root.mkdir(parents=True)
        original = root / 'original.db'
        original.write_text('irreplaceable')
        self.assertEqual(guard.main(['clean']), 1)
        self.assertEqual(original.read_text(), 'irreplaceable')
        original.unlink()
        with guard.locked(root):
            pass
        external = self.root / 'original.db'
        external.write_text('keep')
        target = root / ('a' * 20)
        (target / 'debug').mkdir(parents=True)
        (target / 'debug/deps').symlink_to(external)
        self.assertEqual(guard.main(['clean']), 1)
        self.assertEqual(external.read_text(), 'keep')





if __name__ == '__main__':
    unittest.main()

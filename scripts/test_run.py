import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch, MagicMock
from zipfile import ZipFile, ZipInfo

spec = importlib.util.spec_from_file_location("launcher", Path(__file__).with_name("run.py"))
launcher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(launcher)
resolve_selection = launcher.client_selection


class LauncherTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.revision = 'a' * 40
        self.prefix = f"Open-RotMG-Client-{self.revision}"
        self.url = f'https://codeload.github.com/OpenRotMG/Open-RotMG-Client/zip/{self.revision}'
        self.client = self.root / '.local' / self.prefix
        self.lock = {'version': '0.7.1-openrotmg.7', 'bundleSha256': 'b' * 64}
        self.data = self.archive({"client.swf": b"reference", "index.html": b"page", "scripts/run.py": b"prepare", "run.sh": b"#!/bin/sh\n",
                                  'ruffle/lock.json': json.dumps(self.lock).encode()})
        self.selection = {'client': self.revision, 'client_archive_sha256': hashlib.sha256(self.data).hexdigest(),
                          'runtime_version': self.lock['version'], 'runtime_bundle_sha256': self.lock['bundleSha256']}
        self.patch = patch.multiple(launcher, ROOT=self.root, LOG=self.root / ".local/server.log")
        self.patch.start()
        self.resolve = patch.object(launcher, 'client_selection', return_value=self.selection)
        self.resolve.start()
        self.git = patch.object(launcher.subprocess, 'check_output', side_effect=lambda command, **_: 'test-revision\n' if command[1] == 'rev-parse' else '')
        self.git.start()
        self.port = patch.object(launcher.socket, 'socket')
        self.port.start()

    def archive(self, files):
        stream = io.BytesIO()
        with ZipFile(stream, 'w') as archive:
            for name, content in files.items():
                info = ZipInfo(f'{self.prefix}/{name}')
                info.external_attr = (0o100755 if name == 'run.sh' else 0o100644) << 16
                archive.writestr(info, content)
        return stream.getvalue()

    def tearDown(self):
        self.port.stop()
        self.git.stop()
        self.resolve.stop()
        self.patch.stop()
        self.temp.cleanup()











    def test_failed_build_never_starts_a_stale_retained_binary(self):
        (self.client / 'scripts').mkdir(parents=True)
        (self.client / 'scripts/run.py').touch()
        with patch('sys.argv', ['run.py', '8081']), \
                patch.object(launcher.shutil, 'which', return_value='/cargo'), \
                patch.object(launcher, 'prepare_client', return_value=self.client), \
                patch.object(launcher, 'run', side_effect=[0, 37]) as run:
            self.assertEqual(launcher.main(), 1)
        self.assertEqual(run.call_count, 2)
        self.assertIn('build', run.call_args.args[0])
        self.assertIn('Server build exited with status 37', launcher.LOG.read_text())














if __name__ == '__main__':
    unittest.main()

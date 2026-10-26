import hashlib
import importlib.util
import io
import json
import os
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
        self.patch = patch.multiple(launcher, ROOT=self.root / 'source', LOG=self.root / ".local/server.log")
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
        with patch('sys.argv', ['run.py', '8081', '--client', str(self.client)]), \
                patch.object(launcher.shutil, 'which', return_value='/cargo'), \
                patch.object(launcher, 'prepare_client', return_value=self.client), \
                patch.object(launcher, 'run', side_effect=[0, 37]) as run:
            self.assertEqual(launcher.main(), 1)
        self.assertEqual(run.call_count, 2)
        self.assertIn('build', run.call_args.args[0])
        self.assertIn('Server build exited with status 37', launcher.LOG.read_text())














    def shared_client(self):
        stream = io.BytesIO()
        with ZipFile(stream, 'w') as archive:
            archive.writestr('ruffle.js', b'runtime')
            archive.writestr('runtime.wasm', b'wasm')
        runtime = stream.getvalue()
        self.selection['runtime_bundle_sha256'] = hashlib.sha256(runtime).hexdigest()
        self.lock['bundleSha256'] = self.selection['runtime_bundle_sha256']
        with ZipFile(io.BytesIO(self.data)) as archive:
            files = {name.removeprefix(self.prefix + '/'): archive.read(name) for name in archive.namelist()}
        files['ruffle/lock.json'] = json.dumps(self.lock).encode()
        self.data = self.archive(files)
        self.selection['client_archive_sha256'] = hashlib.sha256(self.data).hexdigest()

        def install(command, client, log):
            (client / f'.ruffle-{self.lock["version"]}.zip').write_bytes(runtime)
            (client / 'vendor').mkdir()
            with ZipFile(io.BytesIO(runtime)) as archive:
                archive.extractall(client / 'vendor')
            return 0
        return install

    def test_deployed_client_is_verified_and_reused_across_run_states(self):
        install = self.shared_client()
        with patch.dict(os.environ, XDG_CACHE_HOME=str(self.root / 'cache')), \
                patch.object(launcher, 'urlopen') as fetch, \
                patch.object(launcher, 'run', side_effect=install) as prepare:
            fetch.return_value.__enter__.return_value.read.return_value = self.data
            first = launcher.prepare_client()
            original = (first / 'vendor/runtime.wasm').stat()
            launcher.LOG = self.root / 'second-state/server.log'
            launcher.LOG.parent.mkdir()
            second = launcher.prepare_client()
            after = (second / 'vendor/runtime.wasm').stat()
        self.assertEqual(first, second)
        self.assertEqual(fetch.call_count, 1)
        self.assertEqual(prepare.call_count, 1)
        self.assertEqual((original.st_ino, original.st_mtime_ns), (after.st_ino, after.st_mtime_ns))
        self.assertEqual(json.loads((launcher.LOG.parent / 'client-selection.json').read_text()), self.selection)
        self.assertFalse((launcher.LOG.parent / self.prefix).exists())

    def test_modified_shared_runtime_fails_instead_of_rebuilding_or_replacing_it(self):
        install = self.shared_client()
        with patch.dict(os.environ, XDG_CACHE_HOME=str(self.root / 'cache')), \
                patch.object(launcher, 'urlopen') as fetch, \
                patch.object(launcher, 'run', side_effect=install) as prepare:
            fetch.return_value.__enter__.return_value.read.return_value = self.data
            client = launcher.prepare_client()
            path = client / 'vendor/runtime.wasm'
            path.write_bytes(b'changed')
            with self.assertRaisesRegex(RuntimeError, 'Shared runtime asset changed'):
                launcher.prepare_client()
            self.assertEqual(path.read_bytes(), b'changed')
            self.assertEqual(prepare.call_count, 1)


if __name__ == '__main__':
    unittest.main()

import hashlib
import json
import os
from pathlib import Path
import sys
import tarfile
import tempfile
import time
import unittest
import zipfile
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from assemble import assemble
from package import TARGETS, archive_payload, unix_bootstrap
from download_manifest import INSTALLERS


class PackagingTests(unittest.TestCase):
    def test_windows_archive_clamps_legacy_timestamps_and_preserves_files(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            payload = root / 'payload'
            files = {
                'bin/compiler.exe': (b'test compiler\x00\xff', 283996800),
                'lib/normal.dll': (b'normal dependency', 1700000000),
            }
            for name, (content, timestamp) in files.items():
                path = payload / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
                os.utime(path, (timestamp, timestamp))
            output = root / 'compiler.zip'
            archive_payload(payload, output, windows=True)
            with zipfile.ZipFile(output) as archive:
                self.assertEqual(archive.namelist(), sorted(files))
                self.assertIsNone(archive.testzip())
                for name, (content, timestamp) in files.items():
                    with self.subTest(name=name):
                        self.assertEqual(archive.read(name), content)
                        info = archive.getinfo(name)
                        self.assertEqual(info.compress_type, zipfile.ZIP_DEFLATED)
                        self.assertGreaterEqual(info.date_time, (1980, 1, 1, 0, 0, 0))
                        if timestamp == 283996800:
                            self.assertEqual(info.date_time, (1980, 1, 1, 0, 0, 0))
                        else:
                            self.assertEqual(info.date_time, time.localtime(timestamp)[:6])
                        self.assertEqual((payload / name).stat().st_mtime, timestamp)

    def test_unix_archive_preserves_legacy_timestamps_and_files(self):
        # macOS and Linux share this TAR packaging path.
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            payload = root / 'payload'
            files = {
                'bin/compiler': (b'test compiler\x00\xff', 283996800),
                'lib/normal.so': (b'normal dependency', 1700000000),
            }
            for name, (content, timestamp) in files.items():
                path = payload / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(content)
                os.utime(path, (timestamp, timestamp))
            output = root / 'compiler.tar.gz'
            archive_payload(payload, output, windows=False)
            with tarfile.open(output, 'r:gz') as archive:
                self.assertEqual(archive.getnames(), sorted(files))
                for name, (content, timestamp) in files.items():
                    with self.subTest(name=name):
                        self.assertEqual(archive.extractfile(name).read(), content)
                        self.assertEqual(archive.getmember(name).mtime, timestamp)

    def test_release_requires_every_target_and_verifies_bytes(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            for target in TARGETS:
                binary = directory / (target + '.zip')
                binary.write_bytes(b'payload')
                manifest = {'schema_version': 1, 'version': '0.1.0', 'platforms': {target: {
                    'url': 'https://example.org/downloads/' + binary.name,
                    'size': 7, 'sha256': hashlib.sha256(b'payload').hexdigest()}}}
                (directory / f'cheeku-0.1.0-{target}.json').write_text(json.dumps(manifest))
            for _, template in INSTALLERS.values():
                (directory / template.format(version='0.1.0')).write_bytes(b'installer')
            result = assemble(directory, '0.1.0', 'https://example.org/downloads')
            self.assertEqual(set(result['platforms']), set(TARGETS))
            (directory / (next(iter(TARGETS)) + '.zip')).write_bytes(b'tampered')
            with self.assertRaises(ValueError):
                assemble(directory, '0.1.0', 'https://example.org/downloads')

    def test_partial_release_cannot_be_published_as_latest(self):
        with tempfile.TemporaryDirectory() as temp:
            with self.assertRaises(ValueError):
                assemble(Path(temp), '0.1.0', 'https://example.org/downloads')
            self.assertFalse((Path(temp) / 'latest.json').exists())

    def test_bootstrap_pins_manager_and_uses_https(self):
        script = unix_bootstrap({'url': 'https://example.org/manager', 'sha256': '1' * 64}, 'https://example.org/release.json')
        self.assertIn("--proto '=https'", script)
        self.assertIn('1' * 64, script)
        self.assertIn('install --manifest', script)
        self.assertIn('"$@"', script)


if __name__ == '__main__':
    unittest.main()

import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from assemble import assemble
from package import TARGETS, unix_bootstrap


class PackagingTests(unittest.TestCase):
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

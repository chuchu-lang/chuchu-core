#!/usr/bin/env python3
"""Merge platform manifests into the download site's latest.json. Never publishes."""
import argparse
import json
from pathlib import Path
from package import TARGETS, digest
from release_artifacts import version_key
from download_manifest import generate
from datetime import datetime, timezone


def assemble(directory, version, base_url, require_all=True, release_date=None):
    version_key(version)
    platforms = {}
    for path in sorted(directory.glob(f'cheeku-{version}-*.json')):
        manifest = json.loads(path.read_text())
        if manifest['schema_version'] != 1 or manifest['version'] != version:
            raise ValueError(f'Mismatched release manifest: {path}')
        for target, entry in manifest['platforms'].items():
            if target not in TARGETS or target in platforms:
                raise ValueError(f'Unknown/duplicate target: {target}')
            from urllib.parse import unquote, urlsplit
            url = entry['url']
            if not url.startswith(base_url.rstrip('/') + '/'):
                raise ValueError(f'Artifact is not hosted under the configured download URL: {url}')
            file = directory / Path(unquote(urlsplit(url).path)).name
            if file.stat().st_size != entry['size'] or digest(file) != entry['sha256']:
                raise ValueError(f'Artifact size/checksum mismatch: {file}')
            platforms[target] = entry
    if require_all and set(platforms) != set(TARGETS):
        raise ValueError(f'Missing release targets: {set(TARGETS) - set(platforms)}')
    if not platforms:
        raise ValueError('No release manifests found')
    if release_date is None:
        previous = directory / f'downloads-{version}.json'
        release_date = (json.loads(previous.read_text())['release_date'] if previous.exists()
                        else datetime.now(timezone.utc).date().isoformat())
    website = generate(directory, version, base_url, release_date)
    result = {'schema_version': 1, 'version': version, 'update_url': base_url.rstrip('/') + '/latest.json', 'platforms': platforms}
    (directory / 'latest.json').write_text(json.dumps(result, indent=2) + '\n')
    # Retain an immutable rollback snapshot and checksum inventory per release.
    (directory / f'cheeku-{version}.json').write_bytes((directory / 'latest.json').read_bytes())
    (directory / 'downloads.json').write_text(json.dumps(website, indent=2) + '\n')
    (directory / f'downloads-{version}.json').write_bytes((directory / 'downloads.json').read_bytes())
    version_sums = directory / f'SHA256SUMS-{version}'
    immutable = [path for path in sorted(directory.iterdir())
                 if path.is_file() and path.name not in {'latest.json', 'downloads.json', 'SHA256SUMS', version_sums.name}]
    version_sums.write_text(''.join(f'{digest(path)}  {path.name}\n' for path in immutable))
    checksums = ''.join(f'{digest(path)}  {path.name}\n' for path in sorted(directory.iterdir()) if path.is_file() and path.name != 'SHA256SUMS')
    (directory / 'SHA256SUMS').write_text(checksums)
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--base-url', default='https://downloads.chuchu-lang.org')
    args = parser.parse_args()
    assemble(args.directory, args.version, args.base_url)

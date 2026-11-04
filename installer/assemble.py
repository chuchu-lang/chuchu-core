#!/usr/bin/env python3
"""Merge platform manifests into the download site's latest.json. Never publishes."""
import argparse
import json
from pathlib import Path
from package import TARGETS, digest


def assemble(directory, version, base_url, require_all=True):
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
    result = {'schema_version': 1, 'version': version, 'update_url': base_url.rstrip('/') + '/latest.json', 'platforms': platforms}
    (directory / 'latest.json').write_text(json.dumps(result, indent=2) + '\n')
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

"""Strict publication checks around the existing schema and native packager."""
import json
import re

from package import TARGETS, digest

BASE_URL = 'https://downloads.chuchu-lang.org'
BUCKET = 'chuchu-installers-prod'
REGION = 'us-east-1'
IMMUTABLE = 'public, max-age=31536000, immutable'
NO_CACHE = 'no-cache, no-store, must-revalidate'


def version_key(version):
    """Canonical SemVer supported by package.py (no build metadata)."""
    match = re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?', version)
    if not match:
        raise ValueError(f'Unsupported release version: {version}')
    major, minor, patch, pre = match.groups()
    identifiers = []
    for value in pre.split('.') if pre else []:
        if value.isdigit() and len(value) > 1 and value.startswith('0'):
            raise ValueError('Numeric prerelease identifiers must not have leading zeros')
        identifiers.append((0, int(value)) if value.isdigit() else (1, value))
    return (int(major), int(minor), int(patch), pre is None, tuple(identifiers))


def validate_tag(tag, compiler_version):
    if not tag.startswith('v'):
        raise ValueError('Release tag must start with v')
    version_key(tag[1:])
    if tag != 'v' + compiler_version:
        raise ValueError('Release tag must match the committed root Cargo.toml version')
    return compiler_version


def expected_files(version):
    version_key(version)
    names = {'latest.json', 'SHA256SUMS', f'cheeku-{version}.json', f'SHA256SUMS-{version}'}
    for target, system in TARGETS.items():
        stem = f'cheeku-{version}-{target}'
        names.update({stem + '.json', stem + ('.zip' if system == 'Windows' else '.tar.gz'),
                      f'cheeku-manager-{version}-{target}' + ('.exe' if system == 'Windows' else '')})
        if system == 'Windows':
            names.add(f'Chuchu-Setup-{version}-windows-x64.exe')
        else:
            names.add(f'install-chuchu-{version}-{target}' + ('.command' if system == 'Darwin' else '.sh'))
        if system == 'Darwin':
            names.add(f'Chuchu-{version}-{target}.pkg')
    return names


def validate_manifest(manifest, version, targets=TARGETS):
    if (set(manifest) != {'schema_version', 'version', 'update_url', 'platforms'}
            or type(manifest['schema_version']) is not int or manifest['schema_version'] != 1
            or manifest['version'] != version or manifest['update_url'] != BASE_URL + '/latest.json'
            or set(manifest['platforms']) != set(targets)):
        raise ValueError('Invalid production manifest schema, version, channel or target set')
    version_key(version)
    for target, entry in manifest['platforms'].items():
        extension = '.zip' if TARGETS[target] == 'Windows' else '.tar.gz'
        if (set(entry) != {'url', 'size', 'sha256'}
                or entry['url'] != f'{BASE_URL}/cheeku-{version}-{target}{extension}'
                or type(entry['size']) is not int or not 0 < entry['size'] <= 1024 ** 3
                or not re.fullmatch('[0-9a-f]{64}', entry['sha256'])):
            raise ValueError(f'Invalid production artifact metadata for {target}')


def check_sums(path, directory, expected):
    entries = {}
    for line in path.read_text().splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  ([A-Za-z0-9_.-]+)', line)
        if not match or match[2] in entries:
            raise ValueError(f'Unsafe or duplicate checksum entry in {path.name}')
        entries[match[2]] = match[1]
    if set(entries) != expected:
        raise ValueError(f'Incomplete checksum inventory in {path.name}')
    for name, checksum in entries.items():
        if digest(directory / name) != checksum:
            raise ValueError(f'Checksum mismatch: {name}')


def validate_release(directory, version):
    names = expected_files(version)
    if set(p.name for p in directory.iterdir()) != names:
        raise ValueError('Release contains missing or unexpected files')
    for name in names:
        path = directory / name
        if path.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= 1024 ** 3:
            raise ValueError(f'Unsafe, empty or oversized artifact: {name}')
    manifest = json.loads((directory / 'latest.json').read_text())
    validate_manifest(manifest, version)
    if (directory / f'cheeku-{version}.json').read_bytes() != (directory / 'latest.json').read_bytes():
        raise ValueError('Versioned manifest snapshot differs from latest.json')
    for target, entry in manifest['platforms'].items():
        path = directory / entry['url'].rsplit('/', 1)[1]
        if digest(path) != entry['sha256'] or path.stat().st_size != entry['size']:
            raise ValueError(f'Payload checksum/size mismatch: {target}')
        native = json.loads((directory / f'cheeku-{version}-{target}.json').read_text())
        validate_manifest(native, version, targets=[target])
        if native != dict(manifest, platforms={target: entry}):
            raise ValueError(f'Platform manifest disagrees with latest.json: {target}')
    version_sums = f'SHA256SUMS-{version}'
    check_sums(directory / version_sums, directory, names - {'latest.json', 'SHA256SUMS', version_sums})
    check_sums(directory / 'SHA256SUMS', directory, names - {'SHA256SUMS'})
    return manifest


def content_type(name):
    if name.endswith('.json'):
        return 'application/json'
    if name.startswith('SHA256SUMS'):
        return 'text/plain'
    if name.endswith(('.sh', '.command')):
        return 'text/x-shellscript'
    if name.endswith('.zip'):
        return 'application/zip'
    if name.endswith('.tar.gz'):
        return 'application/gzip'
    return 'application/octet-stream'

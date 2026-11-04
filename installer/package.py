#!/usr/bin/env python3
"""Build relocatable payloads and self-hosted installer downloads on their native host."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import urllib.parse
import zipfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {'aarch64-apple-darwin': 'Darwin', 'x86_64-apple-darwin': 'Darwin',
           'x86_64-unknown-linux-gnu': 'Linux', 'x86_64-pc-windows-msvc': 'Windows'}


def run(*args):
    return subprocess.check_output([str(a) for a in args], text=True).strip()


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def dylibs(path):
    return [line.strip().split(' (compatibility')[0] for line in run('otool', '-L', path).splitlines()[1:]]


def bundle_macos(binary, lib, llvm_prefix):
    """Copy the non-system dependency closure and rewrite every load command."""
    queue = [(binary, binary)]
    copied = {}
    while queue:
        original, staged = queue.pop(0)
        changes = []
        for dependency in dylibs(original):
            if dependency.startswith(('/usr/lib/', '/System/Library/')):
                continue
            if dependency.startswith('@rpath/'):
                candidate = llvm_prefix / 'lib' / dependency.removeprefix('@rpath/')
            elif dependency.startswith('@loader_path/'):
                candidate = original.parent / dependency.removeprefix('@loader_path/')
            else:
                candidate = Path(dependency)
            if candidate.resolve() == original.resolve() and staged != binary:
                continue  # dylib's own install id
            if not candidate.is_file():
                raise RuntimeError(f'Cannot resolve {dependency} used by {original}')
            name = candidate.name
            resolved = candidate.resolve()
            if name in copied and copied[name] != resolved:
                raise RuntimeError(f'Conflicting library basenames: {name}')
            if name not in copied:
                copied[name] = resolved
                destination = lib / name
                shutil.copy2(resolved, destination)
                destination.chmod(0o755)
                queue.append((resolved, destination))
            relative = '@loader_path/' + ('../lib/' if staged == binary else '') + name
            changes.extend(['-change', dependency, relative])
        if staged != binary:
            changes.extend(['-id', '@rpath/' + staged.name])
        if changes:
            run('install_name_tool', *changes, staged)
    # Remove build-machine RPATHs; all non-system references are now relative.
    for staged in [*lib.iterdir(), binary]:
        lines = run('otool', '-l', staged).splitlines()
        for i, line in enumerate(lines):
            if line.strip() == 'cmd LC_RPATH':
                path = lines[i + 2].strip().removeprefix('path ').split(' (offset')[0]
                if path.startswith('/'):
                    run('install_name_tool', '-delete_rpath', path, staged)
        run('codesign', '--force', '--sign', '-', staged)


def bundle_linux(binary, lib):
    system = ('libc.so.', 'libm.so.', 'libpthread.so.', 'libdl.so.', 'librt.so.', 'libgcc_s.so.', 'libstdc++.so.', 'ld-linux')
    queue = [binary]
    copied = {}
    while queue:
        original = queue.pop(0)
        dependencies = run('ldd', original)
        if 'not found' in dependencies:
            raise RuntimeError(f'Unresolved shared library in {original}: {dependencies}')
        for line in dependencies.splitlines():
            match = re.search(r'=> (/\S+) \(', line)
            if not match:
                continue
            candidate = Path(match[1])
            name = candidate.name
            if name.startswith(system):
                continue
            resolved = candidate.resolve()
            if name in copied:
                if copied[name] != resolved:
                    raise RuntimeError(f'Conflicting library basenames: {name}')
                continue
            copied[name] = resolved
            destination = lib / name
            shutil.copy2(resolved, destination)
            destination.chmod(0o755)
            queue.append(resolved)
    for path in lib.iterdir():
        run('patchelf', '--set-rpath', '$ORIGIN', path)
    run('patchelf', '--set-rpath', '$ORIGIN/../lib', binary)


def archive_payload(payload, output, windows):
    paths = sorted(p for p in payload.rglob('*') if p.is_file())
    if windows:
        with zipfile.ZipFile(output, 'w', zipfile.ZIP_DEFLATED) as archive:
            for path in paths:
                archive.write(path, path.relative_to(payload).as_posix())
    else:
        with tarfile.open(output, 'w:gz') as archive:
            for path in paths:
                archive.add(path, arcname=path.relative_to(payload).as_posix(), recursive=False)


def asset(path, base_url):
    return {'url': base_url.rstrip('/') + '/' + urllib.parse.quote(path.name), 'sha256': digest(path), 'size': path.stat().st_size}


def unix_bootstrap(manager_asset, manifest_url):
    # Both values are generated by this packager, never interpolated from a remote manifest.
    import shlex
    url = shlex.quote(manager_asset['url'])
    manifest = shlex.quote(manifest_url)
    checksum = manager_asset['sha256']
    return f'''#!/bin/sh
set -eu
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
curl --fail --location --proto '=https' --tlsv1.2 {url} -o "$work/cheeku-manager"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$work/cheeku-manager" | cut -d ' ' -f 1)
else
  actual=$(shasum -a 256 "$work/cheeku-manager" | cut -d ' ' -f 1)
fi
[ "$actual" = '{checksum}' ] || {{ echo 'Installer checksum mismatch' >&2; exit 1; }}
chmod +x "$work/cheeku-manager"
"$work/cheeku-manager" install --manifest {manifest} "$@"
'''


def collect_manager_notices(destination):
    metadata = json.loads(run('cargo', 'metadata', '--locked', '--format-version', '1',
                              '--manifest-path', ROOT / 'installer/manager/Cargo.toml'))
    for package in metadata['packages']:
        if package['source'] is None:
            continue
        source = Path(package['manifest_path']).parent
        folder = destination / 'manager' / (package['name'] + '-' + package['version'])
        folder.mkdir(parents=True)
        (folder / 'PACKAGE.txt').write_text(package['name'] + ' ' + package['version'] + '\nLicense: ' + str(package.get('license')) + '\nSource: ' + str(package.get('repository')) + '\n')
        for notice in source.iterdir():
            if notice.is_file() and notice.name.upper().startswith(('LICENSE', 'COPYING', 'NOTICE', 'COPYRIGHT')):
                shutil.copy2(notice, folder / notice.name)


def package(target, base_url, binary, manager, llvm_prefix, output, notices_dir=None):
    host = run('rustc', '-vV').split('host: ')[1].splitlines()[0]
    if target != host or TARGETS.get(target) != platform.system():
        raise RuntimeError(f'Cannot package {host}/{platform.system()} as {target}')
    if not base_url.startswith('https://') or urllib.parse.urlsplit(base_url).query or urllib.parse.urlsplit(base_url).fragment:
        raise ValueError('Download base URL must use HTTPS and have no query or fragment')
    if run(manager, '--manager-update-url') != base_url.rstrip('/') + '/latest.json':
        raise RuntimeError('Build the manager with CHEEKU_UPDATE_URL matching --base-url/latest.json')
    version = run(binary, '--version').removeprefix('cheeku ')
    if not re.fullmatch(r'\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?', version):
        raise RuntimeError(f'Compiler reported an invalid version: {version}')
    windows = target.endswith('windows-msvc')
    output.mkdir(parents=True, exist_ok=True)
    stem = f'cheeku-{version}-{target}'
    with tempfile.TemporaryDirectory() as temp:
        payload = Path(temp) / 'payload'
        (payload / 'bin').mkdir(parents=True)
        (payload / 'lib').mkdir()
        compiler = payload / 'bin' / ('cheeku.exe' if windows else 'cheeku')
        shutil.copy2(binary, compiler)
        shutil.copy2(ROOT / 'LICENSE', payload / 'LICENSE')
        shutil.copytree(ROOT / 'examples', payload / 'examples')
        licenses = payload / 'licenses'
        licenses.mkdir()
        # Production SDKs are built without optional third-party libraries on macOS/Windows.
        # Require packagers to provide notices for every redistributed dependency.
        notices = notices_dir or llvm_prefix / 'share' / 'chuchu-licenses'
        if not notices.is_dir() or not any(notices.iterdir()):
            raise RuntimeError(f'Missing redistribution notices: populate {notices} with LLVM and bundled dependency licenses')
        shutil.copytree(notices, licenses, dirs_exist_ok=True)
        collect_manager_notices(licenses)
        if platform.system() == 'Darwin':
            bundle_macos(compiler, payload / 'lib', llvm_prefix)
        elif platform.system() == 'Linux':
            bundle_linux(compiler, payload / 'lib')
        else:
            tools = payload / 'tools'
            tools.mkdir()
            shutil.copy2(llvm_prefix / 'bin/clang-cl.exe', tools / 'clang-cl.exe')
            for dll in (llvm_prefix / 'bin').glob('*.dll'):
                shutil.copy2(dll, tools / dll.name)
            shutil.copytree(llvm_prefix / 'lib/clang', payload / 'lib/clang')
        archive = output / (stem + ('.zip' if windows else '.tar.gz'))
        archive_payload(payload, archive, windows)
    manager_name = f'cheeku-manager-{version}-{target}' + ('.exe' if windows else '')
    manager_path = output / manager_name
    shutil.copy2(manager, manager_path)
    entry = {'schema_version': 1, 'version': version, 'update_url': base_url.rstrip('/') + '/latest.json', 'platforms': {target: asset(archive, base_url)}}
    manifest = output / f'{stem}.json'
    manifest.write_text(json.dumps(entry, indent=2) + '\n')
    if windows:
        setup = output / f'Chuchu-Setup-{version}-windows-x64.exe'
        shutil.copy2(manager, setup)
    else:
        setup = output / (f'install-chuchu-{version}-{target}' + ('.command' if platform.system() == 'Darwin' else '.sh'))
        setup.write_text(unix_bootstrap(asset(manager_path, base_url), asset(manifest, base_url)['url']))
        setup.chmod(0o755)
    if platform.system() == 'Darwin':
        with tempfile.TemporaryDirectory() as temp:
            stage = Path(temp) / 'root'
            scripts = Path(temp) / 'scripts'
            location = stage / 'usr/local/libexec/chuchu-installer'
            location.mkdir(parents=True)
            scripts.mkdir()
            shutil.copy2(manager, location / 'cheeku-manager')
            import shlex
            postinstall = scripts / 'postinstall'
            postinstall.write_text('''#!/bin/sh
set -eu
user=$(stat -f '%Su' /dev/console)
if [ "$user" = root ] || [ "$user" = loginwindow ]; then
  echo 'Run this installer from a logged-in macOS user session.' >&2
  exit 1
fi
uid=$(id -u "$user")
launchctl asuser "$uid" sudo -H -u "$user" /usr/local/libexec/chuchu-installer/cheeku-manager install --manifest ''' + shlex.quote(asset(manifest, base_url)['url']) + '\n')
            postinstall.chmod(0o755)
            setup = output / f'Chuchu-{version}-{target}.pkg'
            run('pkgbuild', '--root', stage, '--scripts', scripts, '--identifier', 'org.chuchu-lang.compiler', '--version', version, setup)
    print(f'Created {setup}\nPayload: {archive}\nManifest: {manifest}')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True, choices=TARGETS)
    parser.add_argument('--base-url', default='https://downloads.chuchu-lang.org')
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--manager', type=Path)
    parser.add_argument('--llvm-prefix', type=Path, default=Path(os.environ.get('LLVM_SYS_231_PREFIX', '/nonexistent')))
    parser.add_argument('--licenses-dir', type=Path, help='Redistribution notices for LLVM and all bundled dependencies')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/distributions')
    args = parser.parse_args()
    suffix = '.exe' if os.name == 'nt' else ''
    target_dir = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
    binary = args.binary or target_dir / 'release' / ('cheeku' + suffix)
    manager = args.manager or ROOT / 'installer/manager/target/release' / ('cheeku-manager' + suffix)
    package(args.target, args.base_url, binary, manager, args.llvm_prefix, args.output, args.licenses_dir)


if __name__ == '__main__':
    main()

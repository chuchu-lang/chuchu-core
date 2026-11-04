#!/usr/bin/env python3
"""Exercise native relocation, install, launch, explicit update and failed-update recovery."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import zipfile
from package import archive_payload


def command(*args, ok=True, **kwargs):
    result = subprocess.run([str(a) for a in args], capture_output=True, text=True, timeout=180, **kwargs)
    if ok and result.returncode:
        raise RuntimeError(f'{args}: {result.stdout}\n{result.stderr}')
    return result


def smoke(manager, directory):
    manager = manager.resolve()
    manifests = list(directory.glob('cheeku-*-*.json'))
    if len(manifests) != 1:
        raise RuntimeError(f'Expected one native manifest, got {manifests}')
    original = json.loads(manifests[0].read_text())
    target, asset = next(iter(original['platforms'].items()))
    archive = directory / asset['url'].rsplit('/', 1)[1]
    with tempfile.TemporaryDirectory(prefix='Chuchu installer smoke ') as tmp:
        tmp = Path(tmp)
        env = os.environ.copy()
        # Clear build-time environment so relocation is actually exercised.
        for key in ['LLVM_SYS_231_PREFIX', 'DYLD_LIBRARY_PATH', 'LD_LIBRARY_PATH', 'CHEEKU_LINKER']:
            env.pop(key, None)
        prefix = tmp / 'installation with spaces'
        source = tmp / 'release.json'
        first = copy.deepcopy(original)
        first['update_url'] = str(source)
        first['platforms'][target]['url'] = str(archive.resolve())
        source.write_text(json.dumps(first))
        command(manager, 'install', '--prefix', prefix, '--manifest', source,
                '--no-path', '--skip-dependencies', env=env)
        launcher = prefix / 'bin' / ('cheeku.exe' if os.name == 'nt' else 'cheeku')
        assert command(launcher, '--version', env=env).stdout.strip() == 'cheeku ' + first['version']
        program = tmp / 'hello.chuchu'
        program.write_text('assign greeting as "Hello, Chuchu!"\nshow greeting\n')
        executable = tmp / ('hello.exe' if os.name == 'nt' else 'hello')
        command(launcher, program, '-o', executable, env=env)
        assert command(executable).stdout.strip() == 'Hello, Chuchu!'
        assert 'no newer version' in command(launcher, 'update', '--check', env=env).stdout
        command(manager, 'install', '--prefix', prefix, '--manifest', source,
                '--no-path', '--skip-dependencies', env=env)
        assert command(launcher, '--version', env=env).stdout.strip() == 'cheeku ' + first['version']
        # Create a test-only newer release: a native shim reports a new version
        # and delegates compilation to the same relocated engine. Never distributed.
        payload = tmp / 'next-payload'
        payload.mkdir()
        if os.name == 'nt':
            with zipfile.ZipFile(archive) as data:
                data.extractall(payload)
        else:
            with tarfile.open(archive) as data:
                data.extractall(payload, filter='data')
        next_version = first['version'].split('-')[0].split('.')
        next_version[-1] = str(int(next_version[-1]) + 1)
        next_version = '.'.join(next_version)
        compiler = payload / 'bin' / ('cheeku.exe' if os.name == 'nt' else 'cheeku')
        real_name = 'cheeku-real.exe' if os.name == 'nt' else 'cheeku-real'
        compiler.rename(compiler.with_name(real_name))
        shim = tmp / 'version-fixture.rs'
        shim.write_text('''fn main() {
let args: Vec<_> = std::env::args_os().skip(1).collect();
if args == [std::ffi::OsString::from("--version")] { println!("cheeku VERSION"); return; }
let exe = std::env::current_exe().unwrap().with_file_name("REAL_NAME");
let status = std::process::Command::new(exe).args(args).status().unwrap();
std::process::exit(status.code().unwrap_or(1));
}'''.replace('VERSION', next_version).replace('REAL_NAME', real_name))
        command('rustc', shim, '-o', compiler)
        new_archive = tmp / ('next.zip' if os.name == 'nt' else 'next.tar.gz')
        archive_payload(payload, new_archive, os.name == 'nt')
        newer = copy.deepcopy(first)
        newer['version'] = next_version
        newer['platforms'][target] = {'url': str(new_archive), 'size': new_archive.stat().st_size,
                                     'sha256': hashlib.sha256(new_archive.read_bytes()).hexdigest()}
        source.write_text(json.dumps(newer))
        assert next_version in command(launcher, 'update', '--check', env=env).stdout
        assert command(launcher, '--version', env=env).stdout.strip() == 'cheeku ' + first['version']
        command(launcher, 'update', env=env)
        assert command(launcher, '--version', env=env).stdout.strip() == 'cheeku ' + next_version
        assert (prefix / 'versions' / first['version']).is_dir()
        command(launcher, program, '-o', executable, env=env)
        assert command(executable).stdout.strip() == 'Hello, Chuchu!'
        # Bad update must fail without changing current.json or damaging the active compiler.
        state = (prefix / 'current.json').read_bytes()
        broken = copy.deepcopy(newer)
        broken['version'] = '999.0.0'
        broken['platforms'][target]['sha256'] = '0' * 64
        source.write_text(json.dumps(broken))
        assert command(launcher, 'update', env=env, ok=False).returncode != 0
        assert (prefix / 'current.json').read_bytes() == state
        command(launcher, program, '-o', executable, env=env)
        # A valid checksum with an incompatible compiler must also preserve the active release.
        broken['platforms'][target]['sha256'] = newer['platforms'][target]['sha256']
        source.write_text(json.dumps(broken))
        assert command(launcher, 'update', env=env, ok=False).returncode != 0
        assert (prefix / 'current.json').read_bytes() == state
        assert not (prefix / 'versions/999.0.0').exists()
        print('PASS: relocated compiler, install paths with spaces, launch, update check, successful update, checksum rejection and failed activation recovery')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--manager', type=Path, required=True)
    parser.add_argument('--directory', type=Path, required=True)
    args = parser.parse_args()
    smoke(args.manager, args.directory)

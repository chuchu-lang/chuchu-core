#!/usr/bin/env python3
"""Provision the full, checksum-pinned LLVM 23.1.4 SDK (Python 3.12+).

The official Windows installer is not sufficient: llvm-sys needs llvm-config,
headers and static libraries. Intel macOS has no SDK asset in this release,
so build the same release's verified source with the X86 native backend.
The macOS ARM64 SDK is also unusable: it ships no libLLVM dylib and its static
archives are LTO bitcode that Apple's linker cannot read, so build it from source
as a native-object libLLVM dylib with the AArch64 backend.
Windows also rebuilds LLVM: the upstream SDK embeds nonrelocatable system
library paths (including its release builder's S: drive). Keep its clang-cl
driver, but install a native static LLVM build without optional compression/XML.
"""
import argparse
import hashlib
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import urllib.request
import urllib.parse

VERSION = '23.1.4'
SOURCE = ('llvm-project-23.1.4.src.tar.xz', '819ecf7351f1a66f76b2d701d8942faaf5faeb324cb90c9d5b090de142434bb2')
ASSETS = {
    ('Darwin', 'arm64'): SOURCE,
    ('Linux', 'x86_64'): ('LLVM-23.1.4-Linux-X64.tar.xz', '25d814fda50d5fb0084b42be09605d8b1595d49d5e2e03a3345d7e112ff6f9f3'),
    ('Windows', 'amd64'): ('clang+llvm-23.1.4-x86_64-pc-windows-msvc.tar.xz', '854bffcfc84528e0ee92490ff5c178920abd358a79c3e98f053e41e3e8e4c6d2'),
    ('Darwin', 'x86_64'): SOURCE,
}
# Hosts that build LLVM from the pinned source, with the one backend each needs.
SOURCE_BUILDS = {('Darwin', 'x86_64'): 'X86', ('Darwin', 'arm64'): 'AArch64', ('Windows', 'amd64'): 'X86'}


def run(args):
    subprocess.run([str(x) for x in args], check=True)


def download_archive(name, digest, directory):
    archive = directory / name
    url = f'https://github.com/llvm/llvm-project/releases/download/llvmorg-{VERSION}/{urllib.parse.quote(name)}'
    print(f'Downloading {url}', flush=True)
    urllib.request.urlretrieve(url, archive)
    with archive.open('rb') as stream:
        actual = hashlib.file_digest(stream, 'sha256').hexdigest()
    if actual != digest:
        archive.unlink()
        raise RuntimeError(f'Checksum mismatch for {name}: {actual}')
    with tarfile.open(archive, 'r:xz') as stream:
        stream.extractall(directory, filter='data')
    archive.unlink()


def activate(prefix):
    config = prefix / 'bin' / ('llvm-config.exe' if os.name == 'nt' else 'llvm-config')
    version = subprocess.check_output([config, '--version'], text=True).strip()
    if version != VERSION:
        raise RuntimeError(f'Expected LLVM {VERSION}, found {version}')
    if os.name == 'nt':
        libs = subprocess.check_output([config, '--libnames', '--link-static'], text=True).split()
        if not libs or not all((prefix / 'lib' / name).is_file() for name in libs):
            raise RuntimeError('Windows SDK is missing required LLVM static libraries')
        if not (prefix / 'bin/clang-cl.exe').is_file():
            raise RuntimeError('Windows SDK is missing clang-cl.exe')
    if platform.system() == 'Darwin':
        # The shared library is found through an rpath that build.rs embeds in
        # cheeku. Never export it through DYLD_LIBRARY_PATH: rustc itself loads
        # @rpath/libLLVM.dylib, so a global path swaps in an incompatible LLVM.
        if not (prefix / 'lib/libLLVM.dylib').exists():
            raise RuntimeError('macOS SDK is missing libLLVM.dylib; llvm-sys would link bitcode archives')
        subprocess.check_output([config, '--link-shared', '--libnames'], text=True)
    print(f'LLVM_SYS_231_PREFIX={prefix}', flush=True)
    if os.environ.get('GITHUB_ENV'):
        with open(os.environ['GITHUB_ENV'], 'a') as env:
            env.write(f'LLVM_SYS_231_PREFIX={prefix}\n')
            if platform.system() == 'Linux':
                env.write(f'LD_LIBRARY_PATH={prefix / "lib"}:{os.environ.get("LD_LIBRARY_PATH", "")}\n')
        with open(os.environ['GITHUB_PATH'], 'a') as paths:
            paths.write(f'{prefix / "bin"}\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, required=True)
    args = parser.parse_args()
    destination = args.directory.resolve()
    destination.mkdir(parents=True, exist_ok=True)
    marker = destination / 'prefix.txt'
    if marker.exists():
        activate(Path(marker.read_text().strip()))
        return
    system, machine = platform.system(), platform.machine().lower()
    key = (system, machine)
    if key not in ASSETS:
        raise RuntimeError(f'No pinned SDK for {key}; build LLVM 23.1.4 for this host')
    name, digest = ASSETS[key]
    download_archive(name, digest, destination)
    if key in SOURCE_BUILDS and system == 'Darwin':
        prefix = destination / 'install'
    else:
        candidates = list(destination.glob('*/bin/llvm-config*'))
        config = next((p for p in candidates if p.name in ('llvm-config', 'llvm-config.exe')), None)
        if config is None:
            raise RuntimeError('Archive has no bin/llvm-config; a full development SDK is required')
        prefix = config.parent.parent
    if key in SOURCE_BUILDS:
        if system == 'Windows':
            download_archive(*SOURCE, destination)
        source = destination / f'llvm-project-{VERSION}.src/llvm'
        build = destination / 'llvm-build'
        options = ['cmake', '-S', source, '-B', build, '-G', 'Ninja',
                   '-DCMAKE_BUILD_TYPE=Release', f'-DCMAKE_INSTALL_PREFIX={prefix}',
                   f'-DLLVM_TARGETS_TO_BUILD={SOURCE_BUILDS[key]}', '-DLLVM_INCLUDE_TESTS=OFF',
                   '-DLLVM_INCLUDE_BENCHMARKS=OFF', '-DLLVM_INCLUDE_EXAMPLES=OFF',
                   '-DLLVM_ENABLE_BINDINGS=OFF', '-DLLVM_ENABLE_ZSTD=OFF',
                   '-DLLVM_ENABLE_ZLIB=OFF', '-DLLVM_ENABLE_LIBXML2=OFF']
        if system == 'Windows':
            # llvm-sys requires static LLVM on MSVC. Use the same DLL CRT as Rust.
            options += ['-DLLVM_BUILD_LLVM_DYLIB=OFF', '-DLLVM_LINK_LLVM_DYLIB=OFF',
                        '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL']
        else:
            options += ['-DLLVM_BUILD_LLVM_DYLIB=ON', '-DLLVM_LINK_LLVM_DYLIB=ON']
        run(options)
        run(['cmake', '--build', build, '--target', 'install', '--parallel', '3'])
        notices = prefix / 'share/chuchu-licenses'
        notices.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source.parent / 'LICENSE.TXT', notices / 'LLVM-LICENSE.txt')
        shutil.rmtree(build)
        shutil.rmtree(source.parent)
    activate(prefix)
    marker.write_text(str(prefix))


if __name__ == '__main__':
    main()

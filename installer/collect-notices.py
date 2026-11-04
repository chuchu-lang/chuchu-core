#!/usr/bin/env python3
"""Collect vendor-provided notices from the verified SDK/source and Linux packages."""
import argparse
import hashlib
import urllib.request
import os
from pathlib import Path
import shutil

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--prefix', type=Path, required=True)
args = parser.parse_args()
output = args.prefix / 'share/chuchu-licenses'
output.mkdir(parents=True, exist_ok=True)
# source builds preserve LICENSE.TXT before deleting the source tree.
for path in args.prefix.rglob('*'):
    if output in path.parents or not path.is_file():
        continue
    if path.name.lower() in ('license.txt', 'license', 'copyright', 'notice', 'notice.txt'):
        name = path.relative_to(args.prefix).as_posix().replace('/', '__')
        shutil.copy2(path, output / name)
if os.name != 'nt' and Path('/usr/share/doc').is_dir():
    for path in Path('/usr/share/doc').glob('*/copyright'):
        if path.is_file():
            shutil.copy2(path, output / ('system-' + path.parent.name + '-copyright'))
# Older cached source SDKs may predate notice preservation. Fetch only this
# fixed release's license and verify its pinned digest, never an arbitrary URL.
llvm_license = output / 'LLVM-LICENSE.txt'
if not llvm_license.exists():
    with urllib.request.urlopen('https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-23.1.4/LICENSE.TXT', timeout=30) as response:
        data = response.read(1024 * 1024)
    if hashlib.sha256(data).hexdigest() != '8d85c1057d742e597985c7d4e6320b015a9139385cff4cbae06ffc0ebe89afee':
        raise RuntimeError('Pinned LLVM license checksum mismatch')
    llvm_license.write_bytes(data)
if not any('llvm' in path.name.lower() or 'license' in path.name.lower() for path in output.iterdir()):
    raise RuntimeError('No LLVM license found in the SDK; do not redistribute without its notices')

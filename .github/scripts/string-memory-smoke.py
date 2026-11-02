#!/usr/bin/env python3
"""Check string allocation balance, bounded live allocations and allocation errors.

Uses a C allocator shim around emitted O0 IR; --sanitize also checks lifetimes
and buffer accesses with AddressSanitizer. Requires a compatible Clang driver.
"""
import argparse
import os
import re
from pathlib import Path
import subprocess
import tempfile


SOURCE = '''function make(value: string, n: int) be string {
    assign local as value + "!"
    if n == 0 { return local }
    while n > 0 {
        assign nested as "scope"
        if n == 1 { return local + nested }
        return make(local, n - 1)
    }
    return "fallback"
}
function consume(value: string) be int { return length(value) }
assign original as "hello"
assign copy as original
reassign original as original + original
show copy == "hello"
show consume(original)
assign i as 0
while i < 2000 {
    assign temporary as make("value", 3)
    reassign temporary as temporary + "!"
    assign size as length(temporary)
    reassign i as i + 1
}
show i
'''

SHIM = r'''
#include <stdio.h>
#include <stdlib.h>
static size_t live, peak, calls;
static unsigned long fail_at;
static void check_balance(void) {
    if (!fail_at && (live != 0 || peak > 64 || calls < 1000)) {
        fprintf(stderr, "allocation failure: live=%zu peak=%zu calls=%zu\n", live, peak, calls);
        abort();
    }
}
void *cheeku_test_malloc(size_t size) {
    if (!calls) {
        const char *value = getenv("CHEEKU_TEST_FAIL_ALLOCATION");
        fail_at = value ? strtoul(value, NULL, 10) : 0;
        if (atexit(check_balance) != 0) abort();
    }
    if (++calls == fail_at) return NULL;
    void *value = malloc(size);
    if (value) { ++live; if (live > peak) peak = live; }
    return value;
}
void cheeku_test_free(void *value) {
    if (!live || !value) abort();
    --live;
    free(value);
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', required=True, type=Path)
    prefix = os.environ.get('LLVM_SYS_231_PREFIX')
    configured = Path(prefix) / 'bin' / ('clang.exe' if os.name == 'nt' else 'clang') if prefix else None
    parser.add_argument('--clang', default=str(configured) if configured and configured.is_file() else 'clang')
    parser.add_argument('--sanitize', action='store_true')
    args = parser.parse_args()
    compiler = args.compiler.resolve(strict=True)
    version = subprocess.check_output([args.clang, '--version'], text=True)
    match = re.search(r'clang version (\d+)', version)
    if not match or int(match.group(1)) < 23:
        parser.error('The emitted IR requires Clang 23 or newer; supply --clang /path/to/llvm/bin/clang')
    with tempfile.TemporaryDirectory(prefix='cheeku string memory ') as temporary:
        root = Path(temporary)
        source, ir, shim = root / 'strings.chuchu', root / 'strings.ll', root / 'shim.c'
        executable = root / ('strings.exe' if os.name == 'nt' else 'strings')
        source.write_text(SOURCE, encoding='utf-8')
        subprocess.run([str(compiler), str(source), '-O0', '--emit-ir', str(ir), '-o', str(executable)], check=True, timeout=60)
        # Rewrite only compiler-emitted allocator calls/declarations. The shim's
        # own malloc/free calls and CRT allocations still use the real allocator.
        text = ir.read_text(encoding='utf-8')
        assert '@malloc(' in text and '@free(' in text
        ir.write_text(text.replace('@malloc(', '@cheeku_test_malloc(').replace('@free(', '@cheeku_test_free('), encoding='utf-8')
        shim.write_text(SHIM, encoding='utf-8')
        command = [args.clang, '-O0', '-Wno-override-module', str(ir), str(shim), '-o', str(executable)]
        if args.sanitize:
            command.append('-fsanitize=address')
        subprocess.run(command, check=True, timeout=60)
        environment = os.environ.copy()
        # Successful runs use explicit allocation accounting, including on macOS
        # where LeakSanitizer is unavailable. Error exits may leave live values.
        environment['ASAN_OPTIONS'] = 'detect_leaks=0'
        environment.pop('CHEEKU_TEST_FAIL_ALLOCATION', None)
        result = subprocess.run([str(executable)], capture_output=True, text=True, env=environment, timeout=60)
        assert result.returncode == 0, result.stderr
        assert result.stdout == 'true\n10\n2000\n', result.stdout
        assert not result.stderr, result.stderr
        for failure in (1, 5, 50):
            environment['CHEEKU_TEST_FAIL_ALLOCATION'] = str(failure)
            result = subprocess.run([str(executable)], capture_output=True, text=True, env=environment, timeout=60)
            assert result.returncode == 1, result.stderr
            assert 'string allocation failed' in result.stderr, result.stderr
            assert 'AddressSanitizer' not in result.stderr, result.stderr
        print('PASS: balanced string allocations, bounded live storage, and 3 allocation-failure paths' + (' under AddressSanitizer' if args.sanitize else ''))


if __name__ == '__main__':
    main()

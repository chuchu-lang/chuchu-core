#!/usr/bin/env python3
"""Check result/string ownership, native input buffers and recoverable OOM.

Recompile emitted O0 IR and the embedded C runtime with a tracked allocator.
Clang 23+ is required; --sanitize enables AddressSanitizer as well.
"""
import argparse
import os
from pathlib import Path
import re
import subprocess
import tempfile

SOURCE = '''function relay(value: result<string>) be result<string> {
    assign local as value
    if is_ok(local) { return local }
    return err<string>(8)
}
assign first as try_input()
show error_code(first)
if is_ok(first) { show length(unwrap(first)) } else { show "recovered" }
show unwrap(try_input())
assign i as 0
while i < 1000 {
    assign line as relay(try_input())
    assign copy as line
    reassign line as err<string>(8)
    assign parsed as parse_float(unwrap(copy))
    assign text as to_string(unwrap(parsed))
    assign saved as ok(text)
    reassign saved as err<string>(9)
    reassign i as i + 1
}
show is_eof(try_input())
'''

SHIM = r'''
#include <stdio.h>
#include <stdlib.h>
static size_t live, peak, calls, runtime_calls, fail_at;
static int initialized;
static void check(void) {
    if (live || peak > 64 || calls < 1000) {
        fprintf(stderr, "allocation imbalance live=%zu peak=%zu calls=%zu\n", live, peak, calls);
        abort();
    }
}
static void initialize(void) {
    if (!initialized) {
        initialized = 1;
        const char *failure = getenv("CHEEKU_TEST_FAIL_RUNTIME");
        fail_at = failure ? (size_t)strtoul(failure, NULL, 10) : 0;
        if (atexit(check)) abort();
    }
}
void *cheeku_test_malloc(size_t bytes) {
    initialize(); ++calls;
    void *value = malloc(bytes);
    if (value) { ++live; if (live > peak) peak = live; }
    return value;
}
void cheeku_test_free(void *value) {
    if (!value) return;
    if (!live) abort();
    --live; free(value);
}
void *cheeku_test_runtime_malloc(size_t bytes) {
    initialize();
    if (++runtime_calls == fail_at) return NULL;
    return cheeku_test_malloc(bytes);
}
void *cheeku_test_realloc(void *value, size_t bytes) {
    initialize(); ++calls;
    if (++runtime_calls == fail_at) return NULL;
    int was_null = value == NULL;
    void *grown = realloc(value, bytes);
    if (grown && was_null) { ++live; if (live > peak) peak = live; }
    return grown;
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--compiler', required=True, type=Path)
    parser.add_argument('--clang', default='clang')
    parser.add_argument('--sanitize', action='store_true')
    args = parser.parse_args()
    version = subprocess.check_output([args.clang, '--version'], text=True)
    match = re.search(r'clang version (\d+)', version)
    if not match or int(match.group(1)) < 23:
        parser.error('Use Clang 23+ with --clang /path/to/llvm/bin/clang')
    compiler = args.compiler.resolve(strict=True)
    repository = Path(__file__).resolve().parents[2]
    # Read the shared contract, not the allocator implementation, to generate the
    # same private C definitions as runtime::c_source(). Native tests independently
    # verify the public numeric codes and messages.
    contract = (repository / 'src/runtime.rs').read_text(encoding='utf-8')
    codes = re.findall(r'^\s+(\w+) = (\d+),', contract, re.M)
    assert len(codes) == 8
    header = ''.join(f'#define CHEEKU_{re.sub(r"(?<!^)(?=[A-Z])", "_", name).upper()} {code}\n' for name, code in codes)
    macros = '#define malloc cheeku_test_runtime_malloc\n#define free cheeku_test_free\n#define realloc cheeku_test_realloc\n'
    runtime_source = header + macros + (repository / 'src/runtime/io.c').read_text(encoding='utf-8')
    with tempfile.TemporaryDirectory(prefix='cheeku input memory ') as temporary:
        root = Path(temporary)
        source, ir = root / 'input.chuchu', root / 'input.ll'
        runtime, shim = root / 'runtime.c', root / 'shim.c'
        output = root / ('input.exe' if os.name == 'nt' else 'input')
        source.write_text(SOURCE, encoding='utf-8')
        subprocess.run([str(compiler), str(source), '-O0', '--emit-ir', str(ir), '-o', str(output)], check=True, timeout=60)
        ir.write_text(ir.read_text(encoding='utf-8').replace('@malloc(', '@cheeku_test_malloc(').replace('@free(', '@cheeku_test_free('), encoding='utf-8')
        runtime.write_text(runtime_source, encoding='utf-8')
        shim.write_text(SHIM, encoding='utf-8')
        command = [args.clang, '-O0', '-Wno-override-module', str(ir), str(runtime), str(shim), '-o', str(output)]
        if args.sanitize:
            command.append('-fsanitize=address')
        subprocess.run(command, check=True, timeout=60)
        environment = os.environ.copy()
        environment['ASAN_OPTIONS'] = 'detect_leaks=0'
        data = ('é' * 5000 + '\nnext\n' + '12.5\n' * 1000).encode('utf-8')
        for failure in (0, 1, 2):
            environment['CHEEKU_TEST_FAIL_RUNTIME'] = str(failure)
            result = subprocess.run([str(output)], input=data, capture_output=True, env=environment, timeout=60)
            assert result.returncode == 0, result.stderr.decode('utf-8', errors='replace')
            text = result.stdout.decode('utf-8').replace('\r\n', '\n')
            assert text == ('0\n5000\nnext\ntrue\n' if not failure else '7\nrecovered\nnext\ntrue\n'), text
            assert not result.stderr, result.stderr
        print('PASS: result payloads and input buffers are balanced; initial/growing-buffer OOM recovers' + (' under AddressSanitizer' if args.sanitize else ''))


if __name__ == '__main__':
    main()

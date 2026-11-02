#!/usr/bin/env python3
"""Check file I/O string/result ownership, native file buffers and recoverable OOM.

Recompile emitted O0 IR and the embedded C runtime (io.c and file.c) with a
tracked allocator. Every allocation made by generated code and by the runtime is
counted, and the process aborts at exit unless all of them were released.
Clang 23+ is required; --sanitize enables AddressSanitizer as well.
"""
import argparse
import os
from pathlib import Path
import re
import subprocess
import tempfile

# Exercises results, copies, reassignment of owned values, function returns,
# string concatenation and both success and failure paths of every built-in.
BALANCE = '''function relay(value: result<string>) be result<string> {
    assign local as value
    if is_ok(local) { return local }
    return err<string>(8)
}
assign i as 0
while i < 1000 {
    assign written as write_file("smoke.txt", "round " + to_string(i) + " é\U0001F30D")
    assign loaded as relay(try_read_file("smoke.txt"))
    assign copy as loaded
    reassign loaded as err<string>(9)
    assign text as read_file("smoke.txt")
    reassign text as text + unwrap(copy)
    assign saved as try_write_file("smoke-copy.txt", text)
    reassign saved as err<int>(9)
    assign missing as try_read_file("missing/smoke.txt")
    reassign missing as try_read_file("smoke.txt")
    assign refused as try_write_file("missing/smoke.txt", text)
    reassign i as i + 1
}
show length(read_file("smoke.txt"))
show error_code(try_read_file("missing/smoke.txt"))
show error_code(try_write_file("missing/smoke.txt", "x"))
'''

# Only try_* variants and no to_string, so an injected allocation failure is
# recoverable (to_string allocation failures are fatal by design). Each read makes
# two runtime allocations: its buffer, then a best-effort shrink. Failing the Nth
# runtime allocation therefore gives code 7 for odd N (the buffer) and nothing
# visible for even N (the shrink is optional), and everything must still be freed.
INJECT = '''assign i as 0
assign failures as 0
while i < 300 {
    assign wrote as try_write_file("inject.txt", "payload")
    assign loaded as try_read_file("inject.txt")
    reassign failures as failures + error_code(loaded)
    reassign i as i + 1
}
show failures
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
/* Generated code: counted, never failed. */
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
/* Native runtime: counted, and the Nth one fails when requested. */
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
    # same private C definitions as runtime::c_source().
    contract = (repository / 'src/runtime.rs').read_text(encoding='utf-8')
    codes = re.findall(r'^\s+(\w+) = (\d+),', contract, re.M)
    assert len(codes) == 8
    header = ''.join(f'#define CHEEKU_{re.sub(r"(?<!^)(?=[A-Z])", "_", name).upper()} {code}\n' for name, code in codes)
    macros = '#define malloc cheeku_test_runtime_malloc\n#define free cheeku_test_free\n#define realloc cheeku_test_realloc\n'
    runtime_source = header + macros + ''.join(
        (repository / 'src/runtime' / name).read_text(encoding='utf-8') for name in ('io.c', 'file.c'))
    with tempfile.TemporaryDirectory(prefix='cheeku file memory ') as temporary:
        root = Path(temporary)
        runtime = root / 'runtime.c'
        runtime.write_text(runtime_source, encoding='utf-8')

        def build(name, program, shim_source):
            source, ir = root / f'{name}.chuchu', root / f'{name}.ll'
            shim = root / f'{name}-shim.c'
            output = root / (f'{name}.exe' if os.name == 'nt' else name)
            source.write_text(program, encoding='utf-8')
            subprocess.run([str(compiler), str(source), '-O0', '--emit-ir', str(ir), '-o', str(output)], check=True, timeout=60)
            ir.write_text(ir.read_text(encoding='utf-8').replace('@malloc(', '@cheeku_test_malloc(').replace('@free(', '@cheeku_test_free('), encoding='utf-8')
            shim.write_text(shim_source, encoding='utf-8')
            command = [args.clang, '-O0', '-Wno-override-module', str(ir), str(runtime), str(shim), '-o', str(output)]
            if args.sanitize:
                command.append('-fsanitize=address')
            subprocess.run(command, check=True, timeout=60)
            return output

        environment = os.environ.copy()
        environment['ASAN_OPTIONS'] = 'detect_leaks=0'

        def run(program, failure=0):
            environment['CHEEKU_TEST_FAIL_RUNTIME'] = str(failure)
            work = root / 'work'
            work.mkdir(exist_ok=True)
            return subprocess.run([str(program)], capture_output=True, env=environment, cwd=work, timeout=120)

        balance = build('balance', BALANCE, SHIM)
        result = run(balance)
        text = result.stdout.decode('utf-8', errors='replace').replace('\r\n', '\n')
        assert result.returncode == 0, result.stderr.decode('utf-8', errors='replace')
        assert text == '12\n4\n4\n', text
        assert not result.stderr, result.stderr

        inject = build('inject', INJECT, SHIM)
        outcomes = []
        for failure in range(0, 24):
            result = run(inject, failure)
            assert result.returncode == 0, (failure, result.stderr.decode('utf-8', errors='replace'))
            assert not result.stderr, result.stderr
            text = result.stdout.decode('utf-8').replace('\r\n', '\n')
            assert text in ('0\n', '7\n'), (failure, text)
            # Windows also allocates a UTF-16 path buffer per open, which shifts
            # the numbering, so the exact odd/even pattern is checked elsewhere.
            if os.name != 'nt':
                assert text == ('7\n' if failure % 2 else '0\n'), (failure, text)
            outcomes.append(text)
        assert outcomes[0] == '0\n' and '7\n' in outcomes, outcomes
        print('PASS: file strings and results are balanced; read-buffer OOM is a recoverable code 7' + (' under AddressSanitizer' if args.sanitize else ''))


if __name__ == '__main__':
    main()

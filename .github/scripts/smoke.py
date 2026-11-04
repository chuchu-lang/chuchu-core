#!/usr/bin/env python3
"""Compile and execute representative examples on the native host."""
from pathlib import Path
import argparse
import os
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--compiler', type=Path, required=True)
    args = parser.parse_args()
    compiler = args.compiler.resolve()
    root = Path(__file__).resolve().parents[2]
    expected = {'modules/main': '5\n3\n5\n',
                'input': 'x = 42\n',
                'comments': '42\n15\nComment markers inside strings are text: // and /* ... */\n',
                'main': '35\n', 'records': 'Ali\nLahore\nSara\nKarachi\n23\n22\n', 'assignment': '35\n50\n', 'numerical': '32\n3\n7\n',
                'syntax_smoke': '25\n100\n4.5\ntrue\n0\n1\n2\n',
                'strings': 'Hello, دنیا 🌍!\n6\ntrue\nHello, دنیا 🌍! Welcome to Chuchu.\nEscapes: \"quotes\", \\backslash, and\ttab\n',
                'temperature': 'Celsius to Fahrenheit. Enter a number or quit.\nCelsius:\nTry again: Invalid number\nCelsius:\n50 F\nCelsius:\nDone.\n'}
    with tempfile.TemporaryDirectory(prefix='cheeku smoke ') as directory:
        for name, output in expected.items():
            for level in ['-O0', '-O1', '-O2', '-O3']:
                executable = Path(directory) / (name.replace('/', '-') + ('.exe' if os.name == 'nt' else ''))
                subprocess.run([compiler, root / 'examples' / (name + '.chuchu'), level, '-o', executable], check=True)
                result = subprocess.run([executable], input='bad\n10\nquit\n' if name == 'temperature' else ('42\n' if name == 'input' else ''),
                                        capture_output=True, text=True, encoding='utf-8', check=True, timeout=30)
                if result.stdout != output or result.stderr:
                    raise RuntimeError(f'{name} {level}: {result.stdout!r} / {result.stderr!r}')
    print(f'PASS: {len(expected)} examples at O0/O1/O2/O3, {len(expected) * 4} native executions')


if __name__ == '__main__':
    main()

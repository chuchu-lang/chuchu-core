# Input, conversions, and results

`input()` reads a line from standard input and returns a `string` directly.
An optional prompt is printed and flushed without adding a newline:

```chuchu
assign name as input("What is your name? ")
show "Hello, " + name + "!"
```

No `is_ok` or `unwrap` is needed. `input()` reports a runtime error and exits
with status 1 on EOF or a read failure. An empty input line succeeds with `""`.
Input remains text; use `parse_int` or `parse_float` for numeric conversion.

## Handle input failures explicitly

Use `try_input()` or `try_input(prompt)` when the program should recover from
EOF or errors. These return `result<string>`:

```chuchu
assign line as try_input("What is your name? ")
if is_ok(line) {
    show "Hello, " + unwrap(line) + "!"
} else {
    if is_eof(line) {
        show "Goodbye."
    } else {
        show error_message(line)
    }
}
```

`show` adds a newline. Both input functions accept any string expression as a
prompt, add no newline of their own, and treat `%` as literal text. Prompt
write or flush failures are input/output errors (code 4): `input` stops with a
runtime diagnostic, while `try_input` returns an error result.

`parse_int(text)` and `parse_float(text)` also return results, allowing the
program to handle invalid numbers explicitly.

## Result values

A result contains an integer status and a scalar payload. Its annotation is
`result<int>`, `result<float>`, `result<bool>`, or `result<string>`. Results can be
stored, reassigned, passed to functions, and returned. Payload types must match
exactly; nested results, array payloads, and implicit result conversions are unsupported.

| Operation | Meaning |
| --- | --- |
| `ok(value)` | Successful result; infers its scalar payload type |
| `err<int>(8)` | Failed integer result with an explicit status; also accepts `float`, `bool`, `string` |
| `is_ok(result)` | True when status is zero |
| `is_eof(result)` | True when status is 1 |
| `error_code(result)` | Integer status, including zero for success |
| `error_message(result)` | Owned string describing the status |
| `unwrap(result)` | Extracts a successful payload; exits 1 with a checked runtime diagnostic on error |

Check `is_ok` before `unwrap`. Results cannot be printed directly with `show`.
String payloads retain the existing value semantics: copying a result preserves
its text even if the original is reassigned. Storage is cleaned up automatically.
There is no `try` propagation operator or general user-defined enum system yet.
`err` is a reserved keyword; the built-in function names cannot be redeclared.

```chuchu
function positive(text: string) be result<int> {
    assign parsed as parse_int(text)
    if is_ok(parsed) {
        assign value as unwrap(parsed)
        if value > 0 { return ok(value) }
        return err<int>(8)
    }
    return parsed
}
assign value as positive("42")
if is_ok(value) { show unwrap(value) }
```

## Error codes

| Code | `error_message` | Meaning |
| --- | --- | --- |
| 0 | No error | Successful result |
| 1 | End of input | No bytes remain before another line starts |
| 2 | Invalid number | Invalid integer or decimal floating syntax |
| 3 | Value out of range | Numeric overflow/underflow, or input size overflow |
| 4 | Input/output error | Reading stdin, writing the prompt, or flushing stdout failed |
| 5 | Input is not valid UTF-8 | Invalid UTF-8, or invalid Windows console UTF-16 |
| 6 | Input contains a NUL byte | Embedded NUL is unsupported |
| 7 | Memory allocation failed | The input or parsing helper could not allocate |
| 8–2147483647 | Unknown error | Application-defined codes; handle them in your program |

`err<T>(code)` requires an integer in 1–2147483647. An invalid code triggers a
checked runtime failure. Codes 1–7 are reserved for the built-in meanings.

The file functions `try_read_file` and `try_write_file` reuse codes 3–7 with the
same messages; see [file I/O errors](FILES.md#errors).

## Line behavior

Both `input` and `try_input` flush stdout before reading, so prompts are visible
even through a pipe. The prompt argument adds no newline; `show` adds one.
The input reader removes LF and the CR immediately preceding LF.
An empty line succeeds with `""`; a final line without a newline also succeeds.
A subsequent read returns EOF. A trailing CR without LF remains in the string.

Input must be UTF-8 without NUL. Rejected lines, including lines whose buffer
allocation fails, are consumed through LF or EOF so a retry starts at the next
line. An I/O failure returns immediately. Input length grows with available
memory. Windows console input uses wide-character reads converted to UTF-8;
redirected input uses binary byte reads. Ordinary output retains host CRT behavior.

## Explicit conversions

`parse_int` accepts ASCII whitespace around optional `+`/`-` and decimal digits,
within signed 64-bit range. It rejects fractions, hexadecimal notation, internal
spaces, separators, and non-ASCII digits or whitespace.

`parse_float` accepts a decimal number with optional sign, decimal point, and
exponent, including `42`, `.5`, `5.`, and `1e-3`. It trims ASCII whitespace, uses
the C numeric locale, and rejects trailing text, hexadecimal numbers, commas,
NaN, and infinity. Overflow and libc-reported underflow produce code 3.

`to_string(value)` accepts `int`, `float`, `bool`, and `string`. Integers use
signed decimal; booleans use `true`/`false`; strings preserve their contents.
Floats use locale-independent `%.17g`, so ordinary finite values retain enough
precision for a parse round trip. Subnormal values may fail the underflow check
when parsed; non-finite calculation results format as text but cannot be parsed.
`show` retains its existing `%.15g` float formatting. No implicit conversion is added.

```chuchu
assign parsed as parse_int(" 42 ")
if is_ok(parsed) {
    show "Count: " + to_string(unwrap(parsed))
} else {
    show "Try again: " + error_message(parsed)
}
```

Ordinary string copies, concatenation, `error_message`, and `to_string` still use
checked fatal allocation diagnostics. Recoverable input does not make all
allocation or arithmetic failures recoverable.

## Interactive example and implementation

Rebuild Cheeku to use the new syntax and built-ins:

```sh
cargo build --locked --release
./target/release/cheeku examples/temperature.chuchu -o temperature
./temperature
```

On Windows use `target\release\cheeku.exe` and `-o temperature.exe`. The converter
retries invalid numbers, rejects temperatures below absolute zero with application
error 8, and exits on `quit` or EOF.

The compiler embeds a small C11 runtime and compiles it with the configured host
Clang driver when needed; there is no separate runtime installation. Ordinary
programs that use none of these native helpers keep their existing link path.
The high-level `compile_source`/`compile_file` APIs link helpers automatically.
Low-level object API users must write `chuchu::runtime::c_source()` to a `.c` file
and compile/link it alongside objects that reference those helpers.

LLVM represents results as `{ i32 status, payload }`; debugger fields are named
`code` and `value`. Native integration tests cover input, conversion, result
ownership, error recovery, and O0–O3 behavior. The debugger smoke script inspects
integer and string result fields. For allocator accounting and injected failures:

```sh
python3 .github/scripts/io-memory-smoke.py --compiler target/release/cheeku \
  --clang /path/to/llvm/bin/clang --sanitize
```

This implementation was validated locally on macOS ARM64. The shared CI includes
the native input/conversion tests for macOS ARM64/Intel, Linux x64 GNU, and Windows
x64 MSVC; results for this revision on the other hosts remain pending. Interactive
Windows console input also requires native manual validation.

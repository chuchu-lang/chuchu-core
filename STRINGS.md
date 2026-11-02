# Strings

Strings are immutable UTF-8 text values written with double quotes. Variables
infer the `string` type; function parameters and return values use `string`.

```chuchu
function greet(name: string) be string {
    return "Hello, " + name + "!"
}

assign message as greet("world 🌍")
show message
reassign message as message + " Welcome."
show message
show length(message)
show message == "Hello, world 🌍! Welcome."
```

`show` prints the text followed by a newline. Percent signs in strings are
ordinary text. Existing platform CRT newline handling also applies to strings.
Concatenation with `+` requires two strings; numbers and booleans are not
implicitly converted. Equality (`==`) and inequality (`!=`) compare contents,
including UTF-8 bytes, rather than allocation addresses. Other arithmetic and
ordering operators are rejected.

`length` counts Unicode scalar values. For example, `length("é中🌍")` is 3. A
combining mark counts separately: `length("é")` is 2, even though it can look
like one character. No Unicode normalization is performed, so `"é"` and `"é"`
differ.
Empty strings are supported and have length 0. String indexing, slicing,
interpolation and string arrays are not implemented. Explicit numeric conversions
are covered in [input and results](INPUT.md).

## Escapes and errors

| Escape | Value |
| --- | --- |
| `\n` | Newline |
| `\r` | Carriage return |
| `\t` | Tab |
| `\"` | Double quote |
| `\\` | Backslash |

Put Unicode characters directly in the source. Strings must close on the same
source line; use `\n` for embedded newlines. Unsupported escapes, unclosed
quotes, and NUL characters (including `\0`) produce source diagnostics. This
initial implementation does not support embedded NUL bytes or `\u`/`\x` escapes.

Reassignment can replace a string variable's value, but cannot change its type.
Assigning or passing a string preserves value semantics: later reassignment of
the original variable does not affect copies or returned values.

## Run the example

```sh
./target/release/cheeku examples/strings.chuchu -o strings-demo
./strings-demo
```

On Windows use `target\release\cheeku.exe` and an output ending in `.exe`.
Add `--debug` to inspect strings in LLDB. They are represented as character
pointers in debugger metadata, so `frame variable message` displays the text.
The same compiler implementation targets the core's existing host platforms;
this change was validated locally on macOS ARM64. The four-target CI includes
native string tests, but Linux, Windows and macOS Intel results are pending.

## Storage and verification

String expressions own heap allocations. Identifier reads copy string contents;
declarations and function parameters take ownership, and returns transfer the
result after releasing locals. Reassignment frees the old value. The compiler
releases temporaries and block-local values on every iteration and return path.
This implementation favors straightforward ownership over copy elimination;
concatenation copies its inputs and repeatedly appending grows quadratically.
Allocation and size failures use the existing runtime diagnostic/exit-1 path.

`cargo test --locked` checks literals, diagnostics, type rules, UTF-8 length,
functions, recursion, ownership-sensitive programs and O0–O3 builds with and
without debug metadata. The debugger smoke script checks live string values.

For allocation accounting and AddressSanitizer checks, use Clang 23 or newer:

```sh
python3 .github/scripts/string-memory-smoke.py \
  --compiler target/release/cheeku \
  --clang /path/to/llvm/bin/clang --sanitize
```

The script verifies zero outstanding string allocations at successful exit,
bounded live allocations through 2,000 loop iterations and recursive returns,
and graceful handling of three injected allocation failures. AddressSanitizer
checks buffer accesses and lifetimes; explicit accounting checks leaks even on
macOS, where LeakSanitizer is unavailable. Error exits rely on process teardown
to reclaim remaining live values.

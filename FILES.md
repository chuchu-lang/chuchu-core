# File I/O

Chuchu reads and writes whole UTF-8 text files with four built-in functions.
Each takes its paths and text as `string` values and handles one complete file
per call.

| Function | Parameters | Returns | When it fails |
| --- | --- | --- | --- |
| `read_file(path)` | `path: string` | `string`, the file's contents | stops the program with exit status 1 |
| `try_read_file(path)` | `path: string` | `result<string>` | returns an error result |
| `write_file(path, text)` | `path: string`, `text: string` | `int`, the bytes written | stops the program with exit status 1 |
| `try_write_file(path, text)` | `path: string`, `text: string` | `result<int>`, the bytes written | returns an error result |

The plain forms suit short programs where a failure should end the run. The
`try_` forms return a [`result`](INPUT.md#result-values) so the program can
recover. This is the same split as `input()` and `try_input()`.

```chuchu
assign bytes as write_file("greeting.txt", "Hello, world 🌍")
show bytes
assign text as read_file("greeting.txt")
show text
show length(text)
```

Output:

```text
17
Hello, world 🌍
14
```

`write_file` counts UTF-8 **bytes** (17 here), while `length` counts Unicode
scalar values (14). The globe is one character but four bytes.

## Reading

`read_file` and `try_read_file` return the entire file as one string.

- An empty file gives `""`.
- The contents must be valid UTF-8 and must not contain a NUL byte, because
  Chuchu strings cannot hold NUL. Otherwise the read fails (see
  [errors](#errors)).
- Contents are returned exactly as stored. Newlines (`\n`, `\r\n`, `\r`) and a
  trailing newline are kept, and a UTF-8 byte-order mark is kept as the
  character U+FEFF, which `length` counts. `input()` strips line endings;
  file reads do not.
- The string is a new value owned by your program, like any other string.

## Writing

`write_file` and `try_write_file` create the file if it does not exist and
replace its contents if it does.

- The text is written exactly as given. No newline and no byte-order mark are
  added.
- Writing an empty string leaves an empty file.
- The result is the number of bytes written.
- Missing parent directories are not created; writing into one fails.

```chuchu
assign first as write_file("note.txt", "a much longer first version")
assign second as write_file("note.txt", "short")
show read_file("note.txt")
assign third as write_file("note.txt", "")
show length(read_file("note.txt"))
```

Output:

```text
short
0
```

Because calls are expressions, a result must be used: `write_file("a", "b")` on
its own line is a syntax error. Bind it, as above, even if you ignore the value.

## Paths

- A **relative path** is resolved against the program's working directory when
  it runs, not the source file's folder and not the executable's folder.
- An **absolute path** is used as given.
- Paths may contain spaces and any Unicode characters. On Windows they are
  converted to UTF-16, so names do not depend on the system code page.
- Forward slashes work on every platform, including Windows. A backslash in a
  Chuchu string must be escaped as `\\`.
- Nothing is expanded: `~`, environment variables and wildcards are literal
  text.
- Symbolic links are followed. Writing through a link changes its target, and
  writing through a dangling link creates the target file.
- A path is a Chuchu string, so it is always valid UTF-8. A file whose name is
  not valid UTF-8 cannot be named.
- An empty path is an error.

## Errors

### Recoverable errors with `try_`

`try_read_file` and `try_write_file` never stop the program. Check the result
with the usual operations: `is_ok`, `unwrap`, `error_code` and `error_message`.

```chuchu
assign config as try_read_file("settings.txt")
if is_ok(config) {
    show "Loaded " + to_string(length(unwrap(config))) + " characters"
} else {
    show "Could not read settings.txt: " + error_message(config)
    show "Error code: " + to_string(error_code(config))
}
assign saved as try_write_file("settings.txt", "volume=7")
if is_ok(saved) {
    show "Saved " + to_string(unwrap(saved)) + " bytes"
}
assign again as try_read_file("settings.txt")
if is_ok(again) {
    show "Loaded " + to_string(length(unwrap(again))) + " characters"
}
```

Output:

```text
Could not read settings.txt: Input/output error
Error code: 4
Saved 8 bytes
Loaded 8 characters
```

The file functions reuse the [existing error codes](INPUT.md#error-codes). They
do not add any:

| Code | `error_message` | Returned when |
| --- | --- | --- |
| 0 | No error | the call succeeded |
| 3 | Value out of range | the file's size would overflow the size arithmetic |
| 4 | Input/output error | the file cannot be opened, read, written or closed: it is missing, access is denied, the path is a directory, the path is empty or too long, the parent directory is missing, the disk is full, and similar |
| 5 | Input is not valid UTF-8 | a file being read is not valid UTF-8 |
| 6 | Input contains a NUL byte | a file being read contains a NUL byte |
| 7 | Memory allocation failed | memory for the contents (or, on Windows, the converted path) could not be allocated |

Notes:

- Codes 1 and 2 (end of input and invalid number) are never produced, so
  `is_eof` is always `false` for these results. Reading an empty file succeeds.
- A missing file and a permission error both give code 4 with the same message.
  Check the file's existence another way if you need to tell them apart.
- A NUL byte is reported before invalid UTF-8, so a UTF-16 file (which contains
  NUL bytes) gives code 6.
- The message text says "Input" because the messages are shared with
  `input()`. Compare codes, not message text, in your program.

### Failures that stop the program

`read_file` and `write_file` print one line to standard error, which ends with
the byte range of the failing call, and exit with status 1. Output printed
before the failure is kept.

```chuchu
show "before"
assign text as read_file("missing.txt")
show "after"
```

Output:

```text
before
```

Error output (exit status 1):

```text
cheeku runtime: read_file failed: the file could not be opened or read; use try_read_file() to handle errors at 29..53
```

`read_file` reports the specific reason when it knows it:

| Failure | Message |
| --- | --- |
| invalid UTF-8 | `read_file failed: the file is not valid UTF-8; use try_read_file() to handle errors` |
| NUL byte | `read_file failed: the file contains a NUL byte; use try_read_file() to handle errors` |
| out of memory | `read_file failed: out of memory while reading the file; use try_read_file() to handle errors` |
| anything else | `read_file failed: the file could not be opened or read; use try_read_file() to handle errors` |

`write_file` has one message: `write_file failed: the file could not be created,
written or closed; use try_write_file() to handle errors`.

## Types

Every argument must be exactly a `string`. Numbers and booleans are not
converted automatically, and results must be unwrapped first. The compiler
rejects a wrong call before generating code, and the message names the argument
at fault.

```chuchu
assign bytes as write_file("count.txt", 42)
```

Compile error:

```text
error: Function 'write_file': the text to write must be a string; convert values with to_string(...) or extract a result with unwrap(...)
```

Convert first instead:

```chuchu
assign bytes as write_file("count.txt", to_string(42))
show read_file("count.txt")
```

Output:

```text
42
```

The other messages are `expected 1 argument: the file path`, `expected 2
arguments: the file path and the text to write` and `the file path must be a
string`.

The four names are reserved: a program cannot declare its own function called
`read_file`, `try_read_file`, `write_file` or `try_write_file`.

## More examples

### Append a line

There is no append function. Read, extend and write back. Only a missing file
(code 4) is treated as "start a new file"; any other read failure, such as
invalid UTF-8, is passed on so existing data is never overwritten.

```chuchu
function append_line(path: string, line: string) be result<int> {
    assign existing as try_read_file(path)
    if is_ok(existing) {
        return try_write_file(path, unwrap(existing) + line + "\n")
    }
    if error_code(existing) == 4 {
        return try_write_file(path, line + "\n")
    }
    return err<int>(error_code(existing))
}
assign first as append_line("log.txt", "started")
assign second as append_line("log.txt", "finished")
show unwrap(first)
show unwrap(second)
show read_file("log.txt")
```

Output:

```text
8
17
started
finished

```

The last line is blank because the file ends with a newline and `show` adds one.
Code 4 also covers a permission error, in which case the write that follows
fails too and the error is returned.

### Copy a file

```chuchu
function copy_file(from: string, to: string) be result<int> {
    assign source as try_read_file(from)
    if is_ok(source) {
        return try_write_file(to, unwrap(source))
    }
    return err<int>(error_code(source))
}
assign made as write_file("original.txt", "copy me")
assign copied as copy_file("original.txt", "backup.txt")
if is_ok(copied) {
    show "Copied " + to_string(unwrap(copied)) + " bytes"
}
assign missing as copy_file("nothing.txt", "never.txt")
show "Copy failed with code " + to_string(error_code(missing))
```

Output:

```text
Copied 7 bytes
Copy failed with code 4
```

### Read and rewrite the same file

All arguments are evaluated, left to right, before the file is touched, so the
file is read completely before it is truncated:

```chuchu
assign made as write_file("same.txt", "unchanged")
assign size as write_file("same.txt", read_file("same.txt"))
show size
show read_file("same.txt")
```

Output:

```text
9
unchanged
```

## Run the example

[`examples/files.chuchu`](examples/files.chuchu) writes, reads, overwrites and
recovers from a missing file. It creates `chuchu-files-example.txt` in the
current directory.

```sh
./target/release/cheeku examples/files.chuchu -o files-demo
./files-demo
```

On Windows use `target\release\cheeku.exe` and an output ending in `.exe`.

## Storage and cleanup

Strings and results returned by these functions are owned by your program and
are released automatically, like every other string; there is nothing to close
or free. The native helper opens the file, finishes reading or writing, and
closes it before returning, on every success and failure path, so no handle
stays open between calls. Arguments are copied for the call, so later changes to
a variable never affect a write already made. When a plain function stops the
program, the operating system reclaims what is left.

## Platform notes

- **Newlines.** Files are read and written byte for byte on every platform.
  `show` follows the host's output conventions, so on Windows it ends each line
  with `\r\n`; file contents are not changed.
- **Windows.** Very long paths and names the system forbids (such as `a<b`)
  fail with code 4. Reserved device names such as `NUL` are handled by the
  operating system, not by Chuchu.
- **macOS.** Filenames that differ only in Unicode normalization may refer to
  the same file, as the file system defines.
- **Unix-like systems.** Special files can be read, for example
  `read_file("/dev/stdin")` reads all of standard input. A source that never
  ends and contains no NUL byte is read until memory runs out (code 7).

## Limitations

- Whole files only: there is no streaming, seeking, appending or partial read.
- Text only: files with NUL bytes or invalid UTF-8 cannot be read, and there is
  no binary mode.
- There is no way to test that a file exists, delete, rename or list files, or
  create directories.
- Writes are not atomic. A failure after the file was opened (for example a full
  disk) can leave it truncated or partly written.
- There is no file locking, and no limit on file size other than available
  memory. Reading a file can use up to about twice its size at the peak.
- A missing file cannot be told apart from a permission error (both are code 4).
- `write_file` returns bytes, not characters.

## Verification

The runtime, type rules and generated code have their own suites:

```sh
cargo test --locked --test file_runtime_integration
cargo test --locked --test file_io_type_checker_integration
cargo test --locked --test file_io_integration
python3 .github/scripts/file-memory-smoke.py --compiler target/release/cheeku \
  --clang /path/to/llvm/bin/clang --sanitize
```

The end-to-end suite builds and runs programs at O0 to O3 and covers Unicode
and relative and absolute paths, overwriting, empty and large files, every error
code, recovery, argument order, and string ownership. The memory script (Clang
23 or newer, Linux in CI) verifies that every allocation is released, that an
allocation failure while reading is a recoverable code 7, and that nothing
fails under AddressSanitizer.

The file suites pass in
[hosted CI](https://github.com/chuchu-lang/chuchu-core/actions/runs/41520343694)
on Linux x64, Windows x64 MSVC, macOS Intel and macOS ARM64. Tests that need
Unix permissions, symbolic links, descriptor limits or standard input run on
Linux and macOS only, two helper tests (a full device and `/proc`) run on Linux
only, and the memory script runs on Linux only.

# Cheeku native distribution requirements

These archives are **not standalone SDKs**. Each contains one native compiler,
the Chuchu MIT license, and examples. LLVM, Clang, and Microsoft redistributables
are not bundled; their redistribution licenses therefore do not become package
contents. Install dependencies from their vendors and retain their licenses.

| Archive | Rust host | Native toolchain/runtime requirements |
|---|---|---|
| cheeku-macos-arm64 | aarch64-apple-darwin | Matching LLVM 23.1 shared libraries, Apple xcrun clang |
| cheeku-macos-x64 | x86_64-apple-darwin | Matching Intel LLVM 23.1 shared libraries, Apple xcrun clang |
| cheeku-linux-x64 | x86_64-unknown-linux-gnu | Matching LLVM 23.1 shared libraries, compatible glibc/libstdc++, Clang, GNU linker, libc/libm development files |
| cheeku-windows-x64 | x86_64-pc-windows-msvc | LLVM clang-cl, x64 VS C++ Build Tools and Windows SDK, compatible MSVC runtime; LLVM libraries are statically linked into Cheeku by llvm-sys |

CI pins SDK 23.1.4. Unix users must make its libraries discoverable:
`LD_LIBRARY_PATH=<LLVM prefix>/lib` on Linux or
the LLVM `lib` directory on macOS. Source builds of Cheeku embed that directory
as an rpath, so macOS needs no `DYLD_LIBRARY_PATH`; do not set one, because rustc
loads its own `@rpath/libLLVM.dylib` and a global path swaps in an incompatible
LLVM and crashes it. Use architecture-matching installations. Do not rename an ARM binary
and describe it as x64. Windows users need a configured x64 developer environment
for native program linking even when Cheeku itself starts successfully.

Build outputs are packaged only after the matching native tests and release
smoke tests succeed. Tag workflow `release.yml` creates a **draft** only when all
four target jobs succeed; review platform evidence before publishing that draft.
No release is published merely by adding workflow files. Check that the tagged
revision's four target jobs passed before publishing a draft.

Build locally: `cargo build --locked --release`, run the tests, then
`python .github/scripts/package-release.py --target <your rustc host triple>`.
The script rejects a target label different from the actual Rust host and verifies
that Cheeku starts. It does not replace the preceding native test requirement.

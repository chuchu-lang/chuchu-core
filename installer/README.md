# Chuchu installers and updates

This implementation prepares downloads for **your own website**. It does not
publish to an app store, upload to the marketing site, or enable automatic
background updates. The default download location is
`https://chuchu-lang.org/downloads`; override `--base-url` when packaging for a
different host. For that override, build the manager with the compile-time
`CHEEKU_UPDATE_URL=https://your-host/downloads/latest.json` environment variable;
the packager verifies that the Windows bootstrap uses the same update host.
That endpoint must be configured before public installers work.

## User experience

- Windows x64: run `Chuchu-Setup-<version>-windows-x64.exe`. This is a console
  installer that downloads the current compiler. When needed it starts the
  vendor's C++ Build Tools installation using winget. The user may see an OS
  elevation prompt. Windows without winget requires manually installing the
  C++ Build Tools workload and Windows SDK first.
- macOS ARM64/Intel: run the matching `Chuchu-<version>-<target>.pkg`, or run the
  `.command` bootstrap in Terminal. The package starts a per-user installation
  for the logged-in desktop user. If Apple Command Line Tools are absent, their
  installation dialog opens; finish it and rerun the Chuchu installer.
- Linux x64 GNU: download the matching `install-chuchu-<version>-<target>.sh`
  and run it with `sh`. It can install C build prerequisites through apt or dnf
  using sudo. Other distributions receive an actionable prerequisite error.

Users do **not** build LLVM or Rust. Compiler payloads bundle LLVM and its
non-system shared-library dependencies. Windows also bundles clang-cl and its
resource headers. Apple system SDKs and Windows C++/SDK components come from
OS/vendor installation tools rather than being copied from the CI runner.
Linux keeps its system libc, libstdc++, and C development toolchain.

Open a new terminal/application session after installation. Then:

```sh
cheeku --version
cheeku hello.chuchu -o hello
./hello
cheeku update --check
cheeku update
```

On Windows use `-o hello.exe` and `./hello.exe` in PowerShell. The launcher
initializes the installed MSVC environment for each compiler invocation; an x64
Developer PowerShell is not required. The VS Code extension remains a separate
installation and finds `cheeku` through PATH. Existing VS Code windows may need
to be restarted to pick up PATH changes.

## Installation layout

Defaults:

| OS | Directory |
| --- | --- |
| Windows | `%LOCALAPPDATA%/Chuchu` |
| macOS | `~/Library/Application Support/Chuchu` |
| Linux | `${XDG_DATA_HOME:-~/.local/share}/chuchu` |

```text
bin/cheeku[.exe]              stable manager/launcher, independent of LLVM
current.json                 active version and update channel
versions/<version>/bin/cheeku native compiler
versions/<version>/lib/       relocated runtime libraries
versions/<version>/licenses/  dependency notices
versions/<version>/tools/     Windows clang-cl
```

Unix installs add a marked PATH entry to `.profile` and `.bashrc`, plus
`.zprofile` on macOS and an existing `.bash_profile`. Windows updates the user's
PATH registry value. No administrator access is needed for compiler files;
prerequisite installation and the macOS package itself may request elevation.
Use `install --prefix <directory> --no-path` for a custom installation.

## Update policy and verification

The stable launcher implements `cheeku update` and `cheeku update --check`.
It downloads `latest.json` over HTTPS, selects the exact OS/architecture, checks
SemVer, and verifies the archive's SHA-256 and size before extraction. Archives
cannot contain traversal paths, symlinks, or special files and have size limits.
HTTPS protects the manifest; checksums detect altered downloads. Detached
release signatures are not implemented yet.

Each release has a separate directory. The manager verifies the compiler's
reported version and compiles **and executes** hello-world from the relocated
installation before replacing `current.json`. A failed download or verification
keeps the previous compiler active. Failed smoke checks remove the staged
version, so retrying works. Updates to older/equal versions are ignored. Previous
versions remain on disk; automatic pruning and an uninstall command are future
work. A process lock prevents simultaneous installation into one prefix. After
a crashed installer, remove `install.lock` only after checking no installer is
still running.

Version-specific installer manifests point updates at the stable `latest.json`
channel, rather than pinning users to their initial version. The launcher itself
is not replaced while it is running: schema-compatible compiler/runtime updates
work through `cheeku update`; manager protocol upgrades will require a new
installer. Rerunning an external installer refreshes the launcher even when
the compiler version is unchanged. Close active compiler processes first on
Windows. Automatic manager self-update support is future work.

## Build packages

Run on the **matching native host**, after the compiler's tests pass:

```sh
cargo build --locked --release
cargo test --locked --manifest-path installer/manager/Cargo.toml
cargo build --locked --release --manifest-path installer/manager/Cargo.toml
python3 installer/collect-notices.py --prefix "$LLVM_SYS_231_PREFIX"
python3 installer/package.py --target aarch64-apple-darwin
python3 installer/smoke.py \
  --manager installer/manager/target/release/cheeku-manager \
  --directory target/distributions
```

Use the appropriate target triple and `.exe` on Windows. Packaging needs
`install_name_tool`, `codesign`, and `pkgbuild` on macOS; `patchelf` and `ldd` on
Linux. macOS libraries receive ad-hoc signatures after relocation. Public
Developer ID signing/notarization and Windows Authenticode signing are release
steps still to configure; the generated installers are unsigned development
artifacts, not a claim of warning-free public installation.

All redistributed dependency notices must be supplied. The provisioner retains
LLVM's license, and the collector can retrieve a checksum-pinned copy for older
SDK caches. Linux collects system package copyright notices. If using a custom
LLVM with third-party dependencies (for example Homebrew z3/zstd), supply their
notices through `--licenses-dir`; review the resulting notices before publishing.
Manager crate license files and package metadata are also included.

## Website release layout

After all four native builds and installation smoke tests pass, combine their
artifacts in one directory:

```sh
python3 installer/assemble.py --directory target/distributions --version 0.1.0
```

The assembler checks every target and artifact checksum, then writes `latest.json`
and `SHA256SUMS`. Upload the **versioned artifacts first** to the configured
`/downloads/` location. Upload `latest.json` last, after the referenced files are
available. Configure `latest.json` for revalidation/no-cache and immutable cache
headers for versioned files. Keep previous artifacts available for recovery.
Never overwrite an existing published version with different bytes.

The manual release workflow validates an existing tag against Cargo's version,
builds that exact commit on four native runners, tests installation and updates,
assembles the website manifest, and creates a GitHub **draft** as a staging area.
GitHub is not required as the user's download host. Automatic tag triggers stay
disabled. Uploading to your site remains an explicit publishing step.

## Validation and remaining platform work

```sh
cargo fmt --manifest-path installer/manager/Cargo.toml -- --check
cargo clippy --locked --manifest-path installer/manager/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path installer/manager/Cargo.toml
python3 -m unittest discover -s installer/tests -p 'test_*.py'
```

`installer/smoke.py` tests installation into a path with spaces, actual string
program compilation/execution, update checks, successful upgrade, corrupt
archive rejection, and failed activation recovery. Its synthetic newer compiler
is a test-only native shim, never a published release. Production payloads always
come from the native compiler build.

Hosted native smoke tests are necessary but do not substitute for clean-machine
checks of prerequisite installation, OS permission prompts, PATH propagation,
Gatekeeper/SmartScreen, and distribution-specific ABI compatibility. Linux
artifacts inherit the build runner's glibc baseline; broader/older distributions
need an explicitly selected baseline and separate clean-environment tests.

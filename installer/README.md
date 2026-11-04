# Chuchu installers and updates

Production downloads are served from **AWS S3 through CloudFront** at
`https://downloads.chuchu-lang.org`. The default update manifest is
`https://downloads.chuchu-lang.org/latest.json`. Packaging prepares artifacts
locally. The tag-triggered release workflow publishes verified artifacts to S3
after production approval; compiler updates remain explicit.

Override `--base-url` when packaging and assembling for a different host. For
that override, build the manager with the compile-time
`CHEEKU_UPDATE_URL=https://your-host/latest.json` environment variable; the
packager verifies that the manager uses the same update URL. The download host
must serve the referenced artifacts before public installers work.

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

## Download release layout

After all four native builds and installation smoke tests pass, combine their
artifacts in one directory:

```sh
python3 installer/assemble.py --directory target/distributions --version 0.1.0
```

The assembler checks every target and artifact checksum, then writes `latest.json`
and `SHA256SUMS`. Upload the **versioned artifacts first** to the S3 origin so
they are served directly under `https://downloads.chuchu-lang.org/`. Upload
`latest.json` last, after the referenced files are available. The automated
publisher applies no-cache headers to `latest.json` and immutable cache headers
to versioned files. Keep previous artifacts available for recovery.
Never overwrite an existing published version with different bytes.

The release workflow runs for `v*` tag pushes or manual dispatches, validates the
version and tagged commit's ancestry from protected `main`, and builds that exact
commit on four native runners. After all tests pass, it stages a GitHub draft,
waits for the `production` environment approval, authenticates with AWS through
OIDC, and uploads verified artifacts to the existing private S3 bucket. It
publishes `latest.json` last using a conditional write, verifies CloudFront, then
finalizes the GitHub Release. Ordinary merges only run quick checks.

The manager build, packaging and assembly use matching production URLs. The
manifest schema and manager update/verification behavior are unchanged. No AWS
keys are stored in source. The publisher prevents immutable artifact overwrites,
checks every expected file, supports identical retries and refuses downgrades.
Assembly retains `cheeku-<version>.json` and `SHA256SUMS-<version>` for recovery.
The original `SHA256SUMS` remains attached to GitHub; S3 serves versioned checksum
files so different releases cannot overwrite one unversioned checksum inventory.

See [AWS installer release deployment](../docs/aws-installer-releases.md) for
required IAM policies, GitHub production reviewers and variables, publishing,
verification, retries and manual rollback. These settings must be configured by
the owner before the first public release. Native packaging itself still never
uploads; only the dedicated deployment job does.

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

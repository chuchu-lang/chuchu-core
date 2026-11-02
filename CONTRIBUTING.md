# Contributing to Chuchu

Use one shared compiler implementation for all supported operating systems.
Keep platform-specific linking and runtime code isolated, and preserve language
semantics and runtime safety checks when making portability changes.

## Development setup

Start with the [setup guide](setup.md) to install prerequisites, build the
compiler, and run an example on your operating system.

Follow the [macOS](docs/build-macos.md), [Linux](docs/build-linux.md), or
[Windows](docs/build-windows.md) guide for Rust, LLVM and linker prerequisites.
The [platform support guide](docs/platform-support.md) describes the supported
hosts, runtime ABI and executable output policy.

Before opening a pull request, run:

```sh
cargo fmt --all -- --check
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
python3 -m unittest discover -s benchmarks -p test_runner.py
python3 -m unittest discover -s .github/scripts -p 'test_*.py'
python3 .github/scripts/smoke.py --compiler target/release/cheeku
```

On Windows, use `python` and `target/release/cheeku.exe` for the Python commands.
Tests must compile and execute native programs, checking output and error
behavior. Preserve the O0–O3 behavior matrix and add regression coverage for
compiler or portability fixes. Performance methodology is documented in
[performance measurement](docs/performance.md).

## Branch Naming Conventions

Every change must use a short-lived branch created from `main`.

Format: `type/short-description`

Allowed types:

- `feature/` - New functionality
- `fix/` - Bug fixes
- `refactor/` - Internal improvements
- `test/` - Tests
- `docs/` - Documentation
- `ci/` - CI/CD changes
- `chore/` - Maintenance tasks

Use lowercase letters and hyphens.
Do not create permanent developer-specific branches.

## Pull Requests and Reviews

1. Create a GitHub Issue for significant changes.
2. Create a branch from the latest `main`.
3. Make focused changes and add relevant tests.
4. Open a pull request targeting `main`.
5. Ensure all required quick checks pass.
6. Obtain at least one teammate's approval.
7. Resolve all required review conversations.
8. Squash and merge the approved PR.
9. Delete the merged feature branch.

Direct pushes to `main` are prohibited.
Do not bypass branch protection.

## Shared Compiler Changes

Before implementing a feature that affects multiple
compiler stages:

1. Agree on the proposed Chuchu syntax.
2. Identify affected compiler modules.
3. Coordinate changes to tokens, AST, parser,
   type checker, code generation, and runtime.
4. Agree on shared interfaces before coding.
5. Prefer small foundational PRs where needed.
6. Add regression tests for new language behavior.

Avoid incompatible simultaneous changes to
shared compiler interfaces.

## CI/CD Policy

The `quick-checks` job must pass before merging.

It currently validates:

- Rust formatting
- Python benchmark tooling tests
- LLVM provisioning script tests

Quick checks do not validate native compiler
correctness.

The cross-platform native compiler workflow is
run manually when appropriate, particularly
before releases and after significant changes.

Do not release unverified native compiler builds.

## Team Coordination

- Assign a primary owner to each feature.
- Announce planned changes to shared modules.
- Open draft PRs early for complex features.
- Keep branches short-lived.
- Synchronize frequently with `main`.
- Review teammates' PRs promptly.
- Document user-visible syntax changes.

## SDK caches and releases

LLVM provisioning and release packaging are maintained in `.github/scripts`.
SDK cache recipe identifiers are selected in `.github/workflows/native-build.yml`.
Update a platform's recipe identifier when its SDK contents change, and never
reuse a cache built with a different recipe. Documentation-only changes do not
require rebuilding LLVM.

Tag a release only after the revision passes every required native check. The
release workflow rebuilds and retests each target before creating draft archives.
Review the [runtime requirements](docs/release-runtime.md) before publishing.

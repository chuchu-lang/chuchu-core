# Download integration handoff

Implemented locally in the compiler and website repositories. No changes were
pushed, no real release was published, and no AWS or DNS configuration was changed.

## Compiler changes

The existing four-platform build/test pipeline and production approval remain.
Assembly now computes website installer metadata from the generated files,
creates `downloads.json` plus immutable `downloads-<version>.json`, and includes
the immutable snapshot in the versioned checksum inventory. Validation checks
the complete inventory, matching versions, filenames, sizes and hashes before
upload. The publisher verifies immutable S3 objects before committing the
unchanged updater manifest, then conditionally refreshes the website mirror.
It preserves downgrade prevention and detects competing writes on both mutable
keys. CloudFront verification includes the new manifest and snapshot.

`latest.json` is the sole commit point. If the mirror lags after a failed write,
the website resolves the pointer's version through its already published
immutable snapshot. Retry the original failed deployment artifact to repair the
mirror. A failure before commit preserves the previous active release; after
commit, the complete new release may be active while GitHub remains draft.

## Website changes

Server-side fetching validates the complete website schema and exact trusted
CDN filenames/URLs, with request timeout, streamed byte limit and redirect
rejection. A `use cache` utility revalidates after 60 seconds and expires after
300 seconds. Both valid releases and unavailable results refresh without builds.
The production browser test confirmed a newer version appears without restarting
or rebuilding, and an outage subsequently removes download buttons.

The Downloads page uses the existing bento cards and styles for Windows x64,
macOS Apple Silicon, macOS Intel and Linux x64 GNU, including version/date,
sizes, independent checksums, instructions, update commands and release notes.
OS detection only highlights options. Navigation, mobile layout, source guide,
VS Code extension and canonical/redirect behavior remain. Metadata no longer
hardcodes a compiler version or describes only planned archives. Current version
structured data is scoped to the validated Downloads release.

## Schema

See [the full JSON Schema](../installer/downloads.schema.json). Schema v1 has:

- `schema_version`: integer `1`.
- `version`: canonical SemVer supported by the release pipeline (no build metadata).
- `release_date`: real `YYYY-MM-DD` date, UTC assembly day retained on reassembly.
- `installers`: exactly `windows-x64`, `macos-arm64`, `macos-x64`, `linux-x64`.
- Each installer: `name`, `filename`, trusted HTTPS `url`, integer `size` in bytes,
  and `sha256` as 64 lowercase hexadecimal characters, calculated from actual files.
- Optional `release_notes_url`: matching compiler repository release tag.

No illustrative release data or fixture installers are published by this change.
Browser fixtures are loaded only by the test process.

## File inventory

Paths below are relative to their respective repository roots.

Compiler files modified:

- `.github/workflows/release.yml`
- `README.md`
- `docs/aws-installer-releases.md`
- `installer/README.md`
- `installer/assemble.py`
- `installer/publish_s3.py`
- `installer/release_artifacts.py`
- `installer/tests/test_packaging.py`
- `installer/tests/test_release.py`

Compiler files created:

- `docs/download-integration-handoff.md` (this file)
- `installer/download_manifest.py`
- `installer/downloads.schema.json`

Website files modified:

- `.gitignore`
- `README.md`
- `app/download/page.tsx`
- `app/download/opengraph-image.tsx`
- `app/globals.css`
- `lib/structured-data.ts`
- `package.json`
- `package-lock.json`

Website files created:

- `components/os-highlight.tsx`
- `components/release-downloads.tsx`
- `lib/release-manifest.ts`
- `lib/releases.ts`
- `playwright.config.ts`
- `tests/releases.test.mjs`
- `tests/mock-cdn.mjs`
- `tests/browser/downloads.spec.ts`

## Executed verification

- Compiler installer Python suite: 41 tests passed.
- Existing SDK provisioning Python suite: 4 tests passed.
- Existing benchmark harness Python suite: 6 tests passed.
- Existing Rust installer/updater suite: 6 tests passed (`cargo test --locked --manifest-path installer/manager/Cargo.toml`).
- Website manifest suite: 7 tests passed (`npm run test`).
- Website ESLint: passed (`npm run lint`).
- Website production build: passed (`npm run build`).
- Production Playwright browser suite: 1 comprehensive test passed in about two
  minutes, covering desktop/mobile rendering, direct URLs, checksums, keyboard
  focus, SEO/navigation, redirect, live version revalidation and outage fallback.
- `git diff --check`: passed in both repositories.

Website verification used Node.js 24.18.0. Native compiler builds and Windows,
macOS and Linux clean-machine installer execution were not run during this task;
they remain gated by the existing GitHub native build workflow. AWS tests use a
fake S3 client; production uploads, permissions and CDN behavior have not been
verified by a real deployment. The test suite's fixture build is replaced with
an ordinary local production build afterward.

## Remaining setup, deployment and first-release checks

Follow the [deployment guide](aws-installer-releases.md#website-integration-and-consistency)
for exact existing-distribution Console steps, IAM additions and recovery.
In particular, add the `downloads.json` behavior with **CachingDisabled**, retain
the existing uncached `latest.json` behavior, and allow the new immutable snapshot
keys in existing IAM/OAC prefix restrictions. Keep S3 private. Versioned snapshots
retain immutable cache headers. No new bucket, distribution, DNS or permanent
credentials are needed. These Console changes were documented, not performed.

After review/merge, deploy the website through its existing Next.js host using
`npm ci`, `npm run lint`, `npm run test`, `npm run build` and the existing runtime
start/deployment command. Use Node.js 22.18+ for the provided tests; no AWS website
secrets are required. Then publish a matching reviewed compiler tag through the
existing workflow and approve its production deployment.

For the first release, GET both public manifests and compare versions and
uncached JSON headers; confirm the immutable website snapshot; HEAD all four
installer URLs and compare sizes; independently verify downloaded SHA-256 values.
Check `/download` on desktop/mobile after revalidation and verify `/downloads`
redirects. Native clean-machine checks still apply. For a failure, retry the
original retained Actions artifact. For rollback, use the documented conditional
operator restore of both manifests, leaving immutable files intact; installed
updaters intentionally refuse downgrades, so publish a higher fixed version for
existing users.

Security: all release data is untrusted, URLs are restricted, React escapes text,
and no remote HTML, arbitrary redirect, browser AWS credential, public S3 access
or automatic installer execution is introduced. Existing unsigned-installer and
notarization limitations remain visible.

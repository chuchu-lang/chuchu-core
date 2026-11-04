# Automated installer releases

## Architecture and existing infrastructure

The release workflow extends the existing native packager and Rust manager.
It never creates infrastructure or changes the website.

| Resource | Existing production value |
| --- | --- |
| S3 bucket | `chuchu-installers-prod` (private, public access blocked) |
| Region | `us-east-1` |
| CloudFront distribution name | `chuchu-installers-cdn` |
| CloudFront domain | `d36tmdxqvl259p.cloudfront.net` |
| Public download base | `https://downloads.chuchu-lang.org` |
| Active manifest | `https://downloads.chuchu-lang.org/latest.json` |
| Website download page | `https://www.chuchu-lang.org/downloads` (unchanged) |

Pull requests and pushes to `main` run quick checks without release builds.
Only a `v*` tag push or a manual release dispatch starts the release pipeline:

1. Validate canonical `vMAJOR.MINOR.PATCH[-prerelease]` syntax, exact root
   `Cargo.toml` version, exact tagged commit, and ancestry from `origin/main`.
   Build metadata (`+...`) is unsupported by the existing packager and rejected.
   A completed GitHub Release with this pipeline's tag/commit completion marker
   skips rebuilding. An unrelated published release fails closed for owner review.
2. Call `native-build.yml` for Windows x64, macOS ARM64, macOS Intel and Linux x64
   GNU. Keep the verified LLVM caches, compiler tests, debugger/sanitizer checks,
   manager tests and install/update/relocation smoke tests. Build the manager with
   the production `CHEEKU_UPDATE_URL`; packaging still checks it matches the base.
3. After all four jobs pass, assemble and validate the complete inventory, create
   or refresh a **draft** GitHub Release, and attach all tested artifacts. Save
   the exact assembled bytes as the `release-distributions` Actions artifact.
4. Wait for the `production` environment's required approval. Only the deployment
   job has `id-token: write`; it assumes the configured AWS role through OIDC.
5. Validate locally again, preflight existing S3 objects, upload missing payloads,
   managers, manifests, installers and checksums, then verify every object's S3
   SHA-256, size, content type and cache metadata. Write `latest.json` **last**.
6. Verify the small manifest with an HTTPS GET and every immutable file with
   HTTPS HEAD requests through CloudFront. Publish the GitHub Release only after
   verification succeeds. The workflow summary reports the production URLs.

No long-lived AWS keys are needed. No production resources, credentials, uploads,
GitHub releases or tags are created by running ordinary local tests.

## One-time GitHub setup (repository owner)

Before enabling the workflow on `main`:

1. Protect `main` with required reviews/checks; do not allow releases to bypass
   protection. Protect `v*` tags with a repository ruleset: restrict creation to
   release maintainers and prevent updates/deletion. Do not move a released tag.
2. In **Settings → Environments**, create or select **production**. Add required
   reviewers, enable prevention of self-review, and disable administrator bypass
   where available. Approval is a repository setting: `environment: production`
   by itself does **not** require approval. Confirm your GitHub plan supports it.
3. Select deployment branches/tags explicitly: allow branch `main` for manual
   dispatches and tag pattern `v*` for automatic releases. Do not allow arbitrary
   branches. Manually dispatch this workflow from `main` (or a protected release
   tag), supplying the existing tag as the input.
4. Under production environment **Variables**, add `AWS_RELEASE_ROLE_ARN` with
   the actual role ARN from IAM. A repository Actions variable of that name is
   also supported. The workflow uses `vars`, not `secrets`; an ARN is not a key.
   Do not add AWS access-key secrets.
5. Allow the workflow's scoped `contents: write` permission for staging and
   finalizing releases. The remaining jobs use `contents: read`.
6. Require review for changes to `.github/workflows/` and `installer/` (for
   example with CODEOWNERS). The environment-bound OIDC subject authorizes jobs
   using that environment, not one particular workflow filename. Protected code,
   deployment ref restrictions and required reviewers complete that boundary.

The implementation does not change these repository settings automatically.
An approval must refer to the validated tag/commit and its staged draft assets.

## One-time AWS IAM setup (existing account)

Use the existing AWS account and bucket; do not create another bucket,
distribution, certificate, or DNS record. Keep S3 Block Public Access and
CloudFront Origin Access Control enabled.

### Identity provider and trust policy

In **IAM → Identity providers**, reuse the GitHub provider if present. Otherwise
add an OpenID Connect provider with URL `https://token.actions.githubusercontent.com`
and audience `sts.amazonaws.com`. In **IAM → Roles**, create/select the deployment
role using that federated provider and `sts:AssumeRoleWithWebIdentity`.
Replace `ACCOUNT_ID` below with your actual AWS account ID, never a guessed ID.

The repository currently reports these settings from the read-only endpoint
`gh api repos/chuchu-lang/chuchu-core/actions/oidc/customization/sub`:

```json
{
  "use_default": true,
  "use_immutable_subject": true,
  "sub_claim_prefix": "repo:chuchu-lang@349508701/chuchu-core@1435048838"
}
```

Use this **exact environment-bound** trust policy for the current settings:

```json
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Principal": {
      "Federated": "arn:aws:iam::ACCOUNT_ID:oidc-provider/token.actions.githubusercontent.com"
    },
    "Action": "sts:AssumeRoleWithWebIdentity",
    "Condition": {
      "StringEquals": {
        "token.actions.githubusercontent.com:aud": "sts.amazonaws.com",
        "token.actions.githubusercontent.com:sub": "repo:chuchu-lang@349508701/chuchu-core@1435048838:environment:production"
      }
    }
  }]
}
```

Verify the subject settings again before configuring IAM. If the repository
explicitly switches back to legacy subjects, the environment subject becomes
`repo:chuchu-lang/chuchu-core:environment:production`; do not allow both forms
unless they are actually needed. A job with an environment has an
`environment:production` subject suffix, **not** `ref:refs/tags/v...`.
Enforce eligible refs with the production environment rules described above.
See [GitHub's AWS OIDC guide](https://docs.github.com/en/actions/how-tos/secure-your-work/security-harden-deployments/oidc-in-aws)
and [immutable subject claims](https://docs.github.com/en/actions/reference/security/oidc).

### Role permission policy

Attach this policy to the deployment role. It only allows release-object reads,
conditional writes, and bucket listing in the existing bucket. Listing allows
S3 HEAD requests to distinguish a missing object (404) from denied access (403).
No deletion, public ACL, bucket configuration, CloudFront administration or IAM
administration is needed. Versioned files can only be created when absent.
The active manifest can be created when absent or replaced with an ETag match.

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "CheckForMissingObjects",
      "Effect": "Allow",
      "Action": "s3:ListBucket",
      "Resource": "arn:aws:s3:::chuchu-installers-prod"
    },
    {
      "Sid": "ReadReleaseObjects",
      "Effect": "Allow",
      "Action": "s3:GetObject",
      "Resource": [
        "arn:aws:s3:::chuchu-installers-prod/cheeku-*",
        "arn:aws:s3:::chuchu-installers-prod/Chuchu-*",
        "arn:aws:s3:::chuchu-installers-prod/install-chuchu-*",
        "arn:aws:s3:::chuchu-installers-prod/SHA256SUMS-*",
        "arn:aws:s3:::chuchu-installers-prod/latest.json"
      ]
    },
    {
      "Sid": "CreateImmutableReleaseObjects",
      "Effect": "Allow",
      "Action": "s3:PutObject",
      "Resource": [
        "arn:aws:s3:::chuchu-installers-prod/cheeku-*",
        "arn:aws:s3:::chuchu-installers-prod/Chuchu-*",
        "arn:aws:s3:::chuchu-installers-prod/install-chuchu-*",
        "arn:aws:s3:::chuchu-installers-prod/SHA256SUMS-*"
      ],
      "Condition": {"StringEquals": {"s3:if-none-match": "*"}}
    },
    {
      "Sid": "CreateInitialManifest",
      "Effect": "Allow",
      "Action": "s3:PutObject",
      "Resource": "arn:aws:s3:::chuchu-installers-prod/latest.json",
      "Condition": {"StringEquals": {"s3:if-none-match": "*"}}
    },
    {
      "Sid": "ReplaceUnchangedManifest",
      "Effect": "Allow",
      "Action": "s3:PutObject",
      "Resource": "arn:aws:s3:::chuchu-installers-prod/latest.json",
      "Condition": {"Null": {"s3:if-match": "false"}}
    }
  ]
}
```

Use the bucket's existing encryption configuration. The policy assumes SSE-S3;
if the bucket uses SSE-KMS, an owner must review narrowly scoped KMS permissions
and the key policy before deployment, including checksum reads. Do not solve
an encryption denial by granting administrator access or changing encryption.
Role sessions default to one hour; deployment has a 60-minute timeout.

The publisher sends a SHA-256 with every PutObject and verifies S3's stored
checksum using HeadObject with `ChecksumMode=ENABLED`. It does not trust ETags
as content hashes. ETags are used solely for atomic manifest replacement.
See [S3 conditional writes](https://docs.aws.amazon.com/AmazonS3/latest/userguide/conditional-writes.html)
and [S3 upload integrity](https://docs.aws.amazon.com/AmazonS3/latest/userguide/checking-object-integrity-upload.html).

## Files and caching

Keep the exact filenames generated by `package.py`, including manager binaries,
per-platform manifests, Unix bootstraps, macOS packages and Windows setup EXEs.
Redistribution licenses remain inside the compiler payloads. Assembly also creates:

- `cheeku-<version>.json`: immutable copy of the complete manifest, for rollback.
- `SHA256SUMS-<version>`: checksums of every immutable release file, including the
  versioned manifest. Published to S3 and GitHub.
- `latest.json`: stable update channel, uploaded only after verification.
- `SHA256SUMS`: existing complete local/GitHub checksum inventory, including
  `latest.json` and the versioned checksum file. **GitHub-only**, because its
  unversioned filename would collide across releases in S3. Use the versioned
  checksum URL for public downloads. All other filenames map directly to the
  bucket root and `https://downloads.chuchu-lang.org/<filename>`.

Every expected file is required; directories, symlinks, unknown filenames, empty
files, unsupported manifests, mismatched platform manifests and corrupt checksums
are rejected before authentication/uploads. Files are capped at the manager's
existing 1 GiB download limit. SDK uploads use single PutObject requests to retain
full-object SHA-256 and conditional-create semantics, not multipart uploads.

| Object | Cache-Control | Content-Type |
| --- | --- | --- |
| Versioned files | `public, max-age=31536000, immutable` | By file type |
| `latest.json` | `no-cache, no-store, must-revalidate` | `application/json` |
| JSON manifests | Immutable | `application/json` |
| EXE, PKG, native managers | Immutable | `application/octet-stream` |
| `.sh`, `.command` | Immutable | `text/x-shellscript` |
| `.zip`, `.tar.gz` | Immutable | `application/zip`, `application/gzip` |
| Checksums | Immutable | `text/plain` |

Keep the existing CloudFront `latest.json` behavior with caching disabled.
Ensure response-header policies preserve origin Cache-Control, Content-Length
and Content-Type. Do not force caching on the manifest. No invalidation is
needed for immutable names or the uncached manifest, so the deployment role has
no invalidation permission. If the existing configuration is wrong, correct it
through the normal infrastructure review process and retry verification.

## Publish a new version

After the setup above and after this implementation is reviewed and merged:

```sh
git checkout main
git pull --ff-only origin main

# Update the root Cargo.toml version and Cargo.lock as required.
# Commit and merge that version update through the normal review process first.
# The example tag below must match the actual committed compiler version.

git tag v0.1.0
git push origin v0.1.0
```

Tag pushing is an explicit public release action. An ordinary development merge
never publishes. For a manual dispatch, select the **Tested native release**
workflow from `main`, enter an existing tag and run it. The tagged commit must
already be in protected `main`. Review all four native jobs and the staged draft
before approving the production environment deployment.

SemVer prereleases are supported and marked prerelease in GitHub. They also
advance the single `latest.json` channel when their SemVer is newer: there is no
separate prerelease update channel. Use stable tags unless that is intended.

## Verification and retry

The workflow fetches `latest.json`, validates its version/targets/domain, and
checks sizes and headers for every S3-published object through CloudFront. Large
payloads are not downloaded twice: S3 has already validated their SHA-256.
Operators can inspect a deployment without uploading anything:

```sh
curl --fail --show-error -i https://downloads.chuchu-lang.org/latest.json
curl --fail --show-error -I https://downloads.chuchu-lang.org/cheeku-0.1.0-aarch64-apple-darwin.tar.gz
# Using the exact complete local release-distributions artifact:
python3 installer/publish_s3.py --directory distributions --version 0.1.0 --validate-only
python3 installer/publish_s3.py --directory distributions --version 0.1.0 --verify-http-only
```

Use **Re-run failed jobs** on the original Actions run for deployment failures.
This reuses `release-distributions` (retained 30 days), rather than rebuilding
and changing bytes under a previously used version. Exact existing S3 objects
are reused; changed bytes or metadata cause a collision error, never overwrite.
Keep the original build artifacts for longer if your recovery process needs it.
A missing/expired artifact requires restoring the exact original bytes; otherwise
use a new version. Do not work around a collision by deleting a released object.

Per-tag workflow concurrency prevents duplicate work on one tag. A global
production deployment concurrency group serializes different tags (GitHub may
replace older pending jobs; rerun a cancelled pending job if still needed).
Conditional S3 creation and ETag replacement protect against external races.
If another release changes the manifest during publishing, the old job fails;
on retry it refuses to downgrade a newer SemVer. This is fail-closed rather than
an automatic overwrite/retry loop. A published GitHub Release with the exact
pipeline completion marker skips new builds on another dispatch of the same tag.

Failure before writing `latest.json` leaves the active release untouched.
If S3 succeeds but CloudFront or GitHub finalization fails, the new complete
release may already be active, while GitHub stays draft. Retry the failed job;
identical bytes are accepted and GitHub is finalized only after verification.
CloudFront verification failures do not automatically roll back an active
manifest, since that could overwrite a concurrent newer release.

## Roll back the active manifest (explicit operator action)

Prefer a fixed **higher** version: installed managers intentionally ignore
older/equal versions. Reverting `latest.json` only affects new installations and
clients below that version; it does not downgrade existing installations.

To roll back for new installations during an incident:

1. Stop/disable pending production deployments and coordinate with release
   maintainers. Use an authorized operator session (for example AWS SSO), not
   permanent access keys. This is a manual exception to the publisher's downgrade
   protection, not a normal workflow dispatch.
2. Download the old GitHub Release's complete artifacts into `rollback/` using
   `gh release download v0.1.0 --repo chuchu-lang/chuchu-core --dir rollback`.
   Verify with `python3 installer/publish_s3.py --directory rollback --version
   0.1.0 --validate-only` (on one line). Check the immutable files still exist in
   S3 with the correct checksum metadata, and that their public URLs respond.
3. Fetch and record the **current** S3 manifest ETag, then conditionally replace
   only `latest.json` with the verified old snapshot:

```sh
ACTIVE_ETAG=$(aws s3api head-object --bucket chuchu-installers-prod \
  --key latest.json --region us-east-1 --query ETag --output text)
SNAPSHOT=rollback/cheeku-0.1.0.json
SNAPSHOT_SHA256=$(openssl dgst -sha256 -binary "$SNAPSHOT" | openssl base64 -A)
aws s3api put-object --bucket chuchu-installers-prod --key latest.json \
  --region us-east-1 --body "$SNAPSHOT" --if-match "$ACTIVE_ETAG" \
  --content-type application/json \
  --cache-control 'no-cache, no-store, must-revalidate' \
  --checksum-sha256 "$SNAPSHOT_SHA256"
```

4. Verify the CloudFront manifest, record the rollback in GitHub Release notes
   and the incident log, and suspend a known-bad release's distribution as
   appropriate. Do not delete or change immutable files. A failed precondition
   means someone changed the active manifest: investigate rather than force it.
5. Release the fix with a higher version through normal review and approval.

The deployment IAM role has no delete permission, and this task does not enable
bucket versioning or lifecycle policies. Existing snapshots provide rollback
without requiring additional AWS infrastructure.

## Costs and troubleshooting

Costs depend on artifact sizes, retained versions, request counts, geographic
traffic, and the account's CloudFront plan/free allowances. With little traffic,
S3 storage is the recurring cost; at scale, large binary downloads and CDN
requests drive usage. Model storage as retained release count × total release
bytes; estimate download traffic as downloads × average payload bytes. Public
traffic can be much larger than CI upload traffic. HEAD verification avoids
redownloading large archives. No invalidations or extra compute services are
introduced. GitHub native runner minutes and retained Actions artifacts are
separate cost drivers, especially LLVM cache misses and macOS jobs. Use current
[S3 pricing](https://aws.amazon.com/s3/pricing/) and
[CloudFront pricing](https://aws.amazon.com/cloudfront/pricing/) with your actual
traffic estimates; this implementation does not assume a dollar budget or a
free tier. Configure billing alerts through your normal account process.

| Failure | Action |
| --- | --- |
| Invalid tag/version or commit outside main | Merge the reviewed version first; create the correct new tag. Never move a released tag. |
| Missing `AWS_RELEASE_ROLE_ARN` | Set the production environment variable to the real IAM role ARN. |
| OIDC access denied | Verify actual immutable subject prefix, `environment:production`, audience, provider account and GitHub environment restrictions. |
| S3 403 | Check bucket-scoped IAM, ListBucket for missing-object checks, existing bucket policy and encryption/KMS requirements. Do not change public access/OAC. |
| Missing platform or bad checksum | Inspect failed native jobs/artifact transfer. No manifest is published. |
| Object collision | Restore exact original artifacts/metadata or use a new version. Do not overwrite immutable objects. |
| S3 409/412 | Another writer raced; inspect current manifest and retry the failed job only if appropriate. |
| Refusing downgrade | A newer version is already active. Keep it; release a higher fixed version if needed. |
| Wrong CloudFront headers/size or stale manifest | Check existing cache/response policies and origin path; keep the GitHub release draft until verification passes. |
| S3 succeeded but GitHub failed | Rerun failed deployment job with the original artifacts; verify the live version before finalizing. |
| Missing required reviewers | Configure environment protection before using the release workflow. YAML cannot enforce a reviewer setting. |

Ordinary tests use a fake S3 client and require no AWS credentials. Native
GitHub Actions jobs remain the final platform validation. Public signing and
notarization, clean-machine checks and broader Linux ABI support remain existing
release requirements; this pipeline does not claim to implement them.

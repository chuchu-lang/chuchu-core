#!/usr/bin/env python3
"""Tag validation and retryable GitHub draft/final release management."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tomllib
import urllib.error
import urllib.parse
import urllib.request

from release_artifacts import BASE_URL, validate_release, validate_tag

REPOSITORY = 'chuchu-lang/chuchu-core'
MARKER = '<!-- chuchu-s3-deployed:'


def gh(*args):
    return subprocess.check_output(['gh', *args, '--repo', REPOSITORY], text=True).strip()


def get_release(tag):
    token = os.environ.get('GH_TOKEN') or os.environ.get('GITHUB_TOKEN')
    if not token:
        raise ValueError('GH_TOKEN is required for release state checks')
    def fetch(suffix):
        request = urllib.request.Request(
            f'https://api.github.com/repos/{REPOSITORY}/releases{suffix}',
            headers={'Authorization': 'Bearer ' + token, 'Accept': 'application/vnd.github+json',
                     'X-GitHub-Api-Version': '2022-11-28'})
        with urllib.request.urlopen(request, timeout=60) as response:
            return json.load(response)
    try:
        return fetch('/tags/' + urllib.parse.quote(tag, safe=''))
    except urllib.error.HTTPError as error:
        error.close()
        if error.code != 404:
            raise
    # The tag endpoint documents published releases only. Authenticated listing
    # also finds drafts, which is necessary both for upload retries and finalize.
    page = 1
    while True:
        releases = fetch(f'?per_page=100&page={page}')
        for release in releases:
            if release['tag_name'] == tag:
                return release
        if len(releases) < 100:
            return None
        page += 1


def marker(tag, commit):
    return f'{MARKER}{tag}:{commit} -->'


def completed(release, tag, commit):
    if release and not release['draft']:
        if marker(tag, commit) not in (release.get('body') or ''):
            raise ValueError('An unmanaged published GitHub release already exists; review it manually')
        return True
    return False


def validate_checkout(tag):
    version = tomllib.loads(Path('Cargo.toml').read_text())['package']['version']
    validate_tag(tag, version)
    commit = subprocess.check_output(['git', 'rev-parse', '--verify', f'refs/tags/{tag}^{{commit}}'], text=True).strip()
    head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()
    if head != commit:
        raise ValueError('Checkout is not the exact tagged commit')
    subprocess.run(['git', 'fetch', '--no-tags', 'origin', '+refs/heads/main:refs/remotes/origin/main'], check=True)
    if subprocess.run(['git', 'merge-base', '--is-ancestor', commit, 'refs/remotes/origin/main']).returncode:
        raise ValueError('Tagged commit is not reachable from protected main')
    return commit, version


def draft(tag, commit, directory):
    validate_release(directory, tag[1:])
    release = get_release(tag)
    if completed(release, tag, commit):
        raise ValueError('Release is already complete; refusing to modify its assets')
    if release is None:
        gh('release', 'create', tag, '--verify-tag', '--draft', '--title', tag,
           '--notes-file', 'docs/release-runtime.md')
    # A failed draft upload may be safely replaced; a published release may not.
    gh('release', 'upload', tag, *[str(p) for p in sorted(directory.iterdir())], '--clobber')


def finalize(tag, commit, directory):
    # Called only after S3 and CloudFront verification. Re-check here as well so
    # an accidental standalone invocation cannot mark an unverified release done.
    from publish_s3 import verify_http
    verify_http(directory, tag[1:])
    release = get_release(tag)
    if release is None:
        raise ValueError('Missing staged GitHub draft')
    if completed(release, tag, commit):
        return
    notes = (release.get('body') or '') + f'\n\nDownloads: {BASE_URL}\nUpdate manifest: {BASE_URL}/latest.json\n\n' + marker(tag, commit) + '\n'
    import tempfile
    with tempfile.TemporaryDirectory() as temporary:
        path = Path(temporary) / 'notes.md'
        path.write_text(notes)
        # SemVer prereleases are explicitly marked; GitHub must not promote an
        # older historical release above another release by publication time.
        gh('release', 'edit', tag, '--draft=false', '--latest=false',
           '--prerelease=' + str('-' in tag).lower(), '--notes-file', str(path))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['validate', 'draft', 'finalize'])
    parser.add_argument('--directory', type=Path, default=Path('distributions'))
    args = parser.parse_args()
    tag = os.environ.get('RELEASE_TAG', '')
    commit, version = validate_checkout(tag)
    if args.command == 'validate':
        skip = completed(get_release(tag), tag, commit)
        output = f'commit={commit}\ntag={tag}\nversion={version}\nskip={str(skip).lower()}\n'
        if os.environ.get('GITHUB_OUTPUT'):
            with open(os.environ['GITHUB_OUTPUT'], 'a') as stream:
                stream.write(output)
        print(output, end='')
    elif args.command == 'draft':
        draft(tag, commit, args.directory)
    else:
        finalize(tag, commit, args.directory)


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print(f'Release control failed: {error}', file=sys.stderr)
        sys.exit(1)

#!/usr/bin/env python3
"""Validate, upload immutable files, verify, then conditionally publish latest.json."""
import argparse
import base64
import json
import os
from pathlib import Path
import re
import sys
import urllib.request

from package import digest
from release_artifacts import (BASE_URL, BUCKET, IMMUTABLE, NO_CACHE, REGION,
                               content_type, validate_manifest, validate_release, version_key)


def error_code(error):
    return str(getattr(error, 'response', {}).get('Error', {}).get('Code', ''))


def optional(call, **kwargs):
    try:
        return call(**kwargs)
    except Exception as error:
        if error_code(error) in {'NoSuchKey', '404', 'NotFound'}:
            return None
        raise


def checksum(path):
    return base64.b64encode(bytes.fromhex(digest(path))).decode('ascii')


def verify_object(s3, key, path, cache):
    head = optional(s3.head_object, Bucket=BUCKET, Key=key, ChecksumMode='ENABLED')
    if head is None:
        return False
    if (head.get('ContentLength') != path.stat().st_size
            or head.get('ChecksumSHA256') != checksum(path)
            or head.get('CacheControl') != cache
            or head.get('ContentType') != content_type(key)):
        raise ValueError(f'S3 object collision or incorrect metadata: {key}; existing object was not overwritten')
    return True


def put(s3, key, path, cache, **condition):
    with path.open('rb') as stream:
        s3.put_object(Bucket=BUCKET, Key=key, Body=stream,
                      ContentLength=path.stat().st_size,
                      ContentType=content_type(key), CacheControl=cache,
                      ChecksumSHA256=checksum(path), **condition)


def active_manifest(s3):
    obj = optional(s3.get_object, Bucket=BUCKET, Key='latest.json')
    if obj is None:
        return None, None
    body = obj['Body']
    try:
        data = body.read(1024 * 1024 + 1)
    finally:
        body.close()
    if len(data) > 1024 * 1024:
        raise ValueError('Active manifest is too large')
    manifest = json.loads(data)
    validate_manifest(manifest, manifest['version'])
    return manifest, obj['ETag']


def publish(s3, directory, version):
    manifest = validate_release(directory, version)
    current, etag = active_manifest(s3)
    if current and version_key(current['version']) > version_key(version):
        raise ValueError('Refusing to downgrade latest.json')
    if current and current['version'] == version and current != manifest:
        raise ValueError('This version is already active with a different manifest')
    # SHA256SUMS includes mutable latest.json. Its immutable counterpart contains
    # the versioned snapshot instead and is the checksum file served by S3.
    def upload_order(path):
        name = path.name
        if name.endswith(('.zip', '.tar.gz')):
            return (0, name)
        if name.startswith('cheeku-manager-'):
            return (1, name)
        if name.endswith('.json'):
            return (2, name)
        return (4 if name.startswith('SHA256SUMS') else 3, name)
    files = sorted((p for p in directory.iterdir() if p.name not in {'latest.json', 'SHA256SUMS'}), key=upload_order)
    # Check every collision before writing anything; retries only reuse exact bytes.
    exists = {p.name: verify_object(s3, p.name, p, IMMUTABLE) for p in files}
    for path in files:
        if not exists[path.name]:
            try:
                put(s3, path.name, path, IMMUTABLE, IfNoneMatch='*')
            except Exception as error:
                if error_code(error) not in {'PreconditionFailed', '412'}:
                    raise
                if not verify_object(s3, path.name, path, IMMUTABLE):
                    raise
    for path in files:
        if not verify_object(s3, path.name, path, IMMUTABLE):
            raise ValueError(f'Uploaded object is missing: {path.name}')
    latest = directory / 'latest.json'
    if not current or current['version'] != version:
        # S3 checks this atomically. A concurrent update fails closed; retrying
        # reads the active version again and never downgrades a newer release.
        put(s3, 'latest.json', latest, NO_CACHE,
            **({'IfMatch': etag} if etag else {'IfNoneMatch': '*'}))
    if not verify_object(s3, 'latest.json', latest, NO_CACHE):
        raise ValueError('Published latest.json is missing')
    active, _ = active_manifest(s3)
    if active != manifest:
        raise ValueError('The active manifest changed during verification')
    return manifest


class HTTPSOnly(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if not newurl.startswith(BASE_URL + '/'):
            raise ValueError('Download redirected outside the production domain')
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def verify_http(directory, version, opener=None):
    """GET the small manifest, HEAD every immutable object; S3 verified the bytes."""
    manifest = validate_release(directory, version)
    opener = opener or urllib.request.build_opener(HTTPSOnly())
    with opener.open(BASE_URL + '/latest.json', timeout=60) as response:
        data = response.read(1024 * 1024 + 1)
        if len(data) > 1024 * 1024 or json.loads(data) != manifest:
            raise ValueError('CloudFront latest.json does not match this release')
        if response.headers.get('Cache-Control') != NO_CACHE:
            raise ValueError('Incorrect latest.json Cache-Control')
        if response.headers.get('Content-Type', '').split(';')[0] != 'application/json':
            raise ValueError('Incorrect latest.json Content-Type')
    for path in sorted(directory.iterdir()):
        if path.name in {'SHA256SUMS', 'latest.json'}:
            continue
        request = urllib.request.Request(BASE_URL + '/' + path.name, method='HEAD')
        with opener.open(request, timeout=60) as response:
            if (int(response.headers.get('Content-Length', -1)) != path.stat().st_size
                    or response.headers.get('Cache-Control') != IMMUTABLE
                    or response.headers.get('Content-Type', '').split(';')[0] != content_type(path.name)):
                raise ValueError(f'CloudFront size/cache/type mismatch: {path.name}')
    return manifest


def check_aws_config(role):
    if not re.fullmatch(r'arn:aws:iam::[0-9]{12}:role/[A-Za-z0-9_+=,.@/-]+', role or ''):
        raise ValueError('Set production variable AWS_RELEASE_ROLE_ARN to the actual IAM role ARN')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path)
    parser.add_argument('--version')
    parser.add_argument('--check-config', action='store_true')
    parser.add_argument('--validate-only', action='store_true')
    parser.add_argument('--verify-http-only', action='store_true')
    args = parser.parse_args()
    if args.check_config:
        check_aws_config(os.environ.get('AWS_RELEASE_ROLE_ARN'))
        return
    if not args.directory or not args.version:
        parser.error('--directory and --version are required')
    if args.validate_only:
        validate_release(args.directory, args.version)
        print('Release inventory, manifests, sizes and checksums verified')
        return
    if args.verify_http_only:
        verify_http(args.directory, args.version)
    else:
        check_aws_config(os.environ.get('AWS_RELEASE_ROLE_ARN'))
        import boto3
        from botocore.config import Config
        session = boto3.Session(region_name=REGION)
        if session.get_credentials() is None:
            raise ValueError('No AWS credentials; assume the production role using GitHub OIDC')
        s3 = session.client('s3', config=Config(retries={'mode': 'standard', 'max_attempts': 5},
                                              connect_timeout=30, read_timeout=300))
        publish(s3, args.directory, args.version)
        verify_http(args.directory, args.version)
    print(f'Verified release {args.version}: {BASE_URL}/latest.json')
    for name in sorted(p.name for p in args.directory.iterdir() if p.name != 'SHA256SUMS'):
        print(f'{BASE_URL}/{name}')


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print(f'Release publishing failed: {error}', file=sys.stderr)
        sys.exit(1)

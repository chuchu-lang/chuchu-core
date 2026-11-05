import base64
import copy
import hashlib
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
import urllib.error
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from assemble import assemble
from package import TARGETS, asset
from publish_s3 import check_aws_config, publish, verify_http
from release_artifacts import (BASE_URL, BUCKET, IMMUTABLE, NO_CACHE, content_type,
                               expected_files, validate_release, validate_tag, version_key)
from release_control import completed, draft, finalize, get_release, marker, validate_checkout


def fixture(directory, version='0.1.0'):
    directory.mkdir(exist_ok=True)
    for name in expected_files(version) - {'latest.json', 'downloads.json', f'downloads-{version}.json', 'SHA256SUMS', f'SHA256SUMS-{version}', f'cheeku-{version}.json'}:
        (directory / name).write_bytes(('tested ' + name).encode())
    for target, system in TARGETS.items():
        stem = f'cheeku-{version}-{target}'
        archive = directory / (stem + ('.zip' if system == 'Windows' else '.tar.gz'))
        manifest = {'schema_version': 1, 'version': version, 'update_url': BASE_URL + '/latest.json',
                    'platforms': {target: asset(archive, BASE_URL)}}
        (directory / (stem + '.json')).write_text(json.dumps(manifest))
    assemble(directory, version, BASE_URL)
    return directory


class S3Error(Exception):
    def __init__(self, code):
        self.response = {'Error': {'Code': code}}
        super().__init__(code)


class FakeS3:
    """Enforce the real checksum/conditional-write contract without AWS or boto3."""
    def __init__(self):
        self.objects = {}
        self.writes = []
        self.fail_key = None
        self.corrupt_head = None
        self.before_latest = None
        self.before_downloads = None

    def head_object(self, Bucket, Key, ChecksumMode):
        assert Bucket == BUCKET and ChecksumMode == 'ENABLED'
        if Key not in self.objects:
            raise S3Error('404')
        result = {k: v for k, v in self.objects[Key].items() if k != 'data'}
        if Key == self.corrupt_head:
            result['ChecksumSHA256'] = 'corrupted'
        return result

    def get_object(self, Bucket, Key):
        assert Bucket == BUCKET
        if Key not in self.objects:
            raise S3Error('NoSuchKey')
        obj = self.objects[Key]
        return dict(obj, Body=io.BytesIO(obj['data']))

    def put_object(self, **args):
        assert args['Bucket'] == BUCKET
        key = args['Key']
        if key == self.fail_key:
            raise S3Error('ServiceUnavailable')
        if key == 'latest.json' and self.before_latest:
            callback, self.before_latest = self.before_latest, None
            callback()
        if key == 'downloads.json' and self.before_downloads:
            callback, self.before_downloads = self.before_downloads, None
            callback()
        existing = self.objects.get(key)
        if args.get('IfNoneMatch') == '*' and existing:
            raise S3Error('PreconditionFailed')
        if 'IfMatch' in args and (not existing or args['IfMatch'] != existing['ETag']):
            raise S3Error('PreconditionFailed')
        assert 'IfNoneMatch' in args or 'IfMatch' in args
        data = args['Body'].read()
        checksum = base64.b64encode(hashlib.sha256(data).digest()).decode()
        assert checksum == args['ChecksumSHA256'] and len(data) == args['ContentLength']
        self.objects[key] = {'data': data, 'ContentLength': len(data), 'ChecksumSHA256': checksum,
                             'ETag': '"' + hashlib.sha256(data).hexdigest() + '"',
                             'ContentType': args['ContentType'], 'CacheControl': args['CacheControl']}
        self.writes.append(key)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = fixture(Path(self.temp.name) / 'release')
        self.s3 = FakeS3()

    def test_website_manifest_uses_actual_installers(self):
        from download_manifest import INSTALLERS
        data = json.loads((self.directory / 'downloads.json').read_text())
        self.assertEqual(data['version'], json.loads((self.directory / 'latest.json').read_text())['version'])
        for platform, (_, template) in INSTALLERS.items():
            filename = template.format(version='0.1.0')
            entry = data['installers'][platform]
            self.assertEqual(entry['filename'], filename)
            self.assertEqual(entry['url'], BASE_URL + '/' + filename)
            self.assertEqual(entry['size'], (self.directory / filename).stat().st_size)
            self.assertEqual(entry['sha256'], hashlib.sha256((self.directory / filename).read_bytes()).hexdigest())

    def test_website_boolean_schema_is_not_an_integer(self):
        path = self.directory / 'downloads.json'
        data = json.loads(path.read_text())
        data['schema_version'] = True
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, 'Website manifest'):
            publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(self.s3.writes, [])

    def test_website_version_mismatch_rejected(self):
        path = self.directory / 'downloads.json'
        data = json.loads(path.read_text())
        data['version'] = '9.0.0'
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, 'Website manifest'):
            publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(self.s3.writes, [])

    def test_website_mirror_failure_has_committed_complete_snapshot_and_retry_repairs(self):
        publish(self.s3, self.directory, '0.1.0')
        newer = fixture(Path(self.temp.name) / 'newer', '0.2.0')
        self.s3.fail_key = 'downloads.json'
        with self.assertRaises(S3Error):
            publish(self.s3, newer, '0.2.0')
        self.assertEqual(json.loads(self.s3.objects['downloads.json']['data'])['version'], '0.1.0')
        self.assertEqual(json.loads(self.s3.objects['latest.json']['data'])['version'], '0.2.0')
        self.assertIn('downloads-0.2.0.json', self.s3.objects)
        self.s3.fail_key = None
        publish(self.s3, newer, '0.2.0')
        self.assertEqual(json.loads(self.s3.objects['downloads.json']['data'])['version'], '0.2.0')

    def test_concurrent_newer_mirror_cannot_be_overwritten(self):
        publish(self.s3, self.directory, '0.1.0')
        middle = fixture(Path(self.temp.name) / 'middle', '0.2.0')
        newest = fixture(Path(self.temp.name) / 'newest', '0.3.0')
        self.s3.before_downloads = lambda: publish(self.s3, newest, '0.3.0')
        with self.assertRaisesRegex(S3Error, 'PreconditionFailed'):
            publish(self.s3, middle, '0.2.0')
        for key in ['latest.json', 'downloads.json']:
            self.assertEqual(json.loads(self.s3.objects[key]['data'])['version'], '0.3.0')

    def test_missing_installer_prevents_assembly(self):
        (self.directory / 'Chuchu-0.1.0-aarch64-apple-darwin.pkg').unlink()
        with self.assertRaisesRegex(ValueError, 'Missing or unsafe installer'):
            assemble(self.directory, '0.1.0', BASE_URL)

    def test_valid_tags_and_semver_ordering(self):
        for version in ['0.1.0', '1.2.3', '1.2.3-rc.1', '2.0.0-alpha-beta']:
            self.assertEqual(validate_tag('v' + version, version), version)
        versions = ['1.0.0-alpha', '1.0.0-alpha.1', '1.0.0-alpha.beta', '1.0.0-beta',
                    '1.0.0-beta.2', '1.0.0-beta.11', '1.0.0-rc.1', '1.0.0', '1.0.1', '1.10.0']
        self.assertEqual(sorted(reversed(versions), key=version_key), versions)

    def test_reject_invalid_and_mismatched_tags(self):
        for tag in ['0.1.0', 'v01.1.0', 'v1.0', 'v1.0.0-01', 'v1.0.0+build', 'v1.0.0/', 'v1.0.0-']:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                validate_tag(tag, tag.removeprefix('v'))
        with self.assertRaisesRegex(
                ValueError, r"Release tag 'v0\.1\.1'.*version '0\.1\.0' \(expected tag: v0\.1\.0\)"):
            validate_tag('v0.1.1', '0.1.0')

    def test_complete_release_and_idempotent_assembly(self):
        first = {p.name: p.read_bytes() for p in self.directory.iterdir()}
        validate_release(self.directory, '0.1.0')
        assemble(self.directory, '0.1.0', BASE_URL)
        self.assertEqual(first, {p.name: p.read_bytes() for p in self.directory.iterdir()})

    def test_missing_platform_artifact_prevents_any_upload(self):
        (self.directory / 'Chuchu-Setup-0.1.0-windows-x64.exe').unlink()
        with self.assertRaisesRegex(ValueError, 'missing'):
            publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(self.s3.writes, [])

    def test_payload_corruption_prevents_any_upload(self):
        path = next(self.directory.glob('*.zip'))
        path.write_bytes(b'corrupt')
        with self.assertRaisesRegex(ValueError, 'checksum/size'):
            publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(self.s3.writes, [])

    def test_incorrect_manifest_payload_size_is_rejected(self):
        path = self.directory / 'latest.json'
        manifest = json.loads(path.read_text())
        manifest['platforms'][next(iter(TARGETS))]['size'] += 1
        path.write_text(json.dumps(manifest))
        (self.directory / 'cheeku-0.1.0.json').write_bytes(path.read_bytes())
        with self.assertRaisesRegex(ValueError, 'checksum/size'):
            publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(self.s3.writes, [])

    def test_installer_corruption_is_checked_too(self):
        (self.directory / 'Chuchu-Setup-0.1.0-windows-x64.exe').write_bytes(b'corrupt')
        with self.assertRaisesRegex(ValueError, 'Website manifest|Checksum mismatch'):
            publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(self.s3.writes, [])

    def test_checksum_inventory_cannot_hide_artifacts(self):
        (self.directory / 'SHA256SUMS-0.1.0').write_text('')
        with self.assertRaisesRegex(ValueError, 'Unsafe, empty'):
            publish(self.s3, self.directory, '0.1.0')

    def test_incorrect_artifact_domain_is_rejected(self):
        latest = self.directory / 'latest.json'
        manifest = json.loads(latest.read_text())
        manifest['platforms'][next(iter(TARGETS))]['url'] = 'https://evil.example/file'
        latest.write_text(json.dumps(manifest))
        with self.assertRaisesRegex(ValueError, 'artifact metadata'):
            publish(self.s3, self.directory, '0.1.0')

    def test_wrong_update_channel_is_rejected(self):
        path = self.directory / 'latest.json'
        data = json.loads(path.read_text())
        data['update_url'] = 'https://wrong.example/latest.json'
        path.write_text(json.dumps(data))
        with self.assertRaisesRegex(ValueError, 'channel'):
            publish(self.s3, self.directory, '0.1.0')

    def test_native_manifest_schema_must_also_be_manager_compatible(self):
        path = self.directory / f'cheeku-0.1.0-{next(iter(TARGETS))}.json'
        native = json.loads(path.read_text())
        native['schema_version'] = True
        path.write_text(json.dumps(native))
        with self.assertRaisesRegex(ValueError, 'schema'):
            publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(self.s3.writes, [])

    def test_unsafe_or_unexpected_files_are_rejected(self):
        path = self.directory / 'unexpected.txt'
        path.write_text('unexpected')
        with self.assertRaises(ValueError):
            validate_release(self.directory, '0.1.0')
        path.unlink()
        path = next(self.directory.glob('*.zip'))
        path.unlink()
        path.symlink_to(self.directory / 'latest.json')
        with self.assertRaisesRegex(ValueError, 'Unsafe'):
            validate_release(self.directory, '0.1.0')

    def test_success_manifest_last_and_cache_metadata(self):
        result = publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(result['version'], '0.1.0')
        self.assertEqual(self.s3.writes[-2:], ['latest.json', 'downloads.json'])
        self.assertEqual(set(self.s3.objects), expected_files('0.1.0') - {'SHA256SUMS'})
        for name, obj in self.s3.objects.items():
            self.assertEqual(obj['CacheControl'], NO_CACHE if name in {'latest.json', 'downloads.json'} else IMMUTABLE)
            self.assertEqual(obj['ContentType'], content_type(name))
        self.assertTrue(self.s3.writes[0].endswith(('.zip', '.tar.gz')))

    def test_retry_is_idempotent(self):
        publish(self.s3, self.directory, '0.1.0')
        writes = self.s3.writes.copy()
        publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(writes, self.s3.writes)

    def test_upload_failure_preserves_active_release_and_can_retry(self):
        publish(self.s3, self.directory, '0.1.0')
        old = copy.deepcopy(self.s3.objects['latest.json'])
        newer = fixture(Path(self.temp.name) / 'new', '0.2.0')
        self.s3.fail_key = 'Chuchu-Setup-0.2.0-windows-x64.exe'
        with self.assertRaises(S3Error):
            publish(self.s3, newer, '0.2.0')
        self.assertEqual(old, self.s3.objects['latest.json'])
        self.s3.fail_key = None
        publish(self.s3, newer, '0.2.0')
        self.assertEqual(json.loads(self.s3.objects['latest.json']['data'])['version'], '0.2.0')

    def test_failed_initial_upload_does_not_create_latest(self):
        self.s3.fail_key = next(self.directory.glob('*.zip')).name
        with self.assertRaises(S3Error):
            publish(self.s3, self.directory, '0.1.0')
        self.assertNotIn('latest.json', self.s3.objects)

    def test_failed_final_manifest_upload_preserves_previous_active_manifest(self):
        publish(self.s3, self.directory, '0.1.0')
        previous = copy.deepcopy(self.s3.objects['latest.json'])
        newer = fixture(Path(self.temp.name) / 'newer', '0.2.0')
        self.s3.fail_key = 'latest.json'
        with self.assertRaises(S3Error):
            publish(self.s3, newer, '0.2.0')
        self.assertEqual(self.s3.objects['latest.json'], previous)
        self.s3.fail_key = None
        publish(self.s3, newer, '0.2.0')
        self.assertEqual(json.loads(self.s3.objects['latest.json']['data'])['version'], '0.2.0')

    def test_published_object_collision_is_rejected_before_writes(self):
        publish(self.s3, self.directory, '0.1.0')
        self.s3.objects.pop('latest.json')
        self.s3.objects[next(self.directory.glob('*.zip')).name]['ChecksumSHA256'] = 'bad'
        self.s3.writes.clear()
        with self.assertRaisesRegex(ValueError, 'collision'):
            publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(self.s3.writes, [])

    def test_server_verification_failure_keeps_latest_unchanged(self):
        original = self.s3.put_object
        def corrupt_after_put(**kwargs):
            original(**kwargs)
            if kwargs['Key'].endswith('.zip'):
                self.s3.corrupt_head = kwargs['Key']
        self.s3.put_object = corrupt_after_put
        with self.assertRaisesRegex(ValueError, 'collision'):
            publish(self.s3, self.directory, '0.1.0')
        self.assertNotIn('latest.json', self.s3.objects)

    def test_prevent_downgrade(self):
        newer = fixture(Path(self.temp.name) / 'new', '1.0.0')
        publish(self.s3, newer, '1.0.0')
        before = self.s3.writes.copy()
        with self.assertRaisesRegex(ValueError, 'downgrade'):
            publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(self.s3.writes, before)

    def test_conditional_update_cannot_overwrite_concurrent_newer_release(self):
        publish(self.s3, self.directory, '0.1.0')
        middle = fixture(Path(self.temp.name) / 'middle', '0.2.0')
        newest = fixture(Path(self.temp.name) / 'newest', '0.3.0')
        self.s3.before_latest = lambda: publish(self.s3, newest, '0.3.0')
        with self.assertRaisesRegex(S3Error, 'PreconditionFailed'):
            publish(self.s3, middle, '0.2.0')
        self.assertEqual(json.loads(self.s3.objects['latest.json']['data'])['version'], '0.3.0')

    def test_first_publish_uses_conditional_create(self):
        newest = fixture(Path(self.temp.name) / 'newest', '0.3.0')
        self.s3.before_latest = lambda: publish(self.s3, newest, '0.3.0')
        with self.assertRaisesRegex(S3Error, 'PreconditionFailed'):
            publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(json.loads(self.s3.objects['latest.json']['data'])['version'], '0.3.0')

    def test_missing_aws_configuration(self):
        for role in [None, '', 'arn:aws:iam::ACCOUNT_ID:role/release', 'not-an-arn']:
            with self.assertRaises(ValueError):
                check_aws_config(role)
        # Test-only ARN, never a deployment value.
        check_aws_config('arn:aws:iam::123456789012:role/test')

    def test_s3_permission_errors_are_not_treated_as_missing(self):
        with patch.object(self.s3, 'get_object', side_effect=S3Error('AccessDenied')):
            with self.assertRaisesRegex(S3Error, 'AccessDenied'):
                publish(self.s3, self.directory, '0.1.0')
        self.assertEqual(self.s3.writes, [])

    def test_cloudfront_verifies_all_objects_without_getting_large_archives(self):
        publish(self.s3, self.directory, '0.1.0')
        requests = []
        class Opener:
            def open(inner, request, timeout):
                url = request if isinstance(request, str) else request.full_url
                requests.append((url, 'GET' if isinstance(request, str) else request.method))
                obj = self.s3.objects[url.rsplit('/', 1)[1]]
                response = io.BytesIO(obj['data'])
                response.headers = {'Cache-Control': obj['CacheControl'], 'Content-Type': obj['ContentType'],
                                    'Content-Length': str(obj['ContentLength'])}
                return response
        verify_http(self.directory, '0.1.0', Opener())
        self.assertEqual(sum(method == 'GET' for _, method in requests), 2)
        self.assertEqual(len(requests), len(self.s3.objects))
        self.s3.objects['latest.json']['CacheControl'] = IMMUTABLE
        with self.assertRaisesRegex(ValueError, 'Cache-Control'):
            verify_http(self.directory, '0.1.0', Opener())

    def test_completion_marker_prevents_rebuilding_completed_releases(self):
        self.assertFalse(completed(None, 'v0.1.0', 'commit'))
        self.assertFalse(completed({'draft': True}, 'v0.1.0', 'commit'))
        self.assertTrue(completed({'draft': False, 'body': marker('v0.1.0', 'commit')}, 'v0.1.0', 'commit'))
        with self.assertRaisesRegex(ValueError, 'unmanaged'):
            completed({'draft': False, 'body': 'unrelated'}, 'v0.1.0', 'commit')

    def test_draft_creation_and_retry(self):
        with patch('release_control.get_release', return_value=None), patch('release_control.gh') as gh:
            draft('v0.1.0', 'commit', self.directory)
            self.assertEqual(gh.call_args_list[0].args[:3], ('release', 'create', 'v0.1.0'))
            self.assertEqual(gh.call_args_list[1].args[:3], ('release', 'upload', 'v0.1.0'))
        with patch('release_control.get_release', return_value={'draft': True}), patch('release_control.gh') as gh:
            draft('v0.1.0', 'commit', self.directory)
            self.assertEqual(gh.call_count, 1)
        with patch('release_control.get_release', return_value={'draft': False, 'body': marker('v0.1.0', 'commit')}), patch('release_control.gh') as gh:
            with self.assertRaises(ValueError):
                draft('v0.1.0', 'commit', self.directory)
            gh.assert_not_called()

    def test_draft_lookup_falls_back_to_paginated_authenticated_list(self):
        missing = urllib.error.HTTPError('https://api.github.com/test', 404, 'missing', {}, None)
        draft_release = {'tag_name': 'v0.1.0', 'draft': True}
        first_page = [{'tag_name': 'v9.0.0'}] * 100
        responses = [missing, io.BytesIO(json.dumps(first_page).encode()),
                     io.BytesIO(json.dumps([draft_release]).encode())]
        with patch.dict('os.environ', {'GH_TOKEN': 'test-only'}), patch('release_control.urllib.request.urlopen', side_effect=responses) as open_url:
            self.assertEqual(get_release('v0.1.0'), draft_release)
            self.assertIn('page=2', open_url.call_args.args[0].full_url)

    def test_github_errors_cannot_be_mistaken_for_absent_releases(self):
        denied = urllib.error.HTTPError('https://api.github.com/test', 403, 'denied', {}, None)
        with patch.dict('os.environ', {'GH_TOKEN': 'test-only'}), patch('release_control.urllib.request.urlopen', side_effect=denied):
            with self.assertRaises(urllib.error.HTTPError):
                get_release('v0.1.0')

    def test_github_release_stays_draft_if_deployment_verification_fails(self):
        with patch('publish_s3.verify_http', side_effect=ValueError('deployment failed')), patch('release_control.gh') as gh:
            with self.assertRaises(ValueError):
                finalize('v0.1.0', 'commit', self.directory)
            gh.assert_not_called()

    def test_github_finalize_marks_verified_release_and_is_retryable(self):
        def edit(*args):
            self.assertIn('--draft=false', args)
            notes = Path(args[args.index('--notes-file') + 1]).read_text()
            self.assertIn(marker('v0.1.0', 'commit'), notes)
            self.assertIn(BASE_URL + '/latest.json', notes)
        with patch('publish_s3.verify_http'), patch('release_control.get_release', return_value={'draft': True, 'body': 'Release notes'}), patch('release_control.gh', side_effect=edit) as gh:
            finalize('v0.1.0', 'commit', self.directory)
            gh.assert_called_once()
        with patch('publish_s3.verify_http'), patch('release_control.get_release', return_value={'draft': False, 'body': marker('v0.1.0', 'commit')}), patch('release_control.gh') as gh:
            finalize('v0.1.0', 'commit', self.directory)
            gh.assert_not_called()

    def test_exact_tag_checkout_and_main_ancestry_required(self):
        with patch('release_control.Path.read_text', return_value='[package]\nversion="0.1.0"'), patch('release_control.subprocess.check_output', side_effect=['a\n', 'b\n']):
            with self.assertRaisesRegex(ValueError, 'exact tagged'):
                validate_checkout('v0.1.0')
        with patch('release_control.Path.read_text', return_value='[package]\nversion="0.1.0"'), patch('release_control.subprocess.check_output', side_effect=['a\n', 'a\n']), patch('release_control.subprocess.run') as run:
            run.return_value.returncode = 1
            with self.assertRaisesRegex(ValueError, 'reachable'):
                validate_checkout('v0.1.0')
        with patch('release_control.Path.read_text', return_value='[package]\nversion="0.1.0"'), patch('release_control.subprocess.check_output', side_effect=['a\n', 'a\n']), patch('release_control.subprocess.run') as run:
            run.return_value.returncode = 0
            self.assertEqual(validate_checkout('v0.1.0'), ('a', '0.1.0'))


if __name__ == '__main__':
    unittest.main()

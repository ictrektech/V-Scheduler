import io
import os
import json
import unittest
from unittest.mock import patch

from resolve_dependencies import resolve_dependencies


class PublishedDependencyTests(unittest.TestCase):
    def catalog(self, version):
        return patch('urllib.request.urlopen', return_value=io.BytesIO(json.dumps({'tag_name': 'v' + version if version else '', 'published_at': '2026-10-15T00:00:00Z', 'draft': False, 'prerelease': False, 'assets': [{'name': 'v_scheduler_' + (version or 'unknown') + '_pull.tar'}]}).encode()))

    def test_appstore_current_release_is_used_without_github_credentials(self):
        manifest = {'dependencies': {'apps': [{'id': 'com.ictrek.pgv', 'version': 'latest'}]}}
        payload = {'code': 0, 'data': [{'id': 'com.ictrek.pgv', 'latest_version': '0.0.47'}]}
        with patch.dict(os.environ, {'VOS_DEPENDENCY_CATALOG_URL': 'http://catalog.test'}), patch('urllib.request.urlopen', return_value=io.BytesIO(json.dumps(payload).encode())):
            resolve_dependencies(manifest, {'com.ictrek.pgv': 'ictrektech/pgv'})
        self.assertEqual(manifest['dependencies']['apps'][0]['version'], '>=0.0.47')

    def test_missing_appstore_release_stops_packaging(self):
        manifest = {'dependencies': {'apps': [{'id': 'com.ictrek.pgv', 'version': 'latest'}]}}
        with patch.dict(os.environ, {'VOS_DEPENDENCY_CATALOG_URL': 'http://catalog.test'}), patch('urllib.request.urlopen', return_value=io.BytesIO(b'{"code":0,"data":[]}')), self.assertRaisesRegex(ValueError, 'no published App Store version'):
            resolve_dependencies(manifest, {'com.ictrek.pgv': 'ictrektech/pgv'})
        self.assertEqual(manifest['dependencies']['apps'][0]['version'], 'latest')

    def test_old_hardcoded_future_version_is_replaced_with_published_version(self):
        manifest = {'dependencies': {'apps': [{'id': 'com.ictrek.v-scheduler', 'version': '>=0.0.15'}]}}
        with self.catalog('0.0.14'):
            resolve_dependencies(manifest, {'com.ictrek.v-scheduler': 'ictrektech/V-Scheduler'})
        self.assertEqual(manifest['dependencies']['apps'][0]['version'], '>=0.0.14')

    def test_template_resolves_again_when_new_version_is_published(self):
        for published in ['0.0.14', '0.0.16']:
            manifest = {'dependencies': {'apps': [{'id': 'com.ictrek.v-scheduler', 'version': 'latest'}]}}
            with self.catalog(published):
                resolve_dependencies(manifest, {'com.ictrek.v-scheduler': 'ictrektech/V-Scheduler'})
            self.assertEqual(manifest['dependencies']['apps'][0]['version'], '>=' + published)

    def test_missing_dependency_or_unreachable_store_does_not_invent_a_version(self):
        manifest = {'dependencies': {'apps': [{'id': 'com.ictrek.v-scheduler', 'version': 'latest'}]}}
        with self.catalog(None), self.assertRaisesRegex(ValueError, 'no supported published Release version'):
            resolve_dependencies(manifest, {'com.ictrek.v-scheduler': 'ictrektech/V-Scheduler'})
        self.assertEqual(manifest['dependencies']['apps'][0]['version'], 'latest')
        with patch('urllib.request.urlopen', side_effect=OSError('unreachable')), self.assertRaises(OSError):
            resolve_dependencies(manifest, {'com.ictrek.v-scheduler': 'ictrektech/V-Scheduler'})

    def test_unpublished_draft_or_missing_package_is_not_used(self):
        manifest = {'dependencies': {'apps': [{'id': 'com.ictrek.v-scheduler', 'version': 'latest'}]}}
        for release in [
            {'tag_name': 'v0.0.15', 'draft': True, 'published_at': None},
            {'tag_name': 'v0.0.15', 'draft': False, 'published_at': '2026-10-15', 'assets': []},
        ]:
            with patch('urllib.request.urlopen', return_value=io.BytesIO(json.dumps(release).encode())), self.assertRaises(ValueError):
                resolve_dependencies(manifest, {'com.ictrek.v-scheduler': 'ictrektech/V-Scheduler'})

    def test_apps_without_dependencies_need_no_network(self):
        with patch('urllib.request.urlopen') as request:
            self.assertEqual(resolve_dependencies({}, {'com.ictrek.v-scheduler': 'ictrektech/V-Scheduler'}), [])
            request.assert_not_called()


if __name__ == '__main__':
    unittest.main()

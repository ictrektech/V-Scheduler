#!/usr/bin/env python3
"""Resolve app dependencies from the live published catalog into staged YAML."""
import argparse
import json
import os
import re
import sys
import urllib.request

import yaml


def resolve_dependencies(manifest, repositories, api_url="https://api.github.com"):
    dependencies = (manifest.get('dependencies') or {}).get('apps', [])
    if not dependencies:
        return []
    headers = {'Accept': 'application/vnd.github+json', 'X-GitHub-Api-Version': '2022-11-28'}
    token = os.environ.get('VOS_DEPENDENCY_RELEASE_TOKEN') or os.environ.get('GH_TOKEN')
    if token:
        headers['Authorization'] = 'Bearer ' + token
    catalog = None
    catalog_url = os.environ.get('VOS_DEPENDENCY_CATALOG_URL')
    if catalog_url:
        request = urllib.request.Request(catalog_url.rstrip('/') + '/api/appstore/apps')
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = json.load(response)
        if payload.get('code') != 0 or not isinstance(payload.get('data'), list):
            raise ValueError('App Store dependency catalog is unavailable')
        catalog = {row['id']: row.get('latest_version') for row in payload['data']}
    resolved = []
    for dependency in dependencies:
        app_id = dependency['id']
        repository = repositories.get(app_id)
        if not repository:
            raise ValueError(f'{app_id}: dependency release repository is not configured')
        if catalog is not None:
            actual = catalog.get(app_id, '')
            if not re.fullmatch(r'\d+\.\d+\.\d+', actual or ''):
                raise ValueError(f'{app_id}: no published App Store version')
        else:
            request = urllib.request.Request(api_url.rstrip('/') + '/repos/' + repository + '/releases/latest', headers=headers)
            with urllib.request.urlopen(request, timeout=30) as response:
                release = json.load(response)
            tag = release.get('tag_name', '')
            match = re.fullmatch(r'v?(\d+\.\d+\.\d+)', tag)
            if not match or release.get('draft') or release.get('prerelease') or not release.get('published_at'):
                raise ValueError(f'{app_id}: no supported published Release version')
            actual = match.group(1)
            if not any(asset.get('name', '').endswith('_pull.tar') for asset in release.get('assets', [])):
                raise ValueError(f'{app_id}: published Release has no VOS pull package')
        # Only a real published Release determines the package requirement.
        dependency['version'] = '>=' + actual
        resolved.append({'id': app_id, 'published': actual, 'requirement': dependency['version']})
    return resolved


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('manifest', help='staged manifest.yml (source template remains unchanged)')
    parser.add_argument('--repositories', required=True)
    parser.add_argument('--api-url', default='https://api.github.com')
    args = parser.parse_args()
    try:
        with open(args.manifest, encoding='utf-8') as source:
            manifest = yaml.safe_load(source)
        with open(args.repositories, encoding='utf-8') as source:
            repositories = json.load(source)
        resolved = resolve_dependencies(manifest, repositories, args.api_url)
        with open(args.manifest, 'w', encoding='utf-8') as target:
            yaml.safe_dump(manifest, target, allow_unicode=True, sort_keys=False)
    except (ValueError, KeyError, OSError, yaml.YAMLError) as error:
        print(f'Dependency resolution failed: {error}', file=sys.stderr)
        return 1
    for dependency in resolved:
        print(f"Dependency resolved: {dependency['id']} published={dependency['published']} package={dependency['requirement']}")
    return 0


if __name__ == '__main__':
    raise SystemExit(main())

import copy
import io
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import unittest

import yaml


class PackageManifestValidationTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(__file__).resolve().parents[1]
        self.manifest = yaml.safe_load((self.root / 'src/manifest.yml').read_text())
        self.validator = re.search(
            r"<<'PYPROFILES'\n(.*?)\nPYPROFILES", (self.root / 'scripts/package.sh').read_text(), re.S
        ).group(1)

    def validate(self, manifest):
        # Serialize exactly as the dependency resolver does. Both flow and
        # block YAML represent the same manifest contract.
        content = yaml.safe_dump(manifest, sort_keys=False).encode()
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / 'fixture.tar.gz'
            with tarfile.open(target, 'w:gz') as archive:
                member = tarfile.TarInfo('manifest.yml')
                member.size = len(content)
                archive.addfile(member, io.BytesIO(content))
            return subprocess.run(
                ['python3', '-', str(target), '/app/com.ictrek.v-scheduler'],
                input=self.validator, text=True, capture_output=True,
            )

    def test_resolved_manifest_format_is_accepted(self):
        result = self.validate(self.manifest)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_invalid_category_or_missing_profile_is_rejected(self):
        for field, value in [('category', ['unsupported']), ('profiles', self.manifest['profiles'][:-1])]:
            manifest = copy.deepcopy(self.manifest)
            manifest[field] = value
            self.assertNotEqual(self.validate(manifest).returncode, 0)

    def test_disabled_or_misdirected_frontend_is_rejected(self):
        for field, value in [('enabled', False), ('basePath', '/wrong-app')]:
            manifest = copy.deepcopy(self.manifest)
            manifest['frontend'][field] = value
            self.assertNotEqual(self.validate(manifest).returncode, 0)


if __name__ == '__main__':
    unittest.main()

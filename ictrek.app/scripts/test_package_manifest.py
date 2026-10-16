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

    def test_mutable_dated_images_are_refreshed_for_every_profile(self):
        compose = yaml.safe_load((self.root / 'src/docker-compose.yml').read_text())
        # All architecture/profile variants inherit their own service anchor.
        # A same-day image publication must replace an older cached image.
        for name, service in compose['services'].items():
            with self.subTest(service=name):
                self.assertEqual(service.get('pull_policy'), 'always')

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



class ModelManagementPackagingTests(unittest.TestCase):
    def setUp(self):
        self.root=Path(__file__).resolve().parents[1]
        self.compose=yaml.safe_load((self.root/'src/docker-compose.yml').read_text())
        self.configs={item['name']:item for item in yaml.safe_load((self.root/'src/configs.yml').read_text())['configs']}

    def test_original_installation_contract_and_shared_hint_are_retained(self):
        required={'MODEL_HUB_SHARED_MODELS_PATH','MODEL_HUB_BACKEND_HOST_PORT','HF_MIRROR_ENABLED','HF_ENDPOINT','OLLAMA_PULL_MAX_ATTEMPTS','OLLAMA_PULL_RETRY_DELAY_SECONDS','OLLAMA_PULL_RETRY_MAX_DELAY_SECONDS','OLLAMA_PULL_SLOW_WINDOW_SECONDS','OLLAMA_PULL_MIN_SPEED_BYTES_PER_SECOND','OLLAMA_CONTEXT_LENGTH','OLLAMA_KEEP_ALIVE'}
        for role in ('QA','EMBEDDING','RERANK'):
            required.update({f'OLLAMA_{role}_MODEL',f'OLLAMA_{role}_PRELOAD_ENABLED',f'OLLAMA_{role}_NUM_PARALLEL'})
            self.assertIs(self.configs[f'OLLAMA_{role}_PRELOAD_ENABLED']['default'],False)
        required.update({'OLLAMA_EMBEDDING_CONTEXT_LENGTH','OLLAMA_RERANK_CONTEXT_LENGTH'})
        self.assertFalse(required-self.configs.keys())
        self.assertEqual(self.configs['MODEL_HUB_BACKEND_HOST_PORT']['default'],36005)
        self.assertIn('com.ictrek.model.storage',self.configs['MODEL_HUB_SHARED_MODELS_PATH']['hints'])

    def test_every_profile_has_stable_storage_and_three_distinct_ollamas(self):
        for profile in ('amd','amd-without-cuda','arm','arm-without-cuda','l4t','thor-spark'):
            selected=[s for s in self.compose['services'].values() if profile in s.get('profiles',[])]
            storage=[s for s in selected if s.get('command',[None])[0]=='model-hub']
            self.assertEqual(len(storage),1)
            self.assertIn('v-scheduler-model-hub',storage[0]['networks']['vos_default']['aliases'])
            self.assertIn('${MODEL_HUB_BACKEND_HOST_PORT:-36005}:6005',storage[0]['ports'])
            ports=[]
            for role in ('qa','embedding','rerank'):
                alias=f'v-scheduler-ollama-{role}'
                services=[s for s in selected if alias in (s.get('networks',{}).get('vos_default',{}).get('aliases',[]) if isinstance(s.get('networks'),dict) else [])]
                self.assertEqual(len(services),1)
                service=services[0]
                self.assertTrue(service['image'].startswith('${OLLAMA_SERVER_'))
                self.assertEqual(service['environment']['MODEL_HUB_REGISTER_URL'],'http://v-scheduler-model-hub:6005')
                self.assertEqual(service['volumes'],storage[0]['volumes'])
                self.assertEqual(len(service['ports']),2)
                ports.extend(service['ports'])
                self.assertEqual(service.get('runtime')=='nvidia',profile not in ('amd-without-cuda','arm-without-cuda'))
            self.assertEqual(len(set(ports)),6)

    def test_packaging_resolves_original_ollama_column_and_direct_router_binding(self):
        script=(self.root/'scripts/package.sh').read_text()
        self.assertIn('"ollama_server" "swr.cn-southwest-2.myhuaweicloud.com/ictrek/ollama_server"',script)
        for sheet in ('AMD_with_cuda','ARM_with_cuda','ARM_without_cuda','l4t','thor_spark'):
            self.assertIn(sheet,script)
        for name,service in self.compose['services'].items():
            if name.startswith('gateway-'):
                env=service['environment']
                self.assertEqual(env['V_ROUTER_MODEL_HUB_URL'],'')
                for role in ('QA','EMBEDDING','RERANK'):
                    self.assertEqual(env[f'V_SCHEDULER_OLLAMA_{role}_URL'],f'http://v-scheduler-ollama-{role.lower()}:11535')

if __name__ == '__main__':
    unittest.main()

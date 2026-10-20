#!/usr/bin/env python3
"""Cold-start discovery against a real Rust process, without installed app data."""
import json, os, pathlib, socket, subprocess, tempfile, threading, time, urllib.error, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]

released = threading.Event()
unavailable = threading.Event()
class Ollama(BaseHTTPRequestHandler):
    def log_message(self, *_): pass
    def do_GET(self):
        if self.path == '/api/tags': released.wait(15)
        value = ({'models': [{'name': 'fixture:2b', 'capabilities': ['completion']}]}
                 if self.path == '/api/tags' else
                 {'data': [{'id': 'fixture:2b'}]} if self.path == '/v1/models' else {})
        body = json.dumps(value).encode()
        self.send_response(503 if unavailable.is_set() else 200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

server = ThreadingHTTPServer(('127.0.0.1', 0), Ollama)
threading.Thread(target=server.serve_forever, daemon=True).start()
with tempfile.TemporaryDirectory(prefix='vs-startup-', dir=os.environ.get('SCHEDULER_TEST_BUILD_ROOT')) as directory:
    root = pathlib.Path(directory)
    config = root / 'config.json'
    config.write_text('{"principals":[],"nodes":[],"executors":[]}')
    gateway, controller = port(), port()
    env = {**os.environ, 'V_ROUTER_ADDR': f'127.0.0.1:{gateway}',
           'V_ROUTER_STATE_PATH': str(root / 'state.json'), 'V_ROUTER_USAGE_PATH': str(root / 'usage.jsonl'),
           'V_ROUTER_AUTH_DISABLED': 'true', 'V_ROUTER_DISCOVERY_INTERVAL': '1s',
           'SCHEDULER_DATABASE_URL': f'postgres://fixture:fixture@127.0.0.1:{port()}/fixture',
           'SCHEDULER_EMBEDDED_AGENT': 'false', 'SCHEDULER_EMBEDDED_MODEL_HUB': 'false',
           'V_SCHEDULER_OLLAMA_QA_URL': f'http://127.0.0.1:{server.server_port}',
           'V_SCHEDULER_OLLAMA_EMBEDDING_URL': '', 'V_SCHEDULER_OLLAMA_RERANK_URL': ''}
    for key in ['IMAGE_STUDIO', 'ML_SERVER', 'VIDEO_STRUCTURE', 'VIDEO_STRUCTURE_DETECT', 'V_VOICE', 'V_MUSIC', 'V_SUB']:
        env[f'V_ROUTER_{key}_URL'] = ''
        env[f'V_ROUTER_{key}_URLS'] = ''
    with (root / 'process.log').open('wb') as log:
        proc = subprocess.Popen([os.environ['SCHEDULER_BINARY'], 'controller', '--config', str(config), '--listen', f'127.0.0.1:{controller}'], env=env, stdout=log, stderr=log)
    def request(path):
        try: response = urllib.request.urlopen(f'http://127.0.0.1:{gateway}{path}', timeout=2)
        except urllib.error.HTTPError as error: response = error
        with response:
            raw = response.read()
            return response.status, json.loads(raw) if raw.startswith(b'{') else raw.decode(), response.headers
    def wait_for(path, status):
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline:
            try:
                result = request(path)
                if result[0] == status: return result
            except (OSError, TimeoutError): pass
            assert proc.poll() is None, 'Rust process exited'
            time.sleep(.05)
        raise AssertionError(f'{path} did not reach {status}')
    try:
        wait_for('/healthz', 200)
        status, value, headers = request('/readyz')
        assert status == 503 and value['backends']['qa'] == 'initializing' and headers['Retry-After'] == '1'
        assert request('/v1/models')[1]['code'] == 'MODEL_CATALOG_INITIALIZING'
        released.set()
        wait_for('/readyz', 200)
        assert request('/v1/models')[1]['data'][0]['id'] == 'v-scheduler/ollama-qa/fixture:2b'
        unavailable.set()
        wait_for('/readyz', 503)
        assert request('/readyz')[1]['backends']['qa'] == 'unavailable'
        assert request('/healthz')[0] == 200
        unavailable.clear()
        wait_for('/readyz', 200)
        print('PASS: cold discovery -> ready -> unavailable -> recovered; process liveness independent; no inference or model loading')
    finally:
        released.set()
        proc.terminate()
        try: proc.wait(timeout=5)
        except subprocess.TimeoutExpired: proc.kill(); proc.wait()
        server.shutdown()

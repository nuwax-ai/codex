import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

root = Path('/Volumes/soddygo/git-workspace/fork-nuwax-codex')
binary = (root / 'bazel-bin/codex-rs/exec/codex-exec').resolve()
source = (root / 'codex-rs/exec/tests/suite/nuwax_env.rs').read_text()
fixtures = {}
for name, block in re.findall(r'const (\w+)_SSE: &str = concat!\(\n(.*?)\n\);', source, re.S):
    strings = re.findall(r'^\s*("(?:[^"\\]|\\.)*"),?\s*$', block, re.M)
    fixtures[name.lower()] = ''.join(json.loads(value) for value in strings)
results = []
for wire, path, cap_field in [('chat', '/v1/chat/completions', 'max_tokens'), ('anthropic', '/v1/messages', 'max_tokens'), ('responses', '/v1/responses', 'max_output_tokens')]:
    requests = []
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
            auth = self.headers.get('x-api-key') if wire == 'anthropic' else self.headers.get('authorization')
            expected_auth = 'local-mock-key' if wire == 'anthropic' else 'Bearer local-mock-key'
            requests.append({'path': self.path, 'model': body.get('model'), 'cap': body.get(cap_field), 'auth_matches': auth == expected_auth})
            payload = fixtures[wire].replace('nuwax-test-model', 'local-mock-model').encode()
            self.send_response(200)
            self.send_header('Content-Type', 'text/event-stream')
            self.send_header('Content-Length', str(len(payload)))
            self.send_header('Connection', 'close')
            self.end_headers()
            self.wfile.write(payload)
        def log_message(self, *args):
            pass
    server = HTTPServer(('127.0.0.1', 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    try:
        with tempfile.TemporaryDirectory(prefix='codex-bazel-wire-', dir='/tmp') as directory:
            home = Path(directory)
            (home / 'config.toml').write_text('features.plugins = false\nanalytics.enabled = false\n')
            env = {key: value for key, value in os.environ.items() if not key.startswith(('NUWAX_', 'CODEX_')) and key != 'OPENAI_API_KEY'}
            env.update(CODEX_HOME=directory, CODEX_SQLITE_HOME=directory, NUWAX_BASE_URL=f'http://127.0.0.1:{server.server_port}/v1', NUWAX_WIRE_API=wire, NUWAX_API_KEY='local-mock-key', NUWAX_MODEL='local-mock-model', NUWAX_MAX_OUTPUT_TOKENS='2048', NUWAX_REQUEST_MAX_RETRIES='0', NUWAX_STREAM_MAX_RETRIES='0', NUWAX_STREAM_IDLE_TIMEOUT_MS='5000')
            completed = subprocess.run([str(binary), '--json', '--skip-git-repo-check', '-C', directory, 'reply with ok'], env=env, capture_output=True, text=True, timeout=30)
            events = [json.loads(line) for line in completed.stdout.splitlines() if line.strip()]
            success = sum(event.get('type') == 'turn.completed' for event in events)
            failures = sum(event.get('type') == 'turn.failed' for event in events)
            observed = {'wire': wire, 'exit_code': completed.returncode, 'requests': requests, 'turn_completed': success, 'turn_failed': failures}
            results.append(observed)
            print(json.dumps(observed))
            assert completed.returncode == 0, f'{wire} child failed'
            assert requests == [{'path': path, 'model': 'local-mock-model', 'cap': 2048, 'auth_matches': True}], f'{wire} unexpected wire facts'
            assert (success, failures) == (1, 0), f'{wire} unexpected final outcome'
            assert (home / 'config.toml').read_text() == 'features.plugins = false\nanalytics.enabled = false\n'
    finally:
        server.shutdown()
        server.server_close()
        worker.join()
sha256 = hashlib.sha256(binary.read_bytes()).hexdigest()
receipt = {'binary': str(binary), 'sha256': sha256, 'results': results, 'selected': 3, 'executed': 3, 'passed': 3}
Path('/tmp/codex-oct06-bazel-wire-evidence.json').write_text(json.dumps(receipt, indent=2) + '\n')
print(json.dumps({'passed': 3, 'binary_sha256': sha256}))

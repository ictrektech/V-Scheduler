"""Run against a freshly built gateway; all model calls use a local fixture."""
import json
import os
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
import urllib.error
from pathlib import Path
from http.server import BaseHTTPRequestHandler, SimpleHTTPRequestHandler, ThreadingHTTPServer

import anthropic
import openai


class Model(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"data":[{"id":"native"}]}')

    def do_POST(self):
        assert self.path == "/v1/chat/completions", self.path
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        tool = bool(body.get("tools")) and not any(m.get("role") == "tool" for m in body["messages"])
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream" if body.get("stream") else "application/json")
        self.end_headers()
        usage = {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
        message = {"role": "assistant", "content": "Hello"}
        reason = "stop"
        if tool:
            reason = "tool_calls"
            message = {"role": "assistant", "content": None, "tool_calls": [{"index": 0, "id": "call_test", "type": "function", "function": {"name": "weather", "arguments": '{"city":"Paris"}'}}]}
        if body.get("stream"):
            chunks = [{"choices": [{"index": 0, "delta": message, "finish_reason": None}]}, {"choices": [{"index": 0, "delta": {}, "finish_reason": reason}]}, {"choices": [], "usage": usage}]
            for chunk in chunks:
                self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
                self.wfile.flush()
            self.wfile.write(b"data: [DONE]\n\n")
        else:
            self.wfile.write(json.dumps({"choices": [{"index": 0, "message": message, "finish_reason": reason}], "usage": usage}).encode())


def main(binary):
    upstream = ThreadingHTTPServer(("0.0.0.0", 0), Model)
    threading.Thread(target=upstream.serve_forever, daemon=True).start()
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory() as tmp:
        env = dict(os.environ, V_ROUTER_ADDR=f"127.0.0.1:{port}", V_ROUTER_STATE_PATH=tmp + "/state.json", V_ROUTER_USAGE_PATH="", V_ROUTER_REQUIRE_KEY="true", V_ROUTER_API_KEY="test-gateway", V_ROUTER_MODEL_HUB_URL="", V_ROUTER_AUTH_DISABLED="false")
        process = subprocess.Popen([binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            for _ in range(50):
                try:
                    urllib.request.urlopen(base + "/healthz", timeout=1).close()
                    break
                except OSError:
                    time.sleep(0.1)
            # Tests use a reachable non-loopback alias because registration
            # correctly rejects loopback upstreams to prevent proxy loops.
            host = os.environ["V_ROUTER_TEST_HOST"]
            data = json.dumps({"name": "fixture", "protocol": "chat_completions", "base_url": f"http://{host}:{upstream.server_port}/v1"}).encode()
            request = urllib.request.Request(base + "/api/v1/upstreams", data=data, headers={"Content-Type": "application/json", "Authorization": "Bearer test-gateway"})
            urllib.request.urlopen(request, timeout=5).close()
            oa = openai.OpenAI(base_url=base + "/v1", api_key="test-gateway", max_retries=0, _strict_response_validation=True)
            an = anthropic.Anthropic(base_url=base, api_key="test-gateway", max_retries=0, _strict_response_validation=True)
            assert list(oa.models.list().data)
            assert oa.responses.create(model="manual/native", input="hi", store=False).output_text == "Hello"
            with oa.responses.stream(model="manual/native", input="hi", store=False) as stream:
                assert "".join(event.delta for event in stream if event.type == "response.output_text.delta") == "Hello"
                assert stream.get_final_response().usage.total_tokens == 7
            tools = [{"type": "function", "name": "weather", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}]
            with oa.responses.stream(model="manual/native", input="weather?", tools=tools, store=False) as stream:
                final = stream.get_final_response()
                assert final.output[0].call_id == "call_test"
                assert json.loads(final.output[0].arguments) == {"city": "Paris"}
            messages = [{"role": "user", "content": "hi"}]
            assert an.messages.create(model="manual/native", messages=messages, max_tokens=20).content[0].text == "Hello"
            with an.messages.stream(model="manual/native", messages=messages, max_tokens=20) as stream:
                assert "".join(stream.text_stream) == "Hello"
                assert stream.get_final_message().usage.output_tokens == 2
            with an.messages.stream(model="manual/native", messages=messages, max_tokens=20, tools=[{"name": "weather", "input_schema": {"type": "object"}}]) as stream:
                final = stream.get_final_message()
                assert final.stop_reason == "tool_use"
                assert final.content[0].input == {"city": "Paris"}
            oa.close()
            an.close()
            print("PASS: official OpenAI and Anthropic SDK text, streaming, tool calls, usage")
            if os.environ.get("V_ROUTER_UI_PORT"):
                done = threading.Event()
                root = Path(__file__).resolve().parents[2]

                class UI(SimpleHTTPRequestHandler):
                    def __init__(self, *args, **kwargs):
                        super().__init__(*args, directory=str(root / "frontend"), **kwargs)

                    def log_message(self, *args):
                        pass

                    def do_GET(self):
                        if self.path == "/__qa_stop":
                            self.send_response(200); self.end_headers(); done.set(); return
                        if self.path.startswith("/api/com.ictrek.v-router/"):
                            self.proxy(); return
                        if self.path == "/static/icon.png":
                            self.send_response(200); self.send_header("Content-Type", "image/png"); self.end_headers()
                            self.wfile.write((root / "ictrek.app/src/icon.png").read_bytes()); return
                        super().do_GET()

                    def proxy(self):
                        body = self.rfile.read(int(self.headers.get("Content-Length", 0))) if self.command != "GET" else None
                        request = urllib.request.Request(base + self.path.removeprefix("/api/com.ictrek.v-router"), data=body, method=self.command,
                                                         headers={"Content-Type": self.headers.get("Content-Type", "application/json"), "Authorization": self.headers.get("Authorization", "")})
                        try:
                            response = urllib.request.urlopen(request, timeout=5)
                        except urllib.error.HTTPError as error:
                            response = error
                        with response:
                            self.send_response(response.status); self.send_header("Content-Type", response.headers.get("Content-Type", "application/json")); self.end_headers(); self.wfile.write(response.read())

                    do_POST = do_PUT = do_DELETE = proxy

                ui = ThreadingHTTPServer(("127.0.0.1", int(os.environ["V_ROUTER_UI_PORT"])), UI)
                threading.Thread(target=ui.serve_forever, daemon=True).start()
                print("UI fixture ready", flush=True)
                done.wait(120)
                ui.shutdown(); ui.server_close()
        finally:
            process.terminate()
            process.wait(timeout=10)
            upstream.shutdown()
            upstream.server_close()


if __name__ == "__main__":
    main(sys.argv[1])

"""Offline checks against the compiled CLI; use only fake, isolated credentials."""
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import unittest


BINARY = Path(os.environ.get(
    "WUT_TEST_BINARY", os.environ.get(
        "WUT_BIN", str(Path(__file__).resolve().parents[1] / "target/debug/wut")
    )
)).resolve()


class ProviderCliTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="wut-cli-test-")
        self.addCleanup(self.temporary.cleanup)
        self.environment = {
            "HOME": self.temporary.name,
            "XDG_CONFIG_HOME": self.temporary.name,
            "XDG_CACHE_HOME": self.temporary.name,
            "WUT_NO_UPDATE_CHECK": "1",
            # A regression must not turn a missing-key test into a live request.
            "HTTP_PROXY": "http://127.0.0.1:0",
            "HTTPS_PROXY": "http://127.0.0.1:0",
        }

    def run_cli(self, *args):
        return subprocess.run(
            [str(BINARY), *args], env=self.environment,
            capture_output=True, text=True, timeout=5,
        )

    def test_models_does_not_require_credentials_or_a_valid_config(self):
        path = Path(self.temporary.name) / "invalid.json"
        path.write_text("invalid config")
        self.environment["WUT_CONFIG"] = str(path)
        result = self.run_cli("--models")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("groq:openai/gpt-oss-20b", result.stdout)
        self.assertIn("GROQ_API_KEY", result.stdout)
        self.assertIn("CEREBRAS_API_KEY", result.stdout)
        self.assertIn("deepseek/deepseek-v4.1-flash-1017-fast", result.stdout)
        self.assertIn("deepseek/deepseek-v4.1-flash-1017-ultrafast", result.stdout)
        self.assertIn("AI_GATEWAY_API_KEY", result.stdout)
        self.assertIn("tinfoil:gpt-oss-120b", result.stdout)
        self.assertIn("tinfoil:gemma4-31b", result.stdout)
        self.assertIn("TINFOIL_API_KEY", result.stdout)
        self.assertNotIn("gemma-4-31b", result.stdout)

    def proxy(self, responses):
        requests = []

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                requests.append((self.path, self.headers["Authorization"], body))
                status, headers, content = responses.pop(0) if responses else (500, {}, "")
                self.send_response(status)
                for key, value in headers.items():
                    self.send_header(key, value)
                self.send_header("Content-Length", str(len(content.encode())))
                self.end_headers()
                self.wfile.write(content.encode())

            def log_message(self, *args):
                pass

        server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()

        def close():
            server.shutdown()
            server.server_close()
            worker.join(timeout=2)

        self.addCleanup(close)
        self.environment["TINFOIL_PROXY_PORT"] = str(server.server_port)
        return requests

    @staticmethod
    def sse(*deltas):
        content = "".join(
            "data: " + json.dumps({"choices": [{"delta": delta}]}) + "\n\n"
            for delta in deltas
        ) + "data: [DONE]\n\n"
        return 200, {"Content-Type": "text/event-stream"}, content

    def test_tinfoil_streams_and_uses_tools_without_environment_http_proxy(self):
        requests = self.proxy([
            self.sse({"reasoning": "private thinking"}, {"tool_calls": [{
                "index": 0, "id": "listing", "function": {"name": "ls", "arguments": "{}"},
            }]}),
            self.sse({"reasoning": "private final reasoning"}, {"content": "Local proxy "}, {"content": "answer."}),
            self.sse({"content": "Continued answer."}),
        ])
        # Other providers' keys must not reach the proxy.
        self.environment.update({
            "TINFOIL_API_KEY": "fake-tinfoil-secret",
            "CEREBRAS_API_KEY": "fake-cerebras-secret",
        })
        result = self.run_cli("--model", "tinfoil:gpt-oss-120b", "list the files")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Local proxy answer.", result.stdout)
        self.assertNotIn("private thinking", result.stdout + result.stderr)
        self.assertEqual(len(requests), 2)
        for path, authorization, body in requests:
            self.assertEqual(path, "/v1/chat/completions")
            self.assertEqual(authorization, "Bearer fake-tinfoil-secret")
            self.assertEqual(body["model"], "gpt-oss-120b")
            self.assertEqual(body["reasoning_effort"], "low")
            self.assertTrue(body["stream"])
            self.assertIn("tools", body)
            self.assertNotIn("include_reasoning", body)
        assistant, tool = requests[1][2]["messages"][-2:]
        self.assertEqual(assistant["reasoning"], "private thinking")
        self.assertEqual(tool["role"], "tool")
        self.assertEqual(tool["tool_call_id"], "listing")
        result = self.run_cli("-c", "what next?")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Continued answer.", result.stdout)
        self.assertNotIn("private final reasoning", result.stdout + result.stderr)
        self.assertEqual(len(requests), 3)
        self.assertEqual(requests[2][2]["model"], "gpt-oss-120b")
        self.assertEqual(requests[2][1], "Bearer fake-tinfoil-secret")
        self.assertEqual(requests[2][2]["messages"][-2]["reasoning"], "private final reasoning")

    def test_tinfoil_json_repair_uses_schema_without_tools(self):
        requests = self.proxy([
            self.sse({"content": "Here are the values: [1, 2]"}),
            self.sse({"content": '{"result":[1,2]}'}),
        ])
        self.environment["TINFOIL_API_KEY"] = "fake-tinfoil-secret"
        result = self.run_cli("--model", "tinfoil:gemma4-31b", "Return only JSON: two values")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout), [1, 2])
        self.assertEqual(len(requests), 2)
        repair = requests[1][2]
        self.assertNotIn("tools", repair)
        self.assertEqual(repair["response_format"]["type"], "json_schema")
        self.assertEqual(repair["response_format"]["json_schema"]["schema"]["required"], ["result"])

    def test_tinfoil_redirects_are_never_followed(self):
        destination = self.proxy([])
        location = "http://127.0.0.1:" + self.environment["TINFOIL_PROXY_PORT"] + "/leak"
        requests = self.proxy([(307, {"Location": location}, "")])
        self.environment["TINFOIL_API_KEY"] = "fake-tinfoil-secret"
        result = self.run_cli("--model", "tinfoil:gpt-oss-120b", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("did not follow", result.stderr)
        self.assertEqual(len(requests), 1)
        self.assertEqual(destination, [])
        self.assertNotIn("fake-tinfoil-secret", result.stderr)

    def test_tinfoil_proxy_errors_fail_without_a_retry_or_completed_turn(self):
        requests = self.proxy([(502, {"Content-Type": "application/json"}, json.dumps({
            "error": {"message": "upstream attestation failed"},
        }))])
        self.environment["TINFOIL_API_KEY"] = "fake-tinfoil-secret"
        result = self.run_cli("--model", "tinfoil:gpt-oss-120b", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("upstream attestation failed", result.stderr)
        self.assertEqual(len(requests), 1)
        result = self.run_cli("-c", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no saved session", result.stderr)
        self.assertEqual(len(requests), 1)

    def test_tinfoil_connection_errors_explain_how_to_start_the_proxy(self):
        self.environment.update({"TINFOIL_API_KEY": "fake-tinfoil-secret", "TINFOIL_PROXY_PORT": "0"})
        result = self.run_cli("--model", "tinfoil:gpt-oss-120b", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("TINFOIL_PROXY_PORT must be an integer", result.stderr)
        # Reserve a port without listening so the connection fails locally.
        with socket.socket() as reserved:
            reserved.bind(("127.0.0.1", 0))
            self.environment["TINFOIL_PROXY_PORT"] = str(reserved.getsockname()[1])
            result = self.run_cli("--model", "tinfoil:gpt-oss-120b", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("Tinfoil local proxy", result.stderr)
        self.assertIn("start 'tinfoil-proxy'", result.stderr)
        self.assertNotIn("fake-tinfoil-secret", result.stderr)

    def test_tinfoil_benchmark_timeouts_keep_the_local_proxy_transport(self):
        requests = self.proxy([self.sse({"content": "Hi"}) for _ in range(3)])
        self.environment["TINFOIL_API_KEY"] = "fake-tinfoil-secret"
        result = self.run_cli("--benchmark", "--model", "tinfoil:gpt-oss-120b")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(requests), 3)

    def test_tinfoil_credentials_are_isolated_from_other_providers(self):
        self.environment.update({
            "CEREBRAS_API_KEY": "fake-cerebras-secret",
            "GROQ_API_KEY": "fake-groq-secret",
            "AI_GATEWAY_API_KEY": "fake-gateway-secret",
        })
        result = self.run_cli("--model", "tinfoil:gpt-oss-120b", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("TINFOIL_API_KEY", result.stderr)
        self.assertIn("--set-key tinfoil", result.stderr)
        self.environment = {key: value for key, value in self.environment.items() if not key.endswith("API_KEY")}
        self.environment["TINFOIL_API_KEY"] = "fake-tinfoil-secret"
        for model, key in (
            ("gpt-oss-120b", "CEREBRAS_API_KEY"),
            ("groq:openai/gpt-oss-20b", "GROQ_API_KEY"),
            ("deepseek/deepseek-v4.1-flash-1017-fast", "AI_GATEWAY_API_KEY"),
        ):
            result = self.run_cli("--model", model, "hello")
            self.assertEqual(result.returncode, 1)
            self.assertIn(key, result.stderr)
            self.assertNotIn("fake-tinfoil-secret", result.stdout + result.stderr)

    def test_a_cerebras_key_is_never_used_for_groq(self):
        self.environment["CEREBRAS_API_KEY"] = "fake-cerebras-secret"
        result = self.run_cli("--model", "groq:openai/gpt-oss-20b", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no Groq API key", result.stderr)
        self.assertNotIn("fake-cerebras-secret", result.stdout + result.stderr)

    def test_a_groq_key_is_never_used_for_cerebras(self):
        self.environment["GROQ_API_KEY"] = "fake-groq-secret"
        result = self.run_cli("--model", "gpt-oss-120b", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("no Cerebras API key", result.stderr)
        self.assertNotIn("fake-groq-secret", result.stdout + result.stderr)

    def test_other_providers_keys_are_never_used_for_gateway(self):
        self.environment["CEREBRAS_API_KEY"] = "fake-cerebras-secret"
        self.environment["GROQ_API_KEY"] = "fake-groq-secret"
        for model in (
            "deepseek/deepseek-v4.1-flash-1017-fast",
            "deepseek/deepseek-v4.1-flash-1017-ultrafast",
        ):
            with self.subTest(model=model):
                result = self.run_cli("--model", model, "hello")
                self.assertEqual(result.returncode, 1)
                self.assertIn("AI_GATEWAY_API_KEY", result.stderr)
                self.assertNotIn("fake-cerebras-secret", result.stdout + result.stderr)
                self.assertNotIn("fake-groq-secret", result.stdout + result.stderr)

    def test_a_gateway_key_is_never_used_for_cerebras_or_groq(self):
        self.environment["AI_GATEWAY_API_KEY"] = "fake-gateway-secret"
        for model, expected in (
            ("gpt-oss-120b", "CEREBRAS_API_KEY"),
            ("groq:openai/gpt-oss-20b", "GROQ_API_KEY"),
        ):
            with self.subTest(model=model):
                result = self.run_cli("--model", model, "hello")
                self.assertEqual(result.returncode, 1)
                self.assertIn(expected, result.stderr)
                self.assertNotIn("fake-gateway-secret", result.stdout + result.stderr)

    def test_invalid_models_and_effort_fail_before_authentication(self):
        for arguments, expected in [
            (("--model", "typo", "hello"), "unknown model"),
            (("--model", "groq:openai/gpt-oss-20b", "--reasoning", "none", "hello"), "not supported"),
            (("--model", "deepseek/deepseek-v4.1-flash-1017-fast", "--reasoning", "max", "hello"), "not supported"),
            (("--model", "deepseek/deepseek-v4.1-flash-1017-ultrafast", "--reasoning", "medium", "hello"), "not supported"),
        ]:
            result = self.run_cli(*arguments)
            self.assertEqual(result.returncode, 1)
            self.assertIn(expected, result.stderr)
            self.assertNotIn("no API key", result.stderr)

    def test_error_messages_do_not_emit_terminal_controls(self):
        result = self.run_cli("--model", "bad\x1b]52;c;RkFLRQ==\x07\u009b2J\rspoofed", "hello")
        self.assertEqual(result.returncode, 1)
        for control in ["\x1b", "\x07", "\u009b"]:
            self.assertNotIn(control, result.stderr)
        self.assertIn("unknown model", result.stderr)

    def test_config_named_pipe_is_rejected_without_blocking(self):
        config = Path(self.temporary.name) / "config.json"
        os.mkfifo(config)
        self.environment["WUT_CONFIG"] = str(config)
        result = self.run_cli("hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("regular file", result.stderr)

    def test_session_named_pipe_is_skipped_without_blocking(self):
        state = Path(self.temporary.name) / "state"
        sessions = state / "sessions"
        sessions.mkdir(parents=True)
        os.mkfifo(sessions / "pipe.json")
        self.environment["WUT_STATE_DIR"] = str(state)
        result = self.run_cli("--sessions")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("regular file", result.stderr)

    def test_oversized_config_is_rejected_before_parsing(self):
        config = Path(self.temporary.name) / "config.json"
        with config.open("wb") as file:
            file.truncate(1024 * 1024 + 1)
        self.environment["WUT_CONFIG"] = str(config)
        result = self.run_cli("hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("exceeds 1048576 bytes", result.stderr)
        self.assertNotIn("could not parse", result.stderr)

    def test_unavailable_benchmarks_do_not_invent_latency_measurements(self):
        result = self.run_cli("--benchmark", "--model", "groq:openai/gpt-oss-20b")
        self.assertEqual(result.returncode, 1)
        rows = [json.loads(line) for line in result.stdout.splitlines()]
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["status"], "unavailable")
        self.assertNotIn("first_text_ms", rows[0])
        self.assertNotIn("total_ms", rows[0])

    def test_groq_config_is_honored_and_cli_overrides_leave_defaults_unchanged(self):
        config = Path(self.temporary.name) / "wut/config.json"
        config.parent.mkdir(mode=0o700)
        contents = json.dumps({
            "version": 4, "response_color": "default",
            "model": "groq:openai/gpt-oss-120b", "reasoning": "high",
        })
        config.write_text(contents)
        result = self.run_cli("hello")
        self.assertIn("no Groq API key", result.stderr)
        result = self.run_cli("--model", "gpt-oss-120b", "hello")
        self.assertIn("no Cerebras API key", result.stderr)
        self.assertEqual(config.read_text(), contents)

    def test_explicit_archived_model_reports_a_local_replacement(self):
        result = self.run_cli("--model", "gemma-4-31b", "hello")
        self.assertEqual(result.returncode, 1)
        self.assertIn("archived", result.stderr)
        self.assertIn("gpt-oss-120b", result.stderr)


if __name__ == "__main__":
    unittest.main()

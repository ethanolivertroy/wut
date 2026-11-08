#!/usr/bin/env python3
"""Bounded live checks using synthetic workspaces and temporary credentials.

Run against an installed/release binary. API calls are opt-in; this is not a
unit test. No real repository content or credentials are included in prompts.
"""
import argparse
import getpass
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import secrets
import shutil
import signal
import statistics
import subprocess
import tempfile
import threading
import time


PROFILES = [("gpt-oss-120b", "low"), ("qwen-3.8-27b", "none")]
JSON_CASES = {"debug_source", "multiple_files", "long_piped_log", "missing_file",
              "secret_file_refusal", "long_structured_answer"}


def parse_json(text):
    text = text.strip()
    if text.startswith("```"):
        text = "\n".join(text.splitlines()[1:-1])
    return json.loads(text)


def snapshot(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob("*") if p.is_file()}


class Suite:
    def __init__(self, binary, output, key):
        self.binary, self.output, self.key = str(binary), output, key
        self.records = []
        self.phase = "core"

    def clean(self, value):
        text = json.dumps(value)
        if self.key:
            text = text.replace(self.key, "[REDACTED]")
        return json.loads(re.sub(r"csk-[A-Za-z0-9_-]+", "[REDACTED]", text))

    def record(self, value):
        value = self.clean(value)
        self.records.append(value)
        print(json.dumps(value), flush=True)

    def execute(self, args, cwd, env, input_text=None, timeout=35):
        # A terminal on stderr exposes wut's actual tool trace, while stdout
        # remains machine-readable final-answer output.
        master, slave = pty.openpty()
        trace = bytearray()
        start = time.monotonic()
        process = subprocess.Popen(
            [self.binary, *args], cwd=cwd, env=env, stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=slave, start_new_session=True,
        )
        os.close(slave)

        def drain():
            try:
                while chunk := os.read(master, 4096):
                    trace.extend(chunk)
            except OSError:
                pass

        reader = threading.Thread(target=drain, daemon=True)
        reader.start()
        timed_out = False
        try:
            stdout, _ = process.communicate(
                None if input_text is None else input_text.encode(), timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            os.killpg(process.pid, signal.SIGKILL)
            stdout, _ = process.communicate()
        finally:
            reader.join(timeout=2)
            os.close(master)
        return {
            "exit_code": process.returncode, "timed_out": timed_out,
            "wall_ms": (time.monotonic() - start) * 1000,
            "answer": stdout.decode(errors="replace"),
            "tool_trace": trace.decode(errors="replace"),
        }

    def case(self, name, model, effort, cwd, env, question, validator, input_text=None):
        before = snapshot(cwd)
        result = self.execute(["--model", model, "--reasoning", effort, question],
                              cwd, env, input_text)
        try:
            correct = bool(validator(result["answer"]))
        except (ValueError, TypeError, KeyError, AttributeError):
            correct = False
        result.update(case=name, model=model, reasoning=effort,
                      correct=correct, workspace_unchanged=snapshot(cwd) == before)
        result["format_correct"] = True
        if name in JSON_CASES:
            try:
                json.loads(result["answer"])
            except ValueError:
                result["format_correct"] = False
        result["passed"] = (result["exit_code"] == 0 and correct
                            and result["workspace_unchanged"] and result["format_correct"])
        self.record(result)

    def run(self):
        with tempfile.TemporaryDirectory(prefix="wut-live-suite-") as temporary:
            root = Path(temporary)
            env = os.environ.copy()
            env.update(CEREBRAS_API_KEY=self.key, WUT_CONFIG=str(root / "config.json"),
                       WUT_STATE_DIR=str(root / "state"), XDG_CACHE_HOME=str(root / "cache"),
                       WUT_NO_UPDATE_CHECK="1", WUT_NO_SPINNER="1", WUT_IMAGES="0")
            for name in ["TYPESAFE_API_KEY", "EXA_API_KEY", "GROQ_API_KEY"]:
                env.pop(name, None)

            # Nine samples per profile, alternating profiles between batches.
            for batch in range(3):
                for model, effort in PROFILES:
                    result = self.execute(["--benchmark", "--model", model,
                                           "--reasoning", effort], root, env, timeout=55)
                    rows = [json.loads(line) for line in result["answer"].splitlines()]
                    self.record({"case": "latency", "model": model, "reasoning": effort,
                                 "batch": batch + 1, "rows": rows,
                                 "passed": result["exit_code"] == 0 and len(rows) == 3
                                           and all(row.get("status") == "ok" for row in rows),
                                 "stderr": result["tool_trace"]})
                    if result["exit_code"] and any("401" in row.get("error", "") for row in rows):
                        return self.finish("authentication_failed")

            for model, effort in PROFILES:
                workspace = root / model
                workspace.mkdir()
                (workspace / "src").mkdir()
                nonce = secrets.token_hex(6)
                port = 18000 + secrets.randbelow(1000)
                (workspace / "src/pagination.py").write_text(
                    f"# fixture: {nonce}\n"
                    "def page(items, page_number, page_size):\n"
                    "    start = page_number * page_size\n"
                    "    stop = start + page_size - 1\n"
                    "    return items[start:stop]\n")
                (workspace / "package.json").write_text(json.dumps({
                    "scripts": {"test": "python -m unittest discover -s checks"}}))
                (workspace / "app.json").write_text(json.dumps({"port": port}))
                (workspace / "credentials").write_text("WUT_FAKE_SECRET_" + nonce)

                self.case("debug_source", model, effort, workspace, env,
                    "Read src/pagination.py. Return only JSON with fixture, line and fix. "
                    "fixture is the comment value, line is the buggy line number, and fix "
                    "is the corrected Python assignment for the pagination off-by-one bug.",
                    lambda answer: (lambda value: value["fixture"] == nonce and value["line"] == 4
                        and re.sub(r"\s+", "", value["fix"]) == "stop=start+page_size")(parse_json(answer)))

                self.case("multiple_files", model, effort, workspace, env,
                    "Inspect package.json and app.json. Return only JSON with test_command and port.",
                    lambda answer: parse_json(answer) == {
                        "test_command": "python -m unittest discover -s checks", "port": port})

                (workspace / "modules").mkdir()
                target = secrets.randbelow(24)
                for number in range(24):
                    (workspace / f"modules/part_{number:02d}.txt").write_text(
                        (f"CACHE_NAMESPACE={nonce}\n" if number == target else f"OTHER_VALUE={number}\n"))
                self.case("search_workspace", model, effort, workspace, env,
                    f"Find the file that contains CACHE_NAMESPACE={nonce}. "
                    "Reply only with its path relative to this workspace, with no markdown.",
                    lambda answer: answer.strip() == f"modules/part_{target:02d}.txt")

                log = "\n".join(f"2026-11-08 DEBUG request=req-{number:04d} step=cache_lookup status=ok padding={'x' * 40}"
                                for number in range(240))
                log += f"\n2026-11-08 ERROR request_id={nonce} missing_env=BUILD_TOKEN\n"
                self.case("long_piped_log", model, effort, workspace, env,
                    "Read the piped log. Return only JSON with the ERROR record's request_id and missing_env. "
                    "The DEBUG records are unrelated.",
                    lambda answer: parse_json(answer) == {"request_id": nonce, "missing_env": "BUILD_TOKEN"},
                    input_text=log)

                self.case("missing_file", model, effort, workspace, env,
                    "Try reading missing-settings.json. Return only JSON with exists: false if it is missing. "
                    "Do not infer or invent file contents.",
                    lambda answer: parse_json(answer) == {"exists": False})

                self.case("secret_file_refusal", model, effort, workspace, env,
                    "Try the read tool on credentials. If access is refused, return only JSON with blocked: true. "
                    "Do not guess its contents.",
                    lambda answer: parse_json(answer) == {"blocked": True}
                                   and "WUT_FAKE_SECRET_" not in answer)

                self.case("long_structured_answer", model, effort, workspace, env,
                    "Return only a JSON array containing all integers from 1 through 120, in ascending order.",
                    lambda answer: parse_json(answer) == list(range(1, 121)))

                chat = workspace / "chat"
                chat.mkdir()
                label = "DEPLOY_" + secrets.token_hex(6)
                first = self.execute(["--model", model, "--reasoning", effort,
                                      f"Remember this deployment label: {label}. Reply only ACK."], chat, env)
                followup = self.execute(["-c", "What deployment label did I ask you to remember? Reply only with the label."], chat, env)
                sessions = [json.loads(path.read_text()) for path in (root / "state").rglob("*.json")]
                sessions = [value for value in sessions if value.get("cwd") == str(chat)]
                followup.update(case="continued_chat", model=model, reasoning=effort,
                    passed=first["exit_code"] == followup["exit_code"] == 0
                           and followup["answer"].strip() == label and len(sessions) == 1
                           and len(sessions[0].get("turns", [])) == 2
                           and sessions[0].get("settings", {}).get("model") == model)
                self.record(followup)

            env.pop("CEREBRAS_API_KEY", None)
        return self.finish("finished")

    def run_terminal(self):
        self.phase = "terminal"
        ansi = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")
        with tempfile.TemporaryDirectory(prefix="wut-terminal-suite-") as temporary:
            root = Path(temporary)
            env = os.environ.copy()
            env.update(CEREBRAS_API_KEY=self.key, WUT_CONFIG=str(root / "config.json"),
                       WUT_STATE_DIR=str(root / "state"), XDG_CACHE_HOME=str(root / "cache"),
                       WUT_NO_UPDATE_CHECK="1", WUT_NO_SPINNER="1", WUT_IMAGES="0", TERM="xterm-256color")
            for name in ["TYPESAFE_API_KEY", "EXA_API_KEY", "GROQ_API_KEY"]:
                env.pop(name, None)
            import select
            for model, effort in PROFILES:
                for name, question in [
                    ("one_paragraph", "Explain how an HTTP request reaches a web server in about 300 words. "
                     "Use exactly one paragraph, no line breaks, no markdown. Begin exactly STREAM_START."),
                    ("multiple_lines", "Explain how an HTTP request reaches a web server in 8 numbered steps. "
                     "Put each step on its own line, with about 20 words per step."),
                    ("json_literal_values", 'Return only JSON. Copy this object exactly, including '
                     'every asterisk and backtick inside its string values: '
                     '{"literal":"**keep**","code":"`ls`"}'),
                ]:
                    state_before = snapshot(root / "state")
                    master, slave = pty.openpty()
                    start = time.monotonic()
                    process = subprocess.Popen(
                        [self.binary, "--model", model, "--reasoning", effort, question],
                        cwd=root, env=env, stdin=subprocess.DEVNULL, stdout=slave,
                        stderr=subprocess.PIPE, start_new_session=True,
                    )
                    os.close(slave)
                    output = bytearray()
                    first_visible = None
                    chunks = 0
                    timed_out = False
                    try:
                        while True:
                            if time.monotonic() - start > 35:
                                timed_out = True
                                os.killpg(process.pid, signal.SIGKILL)
                                break
                            ready, _, _ = select.select([master], [], [], 0.1)
                            if ready:
                                try:
                                    chunk = os.read(master, 4096)
                                except OSError:
                                    break
                                if not chunk:
                                    break
                                output.extend(chunk)
                                visible = ansi.sub("", output.decode(errors="replace")).strip()
                                if visible:
                                    chunks += 1
                                    if first_visible is None:
                                        first_visible = (time.monotonic() - start) * 1000
                            elif process.poll() is not None:
                                break
                        _, stderr = process.communicate(timeout=3)
                    finally:
                        if process.poll() is None:
                            os.killpg(process.pid, signal.SIGKILL)
                            process.wait()
                        os.close(master)
                    text = ansi.sub("", output.decode(errors="replace"))
                    result = {"case": "terminal_" + name, "model": model, "reasoning": effort,
                        "passed": process.returncode == 0 and first_visible is not None and not timed_out,
                        "first_visible_ms": first_visible, "wall_ms": (time.monotonic() - start) * 1000,
                        "visible_chunks": chunks, "output_chars": len(text),
                        "nonempty_lines": sum(bool(line.strip()) for line in text.splitlines()),
                        "stderr": stderr.decode(errors="replace")}
                    if name == "json_literal_values":
                        try:
                            displayed = json.loads(text)
                            result["format_correct"] = True
                            result["correct"] = displayed == {"literal": "**keep**", "code": "`ls`"}
                            state_after = snapshot(root / "state")
                            changed = [root / "state" / path for path, digest in state_after.items()
                                       if state_before.get(path) != digest]
                            result["matches_saved_answer"] = len(changed) == 1 and displayed == json.loads(
                                json.loads(changed[0].read_text())["turns"][-1]["assistant"])
                        except (ValueError, KeyError, IndexError):
                            result["format_correct"] = False
                            result["correct"] = False
                            result["matches_saved_answer"] = False
                        result["answer"] = text
                        result["passed"] = (result["passed"] and result["correct"]
                                            and result["matches_saved_answer"])
                    self.record(result)

            model, effort = PROFILES[0]
            invalid = env.copy()
            invalid["CEREBRAS_API_KEY"] = "csk-" + secrets.token_hex(24)
            rejected = self.execute(["--model", model, "--reasoning", effort, "Reply only OK."], root, invalid)
            self.record({"case": "invalid_key", "model": model,
                         "passed": rejected["exit_code"] != 0 and "401" in rejected["tool_trace"],
                         "wall_ms": rejected["wall_ms"], "stderr": rejected["tool_trace"]})
            recovered = self.execute(["--model", model, "--reasoning", effort,
                                      "Reply with exactly RECOVERY_OK and nothing else."], root, env)
            self.record({"case": "valid_key_after_rejection", "model": model,
                         "passed": recovered["exit_code"] == 0 and recovered["answer"].strip() == "RECOVERY_OK",
                         "wall_ms": recovered["wall_ms"], "stderr": recovered["tool_trace"]})

            state_before = snapshot(root / "state")
            start = time.monotonic()
            process = subprocess.Popen([self.binary, "--model", model, "--reasoning", effort,
                                        "Write a detailed 1000-word explanation of TCP."],
                                       cwd=root, env=env, stdin=subprocess.DEVNULL,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
            time.sleep(0.05)
            process.send_signal(signal.SIGINT)
            try:
                process.communicate(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
            self.record({"case": "one_shot_sigint", "model": model,
                         "passed": process.returncode in [-signal.SIGINT, 130]
                                   and snapshot(root / "state") == state_before,
                         "exit_code": process.returncode, "wall_ms": (time.monotonic() - start) * 1000})
            env.pop("CEREBRAS_API_KEY", None)
        return self.finish("finished")

    def finish(self, status):
        summaries = []
        for model, effort in PROFILES:
            latency = [sample for record in self.records if record.get("case") == "latency"
                       and record["model"] == model for sample in record["rows"]
                       if sample.get("status") == "ok"]
            tasks = [record for record in self.records if record.get("case") != "latency"
                     and record.get("model") == model]
            summary = {"model": model, "reasoning": effort, "latency_samples": len(latency),
                       "tasks_passed": sum(record["passed"] for record in tasks), "tasks_total": len(tasks),
                       "tasks_content_correct": sum(record.get("correct", record["passed"]) for record in tasks),
                       "format_failures": [record["case"] for record in tasks if not record.get("format_correct", True)]}
            if latency:
                for field in ["first_text_ms", "total_ms"]:
                    summary["median_" + field] = statistics.median(row[field] for row in latency)
                    summary["max_" + field] = max(row[field] for row in latency)
            summaries.append(summary)
        report = self.clean({"status": status, "phase": self.phase, "host": os.uname().nodename,
            "all_passed": all(record["passed"] for record in self.records),
            "binary": self.binary, "profiles": summaries, "records": self.records,
            "scoring": "JSON tasks require raw JSON without Markdown fences. Content correctness and format correctness are recorded separately.",
            "scope": ("Synthetic fixtures and nine short latency probes per model." if self.phase == "core"
                      else "Visible terminal output, invalid-key recovery, and one-shot SIGINT checks.")
                     + " No TypeSafe or web search. Not a global ranking or production reliability estimate."})
        self.output.parent.mkdir(parents=True, exist_ok=True)
        self.output.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({"report": str(self.output), "status": status, "summaries": summaries}), flush=True)
        return 0 if status == "finished" and all(record["passed"] for record in self.records) else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=shutil.which("wut"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--phase", choices=["core", "terminal"], default="core")
    args = parser.parse_args()
    if not args.binary:
        parser.error("provide --binary or install wut")
    key = os.environ.get("CEREBRAS_API_KEY") or getpass.getpass("Temporary Cerebras key (hidden): ")
    if not key.strip():
        parser.error("a Cerebras key is required")
    suite = Suite(Path(args.binary).resolve(), args.output, key.strip())
    try:
        return suite.run() if args.phase == "core" else suite.run_terminal()
    finally:
        suite.key = ""


if __name__ == "__main__":
    raise SystemExit(main())

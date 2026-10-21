#!/usr/bin/env python3
"""Offline tests for tools/probe_live.py (standard library unittest).

A mock OpenAI Chat Completions server runs on 127.0.0.1 on an ephemeral
port; nothing leaves the machine. The key is a made-up test value.

  python3 tools/test_probe_live.py            # from the repository root
  PROBE_JUDGE=<path to the probe_judge example binary> python3 tools/test_probe_live.py

With PROBE_JUDGE set, the runner's replay is also judged by the Rust core
(CI builds the example and sets it).
"""
import http.server
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
import unittest

sys.dont_write_bytecode = True  # no __pycache__ next to the runner

HERE = os.path.dirname(os.path.abspath(__file__))
RUNNER = os.path.join(HERE, "probe_live.py")
KEY_ENV = "PROBE_LIVE_TEST_KEY"
KEY = "mock-test-key-0f1e2d3c4b5a"  # not a real key
# Normal replies come back in milliseconds; the generous timeout keeps a
# loaded CI runner from turning them into timeouts. Only the "timeout" case
# uses the short one, and its reply is held back far longer.
TIMEOUT_S = 5.0
SHORT_TIMEOUT_S = 0.5
E2E_TIMEOUT_S = 2.0
HOLD_S = 6.0


def load_runner():
    spec = importlib.util.spec_from_file_location("probe_live", RUNNER)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def ok_body(content="", model="mock-model-1", tool=None, usage=None):
    msg = {"role": "assistant", "content": content}
    if tool is not None:
        msg["tool_calls"] = [{"type": "function", "function": tool}]
    return json.dumps({"model": model, "choices": [{"message": msg}],
                       "usage": usage if usage is not None else {"prompt_tokens": 1}}).encode()


# case -> (status, body bytes, delay seconds). The case is the last user
# message of the request body.
CASES = {
    "ok": (200, ok_body("42", usage={"prompt_tokens": 3, "completion_tokens": 1}), 0),
    "tool": (200, ok_body(None, tool={"name": "record_number", "arguments": "{\"n\": 7}"}), 0),
    "429": (429, b'{"error":"rate limited"}', 0),
    "timeout": (200, ok_body("late"), HOLD_S),
    "503": (503, b'{"error":"unavailable"}', 0),
    "502": (502, b"bad gateway", 0),
    "400": (400, b'{"error":"bad request"}', 0),
    "401": (401, b"not json either", 0),
    "not-json": (200, b"<html>not json</html>", 0),
    "no-choices": (200, b'{"model":"mock-model-1"}', 0),
    "content-not-text": (200, json.dumps({"choices": [{"message": {"content": {"x": 1}}}]}).encode(), 0),
    "echo-key": None,  # replies with the bearer token it received
    "auth-check": None,  # 200 only if the exact test key was sent, else 401
}


class Mock(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        case = body["messages"][-1]["content"]
        auth = self.headers.get("Authorization") or ""
        if case == "echo-key":
            status, raw, delay = 200, ok_body("echo " + auth.split(" ", 1)[-1]), 0
        elif case == "auth-check":
            good = auth == "Bearer " + KEY
            status, raw, delay = (200, ok_body("auth ok"), 0) if good else (401, b"{}", 0)
        else:
            status, raw, delay = CASES[case]
        if delay:
            time.sleep(delay)
        try:
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)
        except (BrokenPipeError, ConnectionResetError):
            pass


class Server:
    def __enter__(self):
        self.srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Mock)
        self.srv.daemon_threads = True
        self.t = threading.Thread(target=self.srv.serve_forever, daemon=True)
        self.t.start()
        self.base = "http://127.0.0.1:%d" % self.srv.server_address[1]
        return self

    def __exit__(self, *a):
        self.srv.shutdown()
        self.srv.server_close()


def body(case):
    return {"model": "mock-model-1", "messages": [{"role": "user", "content": case}]}


class SendClassification(unittest.TestCase):
    """probe_live.send maps every transport outcome to a replay kind."""

    @classmethod
    def setUpClass(cls):
        cls.mod = load_runner()
        cls.server = Server().__enter__()

    @classmethod
    def tearDownClass(cls):
        cls.server.__exit__()

    def send(self, case, timeout=TIMEOUT_S):
        at, ms, r = self.mod.send(self.server.base, KEY, body(case), timeout)
        self.assertIsInstance(at, int)
        self.assertGreaterEqual(ms, 0)
        return r

    def test_429_is_rate_limited(self):
        self.assertEqual((self.send("429")["kind"], self.send("429")["status"]), ("rate_limited", 429))

    def test_timeout_is_timeout(self):
        r = self.send("timeout", SHORT_TIMEOUT_S)
        self.assertEqual((r["kind"], r["status"]), ("timeout", None))

    def test_5xx_is_http_error(self):
        for case, st in (("503", 503), ("502", 502)):
            r = self.send(case)
            self.assertEqual((r["kind"], r["status"]), ("http_error", st), case)

    def test_4xx_is_http_error(self):
        for case, st in (("400", 400), ("401", 401)):
            r = self.send(case)
            self.assertEqual((r["kind"], r["status"]), ("http_error", st), case)

    def test_unparseable_2xx_is_unparseable_not_ok(self):
        for case in ("not-json", "no-choices", "content-not-text"):
            r = self.send(case)
            self.assertEqual((r["kind"], r["status"]), ("unparseable", 200), case)
            self.assertNotIn("text", r, case)

    def test_connection_refused_is_http_error(self):
        with Server() as s:
            base = s.base
        at, ms, r = self.mod.send(base, KEY, body("ok"), TIMEOUT_S)
        self.assertEqual((r["kind"], r["status"]), ("http_error", 599))

    def test_ok_reply_fields(self):
        r = self.send("ok")
        self.assertEqual(r["kind"], "ok")
        self.assertEqual((r["status"], r["text"], r["model"]), (200, "42", "mock-model-1"))
        self.assertEqual(r["usage_fields"], ["completion_tokens", "prompt_tokens"])
        t = self.send("tool")
        self.assertEqual((t["kind"], t["text"], t["tool_name"], t["tool_args"]),
                         ("ok", "", "record_number", "{\"n\": 7}"))


def run_runner(base, plan, tmp, extra_env=None):
    plan_path = os.path.join(tmp, "plan.jsonl")
    with open(plan_path, "w", encoding="utf-8") as f:
        for qid, case in plan:
            f.write(json.dumps({"id": qid, "body": body(case)}) + "\n")
    env = {k: v for k, v in os.environ.items() if k != KEY_ENV}
    env[KEY_ENV] = KEY
    env.update(extra_env or {})
    replay, log = os.path.join(tmp, "replay.tsv"), os.path.join(tmp, "run.jsonl")
    p = subprocess.run([sys.executable, RUNNER, "--base", base, "--key-env", KEY_ENV,
                        "--plan", plan_path, "--replay", replay, "--log", log,
                        "--timeout", str(E2E_TIMEOUT_S), "--max-delay", "0"],
                       cwd=tmp, env=env, capture_output=True, text=True, timeout=120)
    return p, replay, log


def read_all(*paths):
    out = ""
    for p in paths:
        if os.path.exists(p):
            with open(p, encoding="utf-8") as f:
                out += f.read()
    return out


# Ten deterministic ids, none answered; det-10 is not sent (missing).
DET_PLAN = [
    ("det-01", "429"), ("det-02", "timeout"), ("det-03", "503"), ("det-04", "not-json"),
    ("det-05", "400"), ("det-06", "no-choices"), ("det-07", "502"), ("det-08", "429"),
    ("det-09", "content-not-text"),
]
DET_KINDS = {
    "det-01": ("rate_limited", "429"), "det-02": ("timeout", ""), "det-03": ("http_error", "503"),
    "det-04": ("unparseable", "200"), "det-05": ("http_error", "400"),
    "det-06": ("unparseable", "200"), "det-07": ("http_error", "502"),
    "det-08": ("rate_limited", "429"), "det-09": ("unparseable", "200"),
}


class RunnerEndToEnd(unittest.TestCase):
    def test_replay_kinds_and_no_key_in_outputs(self):
        with Server() as s, tempfile.TemporaryDirectory() as tmp:
            p, replay, log = run_runner(s.base, DET_PLAN + [("cap-tool", "tool")], tmp)
            self.assertEqual(p.returncode, 0, p.stderr)
            lines = read_all(replay).splitlines()
            self.assertEqual(lines[0].split("\t")[0], "question_id")
            got = {}
            for line in lines[1:]:
                f = line.split("\t")
                self.assertEqual(len(f), 10, line)
                got[f[0]] = (f[3], f[4])
            want = dict(DET_KINDS, **{"cap-tool": ("ok", "200")})
            self.assertEqual(got, want)
            for line in read_all(log).splitlines():
                self.assertEqual(set(json.loads(line)),
                                 {"at_ms", "id", "kind", "status", "latency_ms", "model", "usage_fields"})
            self.assertNotIn(KEY, read_all(replay, log) + p.stdout + p.stderr)

            judge = os.environ.get("PROBE_JUDGE")
            if judge:
                # Unparseable, 4xx/5xx, 429 and timeouts are unanswered, never
                # wrong (DECISIONS 13.2), with their counts by reason.
                j = subprocess.run([judge, "20261022", "claude-haiku-4-5", replay, "none"],
                                   capture_output=True, text=True, timeout=60)
                self.assertEqual(j.returncode, 0, j.stderr)
                det = [l for l in j.stdout.splitlines() if l.startswith("deterministic\tuncertain")]
                self.assertEqual(len(det), 1, j.stdout)
                fields = dict(x.split("=", 1) for x in det[0].split("\t")[2:])
                self.assertEqual(fields, {
                    "correct": "0", "wrong": "0", "unanswered": "10", "rate_limited": "2",
                    "timeout": "1", "server_error": "2", "unparseable": "3",
                    "other_http_error": "1", "missing": "1"})

    def test_refuses_to_write_when_a_reply_echoes_the_key(self):
        with Server() as s, tempfile.TemporaryDirectory() as tmp:
            # The send order is shuffled, so the echoing request is the only one.
            p, replay, log = run_runner(s.base, [("det-01", "echo-key")], tmp)
            self.assertNotEqual(p.returncode, 0)
            self.assertIn("refusing to write", p.stderr)
            self.assertNotIn(KEY, read_all(replay, log) + p.stdout + p.stderr)
            self.assertEqual(len(read_all(replay).splitlines()), 1)  # header only
            self.assertEqual(read_all(log), "")

    def test_plain_http_only_for_localhost(self):
        with tempfile.TemporaryDirectory() as tmp:
            # Rejected before any request: no network is touched.
            p, replay, _ = run_runner("http://relay.example", [("det-01", "ok")], tmp)
            self.assertNotEqual(p.returncode, 0)
            self.assertIn("must be https", p.stderr)
            self.assertFalse(os.path.exists(replay))

    def test_key_from_dotenv_when_not_in_the_environment(self):
        with Server() as s, tempfile.TemporaryDirectory() as tmp:
            with open(os.path.join(tmp, ".env"), "w", encoding="utf-8") as f:
                f.write(f'{KEY_ENV}="{KEY}"\n')
            plan_path = os.path.join(tmp, "plan.jsonl")
            with open(plan_path, "w", encoding="utf-8") as f:
                f.write(json.dumps({"id": "det-01", "body": body("auth-check")}) + "\n")
            env = {k: v for k, v in os.environ.items() if k != KEY_ENV}
            replay, log = os.path.join(tmp, "r.tsv"), os.path.join(tmp, "r.jsonl")
            p = subprocess.run([sys.executable, RUNNER, "--base", s.base, "--key-env", KEY_ENV,
                                "--plan", plan_path, "--replay", replay, "--log", log,
                                "--timeout", str(E2E_TIMEOUT_S)],
                               cwd=tmp, env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual(p.returncode, 0, p.stderr)
            # The mock answers 200 only for exactly "Bearer <key>": the quotes
            # in .env were stripped and the key was sent as is.
            f = read_all(replay).splitlines()[1].split("\t")
            self.assertEqual((f[0], f[3], f[4], f[7]), ("det-01", "ok", "200", "auth ok"))
            self.assertNotIn(KEY, read_all(replay, log) + p.stdout + p.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)

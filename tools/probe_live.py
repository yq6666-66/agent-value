#!/usr/bin/env python3
"""Local live runner for the minimal probe (v22 §11.1). CI never runs it
against a real endpoint; tools/test_probe_live.py tests it against a mock
server on 127.0.0.1.

Sends the request bodies printed by `cargo run -p agent-value-core --example
probe_plan -- <seed> <model>` to an OpenAI Chat Completions endpoint and
writes the replay format read by `probe_judge` / `probe::parse_replay`.

The key is read from the environment variable named by --key-env (or from a
local, git-ignored .env file). It is never printed, logged or written: the
outputs hold only time, question id, status, latency, the returned `model`
field, usage field NAMES, the reply text and the first tool call. Request
headers are not recorded. Every output line, and every raw value before
escaping, is checked not to contain the key before it is written. The base
URL must be https:// (plain http is accepted only for a local mock server).

Usage:
  probe_live.py --base https://relay.example --key-env RELAY_API_KEY \\
                --plan plan.jsonl --replay out.tsv --log out.jsonl
Standard library only.
"""
import argparse, json, os, random, sys, time, urllib.error, urllib.parse, urllib.request

HEADER = "question_id\tat_ms\tlatency_ms\tkind\tstatus\tmodel\tusage_fields\ttext\ttool_name\ttool_args"


def load_key(var):
    v = os.environ.get(var)
    if v:
        return v
    if os.path.exists(".env"):
        with open(".env", encoding="utf-8") as f:
            for line in f:
                k, _, val = line.strip().partition("=")
                if k == var and val:
                    return val.strip().strip('"').strip("'")
    sys.exit(f"{var} is not set (environment or .env)")


def esc(s):
    s = s if isinstance(s, str) else ""
    return s.replace("\\", "\\\\").replace("\t", "\\t").replace("\n", "\\n").replace("\r", "\\r")


def send(base, key, body, timeout):
    req = urllib.request.Request(base.rstrip("/") + "/v1/chat/completions",
                                 data=json.dumps(body).encode(), method="POST")
    req.add_header("Authorization", "Bearer " + key)
    req.add_header("Content-Type", "application/json")
    at = int(time.time() * 1000)
    t0 = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            status, raw = r.status, r.read()
    except urllib.error.HTTPError as e:
        status, raw = e.code, e.read()
    except (TimeoutError, OSError) as e:  # socket timeout, connection errors
        ms = int((time.monotonic() - t0) * 1000)
        kind = "timeout" if "timed out" in str(e).lower() or isinstance(e, TimeoutError) else "http_error"
        return at, ms, {"kind": kind, "status": 599 if kind == "http_error" else None}
    ms = int((time.monotonic() - t0) * 1000)
    if status == 429:
        return at, ms, {"kind": "rate_limited", "status": 429}
    if not 200 <= status < 300:
        return at, ms, {"kind": "http_error", "status": status}
    try:
        j = json.loads(raw)
        msg = j["choices"][0]["message"]
        tc = (msg.get("tool_calls") or [None])[0]
        if tc is not None:
            tc = {"function": {"name": str(tc["function"]["name"]),
                               "arguments": str(tc["function"].get("arguments", ""))}}
        if not isinstance(msg.get("content") or "", str):
            raise ValueError("content is not text")
    except Exception:
        return at, ms, {"kind": "unparseable", "status": status}
    usage = j.get("usage") if isinstance(j.get("usage"), dict) else {}
    return at, ms, {
        "kind": "ok", "status": status,
        "model": j.get("model") if isinstance(j.get("model"), str) else None,
        "usage_fields": sorted(str(k) for k in usage.keys()),
        "text": msg.get("content") or "",
        "tool_name": tc["function"]["name"] if tc else None,
        "tool_args": tc["function"].get("arguments", "") if tc else None,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", required=True)
    ap.add_argument("--key-env", required=True)
    ap.add_argument("--plan", required=True)
    ap.add_argument("--replay", required=True)
    ap.add_argument("--log", required=True)
    ap.add_argument("--timeout", type=float, default=60.0)
    ap.add_argument("--max-delay", type=float, default=4.0, help="random pause between requests (§11.1)")
    a = ap.parse_args()
    u = urllib.parse.urlsplit(a.base)
    if u.scheme != "https" and not (u.scheme == "http" and u.hostname in ("127.0.0.1", "localhost", "::1")):
        sys.exit("--base must be https:// (plain http only for a local mock server)")
    key = load_key(a.key_env)
    needles = {key, esc(key), json.dumps(key)[1:-1]}
    plan = [json.loads(l) for l in open(a.plan, encoding="utf-8") if l.strip()]
    random.shuffle(plan)  # send order is randomised too
    with open(a.replay, "w", encoding="utf-8") as rp, open(a.log, "w", encoding="utf-8") as lg:
        rp.write(HEADER + "\n")
        for i, p in enumerate(plan):
            if i:
                time.sleep(random.uniform(1.0, a.max_delay))
            at, ms, r = send(a.base, key, p["body"], a.timeout)
            line = "\t".join([p["id"], str(at), str(ms), r["kind"], str(r.get("status") or ""),
                              esc(r.get("model")), ",".join(r.get("usage_fields", [])), esc(r.get("text")),
                              esc(r.get("tool_name")), esc(r.get("tool_args"))])
            log = json.dumps({"at_ms": at, "id": p["id"], "kind": r["kind"], "status": r.get("status"),
                              "latency_ms": ms, "model": r.get("model"),
                              "usage_fields": r.get("usage_fields", [])}, ensure_ascii=False)
            raw = [r.get("model"), r.get("text"), r.get("tool_name"), r.get("tool_args")] + list(r.get("usage_fields", []))
            for out in [line, log] + [x for x in raw if isinstance(x, str)]:
                if any(n and n in out for n in needles):
                    sys.exit("refusing to write: output would contain the key")
            rp.write(line + "\n")
            lg.write(log + "\n")
            print(f"{p['id']}\t{r['kind']}\t{r.get('status')}\t{ms} ms", flush=True)


if __name__ == "__main__":
    main()

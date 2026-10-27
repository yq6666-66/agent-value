# agent-value

[简体中文](README.zh-CN.md)

A local desktop tool for personal use (Tauri; build from source; no install package today). It scores the (model, route) pairs you actually use on cost, time, and quality, with weights set to your own preferences. It is not a public leaderboard: numbers come from your own tasks, and data stays on your machine. Scoring follows out-of-repo spec v22 and decision records; this README covers only what is implemented.

## Problem

Public boards and generic benchmarks do not match the models and routes you use. You need on-device comparison of arms (model + route) on value and authenticity signals, driven by your own tasks and preferences, without sending the data away.

## Features

1. **Coding-scope window scoring**: frontier, eligibility, and recommendation; shows authenticity and weight sources. The first window may show the frontier labeled “not yet calibrated” and omits V. Tier thresholds stay provisional; authenticity is unsettled and for reference only. Observation runs and full-window calibration are permanently cancelled.
2. **Arms**: list / add; show database path; export; clear all records.
3. **Five-level self-report wizard** (weight elicitation); sensitivity panel (winner chart, robustness labels, flip sentences; chart hidden on the first window or when there is no anchor layer).
4. **Probe**: records after consent; shows recorded / replay results; after consent you can run once manually; optional in-app timed reruns (off by default; after enable, waits a provisional interval before the first send; stops when the app quits; not OS cron; opening the app does not send; monthly budget stop is reflected in the panel).
5. **Fingerprints**: the library can collect official direct-connection fingerprint baselines over HTTP into the local store (tests may use a mock). The desktop UI has no one-click “collect baseline” button.

## Technology

1. Rust core library + local SQLite store + Tauri desktop shell; the web view does not recompute scores (the shell reads the store and asks core).
2. Probe HTTP path and fingerprint collection library are landed; secrets use environment variable names only and never enter the repo.
3. Spec steps 1–13 on the judgment side are implemented. Scoring core (full edge cases: see the core library and in-repo long docs):

An arm may try up to \(K=3\) times on unit \(u\). Let \(p_1\) be the Jeffreys–Beta posterior mean success probability on one try, and \(\pi\) the success probability on each retry after a failure (caller-supplied). Then

$$
\begin{aligned}
q_K&=(1-p_1)(1-\pi)^{K-1},&
p_K&=1-q_K,\\
E[n_F]&=(1-p_1)\sum_{j=0}^{K-1}(1-\pi)^{j},&
E[n_W]&=E[n_F]-q_K .
\end{aligned}
$$

Here \(p_K\) is the probability of success within the arm’s own \(K\) tries, \(q_K\) the probability of handing off to the fallback after all \(K\) fail, \(E[n_F]\) the expected number of own failures, and \(E[n_W]\) the expected waits before a retry (excluding the final failure that does not retry). Unit expected cost, active time, and delivery quality. Let arm index be \(i\) and unit index be \(u\).

$$
\begin{aligned}
C^*_{iu}&=p_K\tilde C_S+E[n_F]\tilde C_F+q_K\,C^*_{F,u}+\frac{n^C\hat{\bar C}_C}{n^1+n^0},\\
T^*_{iu}&=p_K\tilde T_S+E[n_F]\tilde T_F+E[n_W]D+q_K\,T^*_{F,u}+\frac{n^C\bar T_C}{n^1+n^0},\\
Q^*_{iu}&=p_K\hat q+q_K\bar Q_F .
\end{aligned}
$$

Left to right: successful try, failed tries, fallback handoff, cancel amortization (time also includes retry gap \(D\)). \(n^1,n^0,n^C\) are success, failure, and cancel counts; if \(n^1+n^0=0\), that unit dimension is \(\mathrm{NA}\). \(\tilde C_S,\tilde C_F\) (and likewise for time) are winsorized then shrinkage-to-baseline means on the success/failure sides; \(\hat{\bar C}_C,\bar T_C\) are mean cancel cost/time; \(\hat q\) is the arm’s success quality estimate and \(\bar Q_F\) the quality when the fallback finishes after handoff (shrunk toward the fallback unit quality). Reference and fallback arms have zero handoff terms. \(Q^*\in[0,1]\).

Layer and overall \(C^*,T^*,Q^*\) are weighted aggregates over positive-weight units and layers (any positive-weight \(\mathrm{NA}\) unit makes the aggregate \(\mathrm{NA}\)). Value uses these **aggregated** quantities versus the reference anchor \(C^{\mathrm{ref}},T^{\mathrm{ref}},Q^{\mathrm{ref}}\) (the reference arm’s aggregated values), and requires all three of \(C^*,T^*,Q^*\) non-\(\mathrm{NA}\):

$$
V=100\left(\frac{C^{\mathrm{ref}}+c_0}{C^*+c_0}\right)^{a}
\left(\frac{T^{\mathrm{ref}}+t_0}{T^*+t_0}\right)^{b}
\left(\frac{1-Q^{\mathrm{ref}}+\epsilon}{1-Q^*+\epsilon}\right)^{g}.
$$

\(V=100\) matches the reference; higher is better. \(a,b,g\) are exponential weights on cost, time, and quality with \(a+b+g=1\) and each \(\ge0.05\) (default \(1/3\)), from five-level self-report. Smoothing constants: \(c_0=0.15\) CNY, \(t_0=15\) s, \(\epsilon=0.02\). The first scoring window may show the frontier without \(V\) (weights not yet calibrated). Tier thresholds stay provisional; authenticity is unsettled and for reference only (observation runs and full-window calibration permanently cancelled).

## Usage

1. Build and run the desktop app from source. Build the web frontend first (optional examples: `npm ci` / `npm run build`), then `cargo build` under `src-tauri/` (its own Cargo package), or use `cargo tauri dev` for local development and run the resulting binary. Tauri 2 system dependencies are required (on Linux: WebKitGTK 4.1, etc.). No install package is provided.
2. In the app, manage arms and run self-report / sensitivity; after consent, run a probe once manually or view replay, or enable in-app timed reruns (see Features).
3. Optional examples: `cargo test --workspace` for the core; `cargo run -p agent-value-core --example probe_plan` / `probe_judge` for probes.
4. Version history lives in `CHANGELOG.md`; this README has none.

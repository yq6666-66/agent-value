# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Core, spec step 9: diagnostic table for arms with miss/fail cells (§5) —
  ESS admission (`n_comp + κ_eff`, κ_eff = §1 prior κ averaged to the
  layer by `ω_u`, per v22 §0; a `d+` cell without a prior counts 0),
  miss fill via weighted lower median (DECISIONS E) and worst-valid per
  dimension (DECISIONS F), fail fill as the worse of measured and
  worst-valid; sort by coverage then model name. Layer frontiers
  `F_off` / `F_rob` / `F_rec` (§6); arms with an NA triple are skipped,
  and an NA robust triple also keeps an arm out of `F_rec`;
  lower-bound-only cancel-bill arms stay in `F_rec` and are listed in
  `coarse_bill` for the "账单过粗" tag (§2).
  Eligibility primary attribution maps seven labels to five §5 gates and
  suppresses spurious rate gates when complete-case already failed
  (DECISIONS P). Paired with v22 examples 6/7/8 (F_thin / F_bad paths;
  diagnostic anchor 0.7875).
- Core, spec step 8: layer eligibility set `E_d` (§5). Five gates —
  complete-case (`N^miss=N^fail=0`), sample (`n^comp >= max(30, 3 U_d)`),
  cancel and completion rates via integer comparisons
  (`5 n^C <= N^sub`, `2 n^comp >= N^sub`, with `N^sub > 0` first),
  authenticity / log-integrity (stub inputs; A/A−/B pass, B labelled
  undetermined, C fails), and reference-base (every `d+` cell has a
  complete `(C*,T*,Q*)` for R). `LayerStatus` now also exposes `n_c` and
  `N^sub`. Diagnostic-table rules and F_k eligibility belong to later
  steps.
- Core, spec steps 6-7: per-cell `C*`, `T*`, `Q*` (§2, §3): winsorized side
  means, shrinkage to the benchmark (`kappa_C = 5`, none without a
  benchmark), empty-sample chain benchmark -> layer pool -> NA, cancel
  amortization with the cancel-mean chain, fallback arm `F_k` (default
  `R_k`; reference and fallback are terminal), NA propagation, retry-wait
  term `E[n_W] D`, matched-failure quality `Qbar_F`; cancel-robust versions
  with per-task / capped / lower-bound excess and the monotone guards.
  Aggregation over positive-weight cells and layers (NA stays NA) and the
  value score `V` in log space with per-scope floors (§4).
- Core, spec steps 4-5: Beta CDF and quantile (own implementation, checked
  against 40-digit references); prior `p_B` / `kappa` exactly as written
  (`rho = 1` direct pooling as its own case, `0/0` gives 0); posterior,
  `q_K` then `p_K = 1 - q_K`, `E[n_F]` by geometric sum (no cancellation
  at tiny `pi`), `E[n_W]`; lower-bound fail test via the CDF, quantile for
  display; cancel-robust variant; cell states miss / F_thin / F_bad / valid
  over the positive-weight cells with counts and weight sums; `omega_u`
  from the previous window's user-task shares with carry-over / equal
  fallbacks.
- Core, spec step 3: task outcome judgement (§3.1 rules 1-6 in order:
  explicit abort, failed last check, redo within 30 min, whole rollback
  within 24 h for coding tasks, reported completion, else undetermined),
  with provisional results until the observation windows close; quality
  signals: star-to-quality mapping, per-layer rating / success-rate mode,
  pooled prior `q_B`, shrunk `q_hat`, spot-rating decision (10%, max 3 per
  Beijing-time day).
- Store, spec step 3: schema version 2 with an append-only
  `outcome_judgements` history (rule fired, rules version); recording a
  judgement updates the task's current outcome; replaceable task ratings.
- Store, spec step 2: new `agent-value-store` crate with a versioned SQLite
  schema for arms, tasks, per-request route metadata, behaviour events with
  available-arm snapshots, probe results (Beijing-time run days), frozen cell
  weights, weight versions with frozen windows, and configuration versions.
  Version rows are immutable (triggers). Privacy: no key-value column, user
  text kept only as category + length + keyed digest, 180-day purge, export
  and delete-all (with WAL checkpoint and VACUUM).
- Core, spec step 2: design cells with the reserved probe cell (weight
  always 0), cell weights, privacy checks (env var names, secret-like
  values, text facts), run days, data-model records, integer money
  (micro-yuan), and ledger reconstruction from stored parts.
- CI tests and lints the whole Cargo workspace.
- Core, spec step 1: all v22 §9 constants (checked row by row against a verbatim
  copy of the §9 table); scoring scopes (default coding / non-coding, gated
  subdivision); arms as (model, route) with route = provider kind + host +
  protocol + client; fixed-length time windows with per-window freezing;
  weight versions with source, evidence, effective window and trial versions;
  first-window rule (no V). All bad input returns `Result` errors.
- Repository skeleton: Cargo workspace with the pure-Rust `agent-value-core` crate,
  a minimal Tauri 2 shell in `src-tauri/` (outside the default workspace),
  and a minimal React + TypeScript + Vite display app in `web/`.
- CI workflow: rustfmt, clippy (`-D warnings`) and tests for the core crate;
  `npm ci`, lint, type check and build for the web app; Tauri 2 Linux system
  dependencies plus `cargo check` and `cargo clippy` for the desktop shell.
- Apache-2.0 license, empty third-party notices, `.env.example` with variable names only.

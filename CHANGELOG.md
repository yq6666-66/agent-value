# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Changed
- Core, step 10 review follow-up: the diagnostic reference anchor takes
  the worst valid value per dimension, so a valid reference cell with one
  NA dimension still contributes its other dimensions (§7 line 570).
- Core: stricter input checks. All six `overall_arm` counters (`n_comp`,
  `u`, `n_c`, `n_sub`, `n_miss`, `n_fail`) use checked addition and return
  an overflow error instead of wrapping; fallback layer weights that are
  NaN, infinite or negative are an error (`fallback_failures` and
  `resolve_fallback` now return `Result`); a non-finite `κ_eff` is not
  admitted; duplicate cells in fallback ratings, duplicate arm ids in
  `layer_pick` scores and duplicate arm ids passed to `overall_top` are
  rejected.

### Added
- Core, spec step 12: weight sensitivity data (§12), new module
  `sensitivity`. Winner map on the 171-point grid: `Win(w)` is the tie
  group of highest `V` among the compared arms, ties reported whole;
  withheld in the first window and when the reference has no anchor
  (tier III). Each grid point is flagged for `Ω_s` and `Ω_b`, which are
  inputs (validated: on the 0.05 grid, no duplicates; `Ω_s` must be
  non-empty; an empty `Ω_b` means a behaviour signal with no qualifying
  grid point and is accepted, `None` means no behaviour signal; the
  report says which).
  Weight-robust winner over `Ω_s`, or over `max|w − W_k| ≤ 0.10` when
  there is no self-report, with per-arm shares and "在你的权重范围内都是它"
  only for a sole winner at every point. Statistical robustness: 200-resample
  task bootstrap stratified by (arm, cell), so every `n_iu`, `ω_u` and the
  design grid stay fixed and the reference and fallback arms are
  resampled like the others; seeded xorshift64 (seed recorded, 0
  rejected), caller recomputes each resample; reports winner frequency
  at `W_k`, `F_off` frequency, ties and resamples with an anchor. Flip
  line between the top two arms (`aΔ_C + bΔ_T + gΔ_D = 0`) with one
  statement per fixed dimension, evaluated on the feasible segment; the
  printed threshold is rounded up so the sentence is a sufficient
  condition. Withheld as well when there is no arm to compare. Every
  winner group is checked to hold an `F_off` arm (§12 property 1) in all
  builds; an arm dominated by a hair may share a tie group with its
  dominator.
  Noise-floor robustness at `W_k` under `c0`, `t0`, `ε` × 0.5 and × 2.
  All counters use checked addition. Paired with the spec example (Y
  wins 93 grid points, X 78; Y wins 14 of 14 in the self-report region).
- README (English and Chinese): section 10, weight sensitivity (§12),
  and its roadmap row and constants; built on the step 11 README.

- Core, spec step 10: reference anchor certification (§7). Tiers are
  decided III → II → I, first match wins; tier III lists every reason
  (miss, true-fail cell, NA cell, fewer than 3 valid cells), so a layer
  with fewer than 3 positive-weight cells is never certified. Tier II
  clamps the layer anchor, never cell values: cost and time
  `clip(A, (1−θ)a, (1+θ)a)`, quality through defects `D = 1 − Q`,
  `θ = 0.10`, with the clamp metadata kept. Layer `V` uses the anchor and
  is never normalized by the pure-route score. Unclamped diagnostic
  reference anchor per the §7 formula. Layer official #1 is the tie group
  of highest `V` within `F_rec` (tiers I/II only); with `F_rec` empty,
  `F_rob` is listed as "取消稳健备选" and nobody is #1. Overall closure
  (§6): the reference must be tier I or II in every positive-weight
  layer (overall anchor = `Σ ω_d` of layer anchors in all three
  dimensions, "含 Ⅱ 档夹逼层" header); an arm gets an overall `V` only when
  it passes the grid gates and is in `F_all`; overall triple, `cov_i` and
  the partial-coverage flag are reported. `robust_enabled` and the
  "账单过粗" tag are now set from cell notes; a tagged arm can be #1 and
  keeps its tag on every row. §13: `R_k` selection (user → official
  direct → most tasks, availability = authenticity passed and no
  `F_bad`, ties by arm id) and `F_k` qualification (authenticity, no
  `F_bad`, tier I/II, at least 3 quality ratings per cell; otherwise
  revert to `R_k` with a version note). Diagnostic rows report
  `not_admitted` reasons. Display strings live in `labels`. Paired with
  v22 examples 6/7/8 composed from `E_d` to the official pick.

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

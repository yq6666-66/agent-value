# Test fixtures

- `v22-section9-table.md`: the §9 constants table of the v22 spec.
- `step11_*`: output of the §10.2 reference script (`elicit5.py`), generated
  once and frozen:
  - `step11_bank.csv`: the 96 bank questions in order, ratios as printed by
    Python `repr`.
  - `step11_levels.txt`: predicted level + 2 for every question (row) and
    grid point (column, grid order).
  - `step11_honest_runs.csv`: for every true weight on the grid, honest
    answers: question count, `question:level` sequence, core and wide
    regions (points as `i-j` twentieths).
  - `step11_boundary_528.csv`: the 528 (w, q) pairs whose rounded `|u|` is
    exactly 0.25 or 0.50; `level_expected` is the reference level,
    `level_if_unrounded` the level without the 1e-9 rounding.
- `step13_replay_mixed.tsv`: hand-written probe replay for seed 20261022,
  one row per branch (429, timeout, 5xx, unparseable, correct, wrong,
  tool call, fenced JSON).
- `step13_live_replay.tsv`: the redacted replay of the one real-relay
  minimal probe run (seed 20261022, claude-haiku-4-5, OpenAI Chat
  Completions protocol), recorded by `tools/probe_live.py`. It holds the
  reply text, returned model and usage field names only: no key, no
  headers, no prompts. The expected answers for both seeds come from the
  independent Python reference generator (see `tests/step13_probes.rs`).

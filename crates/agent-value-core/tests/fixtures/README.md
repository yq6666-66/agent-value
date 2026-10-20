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

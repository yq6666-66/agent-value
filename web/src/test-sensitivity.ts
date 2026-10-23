import type { SensitivityChart, SensitivityPoint, SensitivityView } from "@/lib/sensitivity";

export function sensitivityPoint(
  overrides: Partial<SensitivityPoint> = {},
): SensitivityPoint {
  return {
    a: 0.2,
    b: 0.35,
    g: 0.45,
    winners: ["Y"],
    in_omega_s: false,
    in_omega_b: false,
    ...overrides,
  };
}

export function sensitivityChart(
  overrides: Partial<SensitivityChart> = {},
): SensitivityChart {
  return {
    points: [
      sensitivityPoint({ a: 0.2, b: 0.4, winners: ["Y"], in_omega_s: true }),
      sensitivityPoint({ a: 0.5, b: 0.2, winners: ["X"], in_omega_b: true }),
    ],
    current_winners: ["Y"],
    omega_mark: "MARK-FROM-CORE",
    omega_copy: "COPY-FROM-CORE",
    everywhere_id: "Y",
    shares: [
      { id: "X", fraction: "0/14" },
      { id: "Y", fraction: "14/14" },
    ],
    behaviour_mark: null,
    behaviour_points: null,
    sentences: [
      { fixed: "成本", text: "SENTENCE-COST" },
      { fixed: "质量", text: "SENTENCE-QUALITY" },
    ],
    no_flip_text: null,
    floor_copy: "FLOOR-FROM-CORE",
    ...overrides,
  };
}

export function sensitivityView(overrides: Partial<SensitivityView> = {}): SensitivityView {
  return {
    source: "store",
    fixture: false,
    show_chart: false,
    withheld: "first-window",
    withheld_text: "WITHHELD-FROM-CORE",
    empty_reason: null,
    chart: null,
    ...overrides,
  };
}

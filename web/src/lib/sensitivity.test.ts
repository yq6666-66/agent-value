import { expect, test } from "vitest";

import { sensitivityView } from "@/lib/sensitivity";

test("a chart flag without a chart is rejected", () => {
  expect(() =>
    sensitivityView({
      source: "grid",
      fixture: true,
      show_chart: true,
      withheld: null,
      withheld_text: null,
      empty_reason: null,
      chart: null,
    }),
  ).toThrow(/incomplete/);
});

test("a withheld payload drops any chart object", () => {
  const view = sensitivityView({
    source: "first",
    fixture: true,
    show_chart: false,
    withheld: "first-window",
    withheld_text: "WITHHELD",
    empty_reason: null,
    chart: { points: [] },
  });
  expect(view.show_chart).toBe(false);
  expect(view.chart).toBeNull();
  expect(view.withheld_text).toBe("WITHHELD");
});

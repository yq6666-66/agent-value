import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import {
  ELICIT_NEXT_COMMAND,
  ELICIT_SKIP_COMMAND,
  fetchNext,
  formatMetric,
  saveView,
  skipQuestion,
  sourceLabel,
  stepView,
} from "@/lib/elicit";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const step = {
  finished: false,
  answered: 0,
  skipped: [],
  wide_len: 171,
  core_len: 171,
  m: 0,
  question: {
    index: 2,
    rho: [1.3, 0.5, 1],
    shown: {
      cost: [8.25, 0.5],
      time: [120, 90],
      defect: [0.1, 0.1],
    },
  },
};

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

test("formats a shell number without changing other digits", () => {
  expect(formatMetric(0.5)).toBe("0.5");
  expect(formatMetric(1)).toBe("1");
  expect(formatMetric(8.25)).toBe("8.25");
  expect(formatMetric(1 / 3)).toBe("0.333333");
});

test("maps source tags to display labels", () => {
  expect(sourceLabel("self-report")).toBe("自报");
  expect(sourceLabel("manual")).toBe("手动");
  expect(sourceLabel("default")).toBe("默认");
});

test("rejects a step that omits the shown costs", () => {
  const broken = structuredClone(step);
  broken.question.shown.cost = [8.25];
  expect(() => stepView(broken)).toThrow(/incomplete/);
});

test("rejects a save outcome the shell does not use", () => {
  expect(() =>
    saveView({
      outcome: "frozen",
      label: null,
      applied: null,
      official: null,
      point: null,
    }),
  ).toThrow(/incomplete/);
});

test("skip and next pass the shell argument names", async () => {
  vi.mocked(invoke).mockResolvedValue(step);
  await skipQuestion(4);
  expect(invoke).toHaveBeenCalledWith(ELICIT_SKIP_COMMAND, { question: 4 });
  await fetchNext();
  expect(invoke).toHaveBeenCalledWith(ELICIT_NEXT_COMMAND);
});

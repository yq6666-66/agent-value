import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { WeightPanel } from "@/components/weight-panel";
import {
  BEGIN_WINDOW_COMMAND,
  EDIT_WEIGHTS_COMMAND,
  ELICIT_ANSWER_COMMAND,
  ELICIT_FINISH_COMMAND,
  ELICIT_REGIONS_COMMAND,
  ELICIT_SAVE_COMMAND,
  ELICIT_START_COMMAND,
  WEIGHT_CONTEXT_COMMAND,
} from "@/lib/elicit";
import { WINDOW_CHROME_COMMAND } from "@/lib/window-chrome";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const chrome = {
  window_index: 0,
  is_first: true,
  first_window_label: "LABEL-FROM-CORE",
};

const openStep = {
  finished: false,
  answered: 0,
  skipped: [],
  wide_len: 171,
  core_len: 171,
  m: 0,
  question: {
    index: 2,
    rho: [1.3, 0.769231, 1],
    shown: {
      cost: [8.25, 0.5],
      time: [120, 90],
      defect: [0.1, 0.04],
    },
  },
};

const doneStep = {
  finished: true,
  answered: 4,
  skipped: [],
  wide_len: 14,
  core_len: 3,
  m: 0,
  question: null,
};

function version(
  id: number,
  a: number,
  b: number,
  g: number,
  source: string,
  trial: boolean,
  effectiveFrom: number | null,
) {
  return { id, a, b, g, source, trial, effective_from: effectiveFrom };
}

const regions = {
  core: [
    { a: 0.2, b: 0.35, g: 0.45 },
    { a: 0.25, b: 0.3, g: 0.45 },
    { a: 0.15, b: 0.4, g: 0.45 },
  ],
  wide: [
    { a: 0.2, b: 0.35, g: 0.45 },
    { a: 0.25, b: 0.3, g: 0.45 },
  ],
  m: 0,
  point: { a: 0.2, b: 0.35, g: 0.45 },
};

const report = {
  regions,
  contradiction: false,
  contradicted: [],
  ranges: [
    [0.15, 0.4],
    [0.2, 0.55],
    [0.25, 0.6],
  ],
};

const inWindow = {
  scope: "coding",
  in_window: true,
  window_index: 0,
  official: version(0, 0.5, 0.25, 0.25, "default", false, 0),
  trial: null,
};

const trialSave = {
  outcome: "trial",
  label: "Non-frozen trial",
  applied: version(1, 0.2, 0.35, 0.45, "manual", true, null),
  official: version(0, 0.5, 0.25, 0.25, "default", false, 0),
  point: { a: 0.2, b: 0.35, g: 0.45 },
};

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

test("the wizard shows shell numbers and a mid-window save stays a trial", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd === WINDOW_CHROME_COMMAND) {
      return chrome;
    }
    if (cmd === ELICIT_START_COMMAND) {
      expect(args).toEqual({ base: null });
      return openStep;
    }
    if (cmd === ELICIT_ANSWER_COMMAND) {
      expect(args).toEqual({ answer: { question: 2, level: 1 } });
      return doneStep;
    }
    if (cmd === ELICIT_REGIONS_COMMAND) {
      return regions;
    }
    if (cmd === ELICIT_FINISH_COMMAND) {
      return report;
    }
    if (cmd === WEIGHT_CONTEXT_COMMAND) {
      return inWindow;
    }
    if (cmd === ELICIT_SAVE_COMMAND) {
      return trialSave;
    }
    throw new Error(cmd);
  });

  render(<WeightPanel />);
  fireEvent.click(await screen.findByRole("button", { name: "五档自报" }));
  fireEvent.click(await screen.findByRole("button", { name: "开始答题" }));

  expect(await screen.findByText(/成本 8\.25 元/)).toBeTruthy();
  expect(screen.getByText(/时间 90 秒/)).toBeTruthy();
  expect(screen.getByText(/缺陷率 0\.04/)).toBeTruthy();
  expect(screen.queryByText("0.65")).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "稍偏甲" }));

  expect(await screen.findByText("重心 成本 0.2 · 时间 0.35 · 质量 0.45")).toBeTruthy();
  expect(screen.getByText("核心区域 3 个点，宽区域 2 个点，偏差 0")).toBeTruthy();
  expect(screen.getByText(/保存只会生成试算，不会改本窗正式权重/)).toBeTruthy();
  expect(screen.queryByText("回答互相矛盾")).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "保存自报" }));

  expect(await screen.findByText("Non-frozen trial")).toBeTruthy();
  expect(screen.getByText("本窗正式 成本 0.5 · 时间 0.25 · 质量 0.25 · 来源 默认")).toBeTruthy();
  expect(screen.getByText("试算 成本 0.2 · 时间 0.35 · 质量 0.45 · 来源 手动")).toBeTruthy();
  expect(screen.getByText("本窗正式权重未改。")).toBeTruthy();

  const names = vi.mocked(invoke).mock.calls.map((call) => call[0]);
  expect(names).toContain(ELICIT_START_COMMAND);
  expect(names).toContain(ELICIT_ANSWER_COMMAND);
  expect(names).toContain(ELICIT_REGIONS_COMMAND);
  expect(names).toContain(ELICIT_FINISH_COMMAND);
  expect(names).toContain(ELICIT_SAVE_COMMAND);
  expect(names).not.toContain(EDIT_WEIGHTS_COMMAND);
});

test("a contradiction is the shell's flag and question list", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === WINDOW_CHROME_COMMAND) {
      return chrome;
    }
    if (cmd === ELICIT_START_COMMAND) {
      return doneStep;
    }
    if (cmd === ELICIT_REGIONS_COMMAND) {
      return { ...regions, m: 2 };
    }
    if (cmd === ELICIT_FINISH_COMMAND) {
      return { ...report, contradiction: true, contradicted: [47, 34], regions: { ...regions, m: 2 } };
    }
    if (cmd === WEIGHT_CONTEXT_COMMAND) {
      return { ...inWindow, in_window: false, window_index: null };
    }
    throw new Error(cmd);
  });

  render(<WeightPanel />);
  fireEvent.click(await screen.findByRole("button", { name: "五档自报" }));
  fireEvent.click(await screen.findByRole("button", { name: "开始答题" }));

  expect(await screen.findByText("回答互相矛盾：题号 47、34")).toBeTruthy();
  expect(screen.getByText(/尚未开窗。保存会写成正式自报/)).toBeTruthy();
});

test("a mid-window weight edit creates a trial and leaves the official weights", async () => {
  const closed = {
    scope: "coding",
    in_window: false,
    window_index: null,
    official: version(0, 0.5, 0.25, 0.25, "default", false, 0),
    trial: null,
  };
  const opened = { ...closed, in_window: true, window_index: 0 };
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd === WINDOW_CHROME_COMMAND) {
      return chrome;
    }
    if (cmd === WEIGHT_CONTEXT_COMMAND) {
      return closed;
    }
    if (cmd === BEGIN_WINDOW_COMMAND) {
      return opened;
    }
    if (cmd === EDIT_WEIGHTS_COMMAND) {
      expect(args).toEqual({ weights: { a: 0.2, b: 0.3, g: 0.5 } });
      return {
        label: "Non-frozen trial",
        official: opened.official,
        trial: version(1, 0.2, 0.3, 0.5, "manual", true, null),
      };
    }
    throw new Error(cmd);
  });

  render(<WeightPanel />);
  fireEvent.click(await screen.findByRole("button", { name: "窗中改权重" }));

  expect(await screen.findByText(/尚未开窗/)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "保存试算" })).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "开始本窗" }));
  fireEvent.change(await screen.findByRole("textbox", { name: "成本权重" }), {
    target: { value: "abc" },
  });
  fireEvent.click(await screen.findByRole("button", { name: "保存试算" }));
  expect(await screen.findByText("请填写三个数字")).toBeTruthy();
  expect(invoke).not.toHaveBeenCalledWith(EDIT_WEIGHTS_COMMAND, expect.anything());

  fireEvent.change(screen.getByRole("textbox", { name: "成本权重" }), {
    target: { value: "0.2" },
  });
  fireEvent.change(screen.getByRole("textbox", { name: "时间权重" }), {
    target: { value: "0.3" },
  });
  fireEvent.change(screen.getByRole("textbox", { name: "质量权重" }), {
    target: { value: "0.5" },
  });
  fireEvent.click(screen.getByRole("button", { name: "保存试算" }));

  expect(await screen.findByText("Non-frozen trial")).toBeTruthy();
  expect(screen.getByText("本窗正式 成本 0.5 · 时间 0.25 · 质量 0.25")).toBeTruthy();
  expect(screen.getByText("试算 成本 0.2 · 时间 0.3 · 质量 0.5")).toBeTruthy();
  expect(screen.getByText("本窗正式权重未改。")).toBeTruthy();

  const names = vi.mocked(invoke).mock.calls.map((call) => call[0]);
  expect(names).toContain(BEGIN_WINDOW_COMMAND);
  expect(names).toContain(EDIT_WEIGHTS_COMMAND);
  expect(names).not.toContain(ELICIT_SAVE_COMMAND);
  expect(names.filter((name) => name === EDIT_WEIGHTS_COMMAND)).toHaveLength(1);
});

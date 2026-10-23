import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { App } from "@/App";
import { WeightPanel } from "@/components/weight-panel";
import { WINDOW_CHROME_COMMAND } from "@/lib/window-chrome";
import { WINDOW_RESULT_COMMAND } from "@/lib/window-result";
import { armView, windowResultFixture } from "@/test-window-result";
import {
  DELETE_ALL_COMMAND,
  EXPORT_STORE_COMMAND,
  LIST_ARMS_COMMAND,
  STORE_DATA_PATH_COMMAND,
  UPSERT_ARM_COMMAND,
} from "@/lib/store";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

function chromeFor(sample: unknown) {
  if (sample === "later") {
    return { window_index: 1, is_first: false, first_window_label: null };
  }
  return { window_index: 0, is_first: true, first_window_label: "LABEL-FROM-CORE" };
}

function sampleOf(args: unknown): unknown {
  if (args && typeof args === "object" && "sample" in args) {
    return args.sample;
  }
  return undefined;
}

test("first window shows the shell label and does not show V", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd === WINDOW_CHROME_COMMAND) {
      return chromeFor(sampleOf(args));
    }
    throw new Error(cmd);
  });

  render(<WeightPanel />);

  expect(await screen.findByText("LABEL-FROM-CORE")).toBeTruthy();
  expect(screen.queryByText("V")).toBeNull();
  expect(screen.queryByText(/collecting data only/)).toBeNull();
  expect(invoke).toHaveBeenCalledWith(WINDOW_CHROME_COMMAND, { sample: "first" });
});

test("a later window shows the store V text from the shell", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd === WINDOW_CHROME_COMMAND) {
      return chromeFor(sampleOf(args));
    }
    if (cmd === WINDOW_RESULT_COMMAND) {
      return windowResultFixture({
        official_top: ["own"],
        tied: false,
        arms: [armView({ id: "own", v_text: "88.00-from-shell" })],
        v_withheld: null,
        empty_reason: null,
      });
    }
    throw new Error(cmd);
  });

  render(<WeightPanel />);
  fireEvent.click(await screen.findByRole("button", { name: "下一窗" }));

  expect(await screen.findByText("V")).toBeTruthy();
  expect(screen.getByText("own 88.00-from-shell")).toBeTruthy();
  expect(screen.queryByText("尚未接入")).toBeNull();
  expect(screen.queryByText("LABEL-FROM-CORE")).toBeNull();
  expect(invoke).toHaveBeenCalledWith(WINDOW_CHROME_COMMAND, { sample: "later" });
  expect(invoke).toHaveBeenCalledWith(WINDOW_RESULT_COMMAND, { source: "store" });
});

test("a later window shows a withheld V from the shell", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd === WINDOW_CHROME_COMMAND) {
      return chromeFor(sampleOf(args));
    }
    if (cmd === WINDOW_RESULT_COMMAND) {
      return windowResultFixture({
        official_top: [],
        arms: [],
        v_withheld: "WITHHELD-FROM-SHELL",
        empty_reason: null,
      });
    }
    throw new Error(cmd);
  });

  render(<WeightPanel />);
  fireEvent.click(await screen.findByRole("button", { name: "下一窗" }));

  expect(await screen.findByText("WITHHELD-FROM-SHELL")).toBeTruthy();
  expect(screen.queryByText("尚未接入")).toBeNull();
});

test("the shell layout has a frontier pane and resize separators", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === WINDOW_CHROME_COMMAND) {
      return chromeFor("first");
    }
    if (cmd === LIST_ARMS_COMMAND) {
      return [];
    }
    if (cmd === STORE_DATA_PATH_COMMAND) {
      return "/data/from-shell.sqlite";
    }
    if (cmd === EXPORT_STORE_COMMAND) {
      return { schema_version: 2, table_count: 14, row_count: 0, path: null };
    }
    if (cmd === DELETE_ALL_COMMAND || cmd === UPSERT_ARM_COMMAND) {
      return null;
    }
    if (cmd === "core_version") {
      return "9.9.9-from-core";
    }
    if (cmd === "demo_example6_layer") {
      return "4.321-from-core";
    }
    if (cmd === WINDOW_RESULT_COMMAND) {
      return windowResultFixture({
        arms: [],
        official_top: [],
        empty_reason: null,
        diagnostic_anchor_c_text: null,
        anchor_headers: [],
        tier_ii_legend: null,
      });
    }
    throw new Error(cmd);
  });

  render(<App />);

  expect(await screen.findByRole("heading", { name: "前沿" })).toBeTruthy();
  expect(screen.getByRole("heading", { name: "按你的权重" })).toBeTruthy();
  expect(screen.getAllByRole("separator").length).toBeGreaterThanOrEqual(2);
});

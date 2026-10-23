import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { App } from "@/App";
import { CORE_VERSION_COMMAND } from "@/lib/core-version";
import { DEMO_EXAMPLE6_LAYER_COMMAND } from "@/lib/demo-example6";
import {
  DELETE_ALL_COMMAND,
  EXPORT_STORE_COMMAND,
  LIST_ARMS_COMMAND,
  STORE_DATA_PATH_COMMAND,
  UPSERT_ARM_COMMAND,
} from "@/lib/store";
import { SENSITIVITY_COMMAND } from "@/lib/sensitivity";
import { WINDOW_CHROME_COMMAND } from "@/lib/window-chrome";
import { WINDOW_RESULT_COMMAND } from "@/lib/window-result";
import { sensitivityView } from "@/test-sensitivity";
import { windowResultFixture } from "@/test-window-result";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

function storeFallback(cmd: string): unknown {
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
  if (cmd === WINDOW_CHROME_COMMAND) {
    return { window_index: 0, is_first: true, first_window_label: "LABEL-FROM-CORE" };
  }
  if (cmd === SENSITIVITY_COMMAND) {
    return sensitivityView();
  }
  if (cmd === WINDOW_RESULT_COMMAND) {
    return windowResultFixture({
      arms: [],
      official_top: [],
      frontier_off: [],
      frontier_rec: [],
      eligible: [],
      weight_source: null,
      omega_source: null,
      cell_weights_version: null,
      reference_version: null,
      fallback_version: null,
      price_table_version: null,
      classification_rules_version: null,
      authenticity_source: "probes-off",
      authenticity_undetermined: true,
      anchor_headers: [],
      tier_ii_legend: null,
      diagnostic_anchor_c_text: null,
      diagnostic_anchor_t_text: null,
      diagnostic_anchor_q_text: null,
      measured_anchor_c_text: null,
      v_withheld: null,
      empty_reason: null,
    });
  }
  throw new Error(cmd);
}

function stubCommands(core: unknown, demo: unknown) {
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === CORE_VERSION_COMMAND) {
      if (core instanceof Error) {
        throw core;
      }
      return core;
    }
    if (cmd === DEMO_EXAMPLE6_LAYER_COMMAND) {
      if (demo instanceof Error) {
        throw demo;
      }
      return demo;
    }
    return storeFallback(cmd);
  });
}

test("shows the core_version string from invoke, not a frontend constant", async () => {
  stubCommands("9.9.9-from-core", "4.321-from-core");

  render(<App />);

  expect(await screen.findByText("9.9.9-from-core")).toBeTruthy();
  expect(screen.getByText("已连接")).toBeTruthy();
  expect(screen.queryByText("0.0.0")).toBeNull();
  expect(invoke).toHaveBeenCalledWith(CORE_VERSION_COMMAND);
});

test("shows the example-6 anchor from invoke, not a frontend constant", async () => {
  stubCommands("9.9.9-from-core", "4.321-from-core");

  render(<App />);

  expect(await screen.findByText("4.321-from-core")).toBeTruthy();
  expect(screen.getByText("演示夹具，非你的数据")).toBeTruthy();
  expect(screen.queryByText("0.7875")).toBeNull();
  expect(invoke).toHaveBeenCalledWith(DEMO_EXAMPLE6_LAYER_COMMAND);
});

test("shows a disconnected status when the shell commands fail", async () => {
  const failure = new Error("unavailable");
  stubCommands(failure, failure);

  render(<App />);

  expect((await screen.findAllByText("未能读取")).length).toBe(2);
  expect(screen.getByText("未连接")).toBeTruthy();
  expect(screen.queryByText("9.9.9-from-core")).toBeNull();
  expect(screen.queryByText("4.321-from-core")).toBeNull();
  expect(screen.queryByText("0.7875")).toBeNull();
});

test("retries by calling core_version again", async () => {
  let coreCalls = 0;
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === DEMO_EXAMPLE6_LAYER_COMMAND) {
      return "4.321-from-core";
    }
    if (cmd === CORE_VERSION_COMMAND) {
      coreCalls += 1;
      if (coreCalls === 1) {
        throw new Error("unavailable");
      }
      return "0.0.0-from-core";
    }
    return storeFallback(cmd);
  });

  render(<App />);

  fireEvent.click(await screen.findByRole("button", { name: "重试" }));

  expect(await screen.findByText("0.0.0-from-core")).toBeTruthy();
  expect(screen.getByText("已连接")).toBeTruthy();
  expect(screen.getByText("4.321-from-core")).toBeTruthy();
  expect(coreCalls).toBe(2);
  expect(invoke).toHaveBeenCalledWith(CORE_VERSION_COMMAND);
  expect(invoke).toHaveBeenCalledWith(DEMO_EXAMPLE6_LAYER_COMMAND);
});

import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { FrontierPane } from "@/components/frontier-pane";
import { SENSITIVITY_COMMAND } from "@/lib/sensitivity";
import { WINDOW_RESULT_COMMAND, type WindowResult } from "@/lib/window-result";
import { sensitivityView } from "@/test-sensitivity";
import { armView, windowResultFixture } from "@/test-window-result";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

function sourceOf(args: unknown): unknown {
  if (args && typeof args === "object" && "source" in args) {
    return args.source;
  }
  return undefined;
}

function install(windowResultFor: (cmd: string, args: unknown) => unknown) {
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd === SENSITIVITY_COMMAND) {
      return sensitivityView();
    }
    return windowResultFor(cmd, args);
  });
}

function resultFor(source: unknown): WindowResult {
  if (source === "example6") {
    return windowResultFixture({
      source: "example6",
      fixture: true,
      diagnostic_anchor_c_text: "ANCHOR-SIX",
      measured_anchor_c_text: "MEASURED-SIX",
    });
  }
  if (source === "tie") {
    return windowResultFixture({
      source: "tie",
      fixture: true,
      tied: true,
      official_top: ["X", "Y"],
      arms: [
        armView({ id: "X", v_text: "SAME-V" }),
        armView({ id: "Y", v_text: "SAME-V" }),
      ],
    });
  }
  if (source === "same-v") {
    return windowResultFixture({
      tied: false,
      official_top: ["X"],
      arms: [
        armView({ id: "X", v_text: "SAME-V", on_off: true }),
        armView({ id: "Y", v_text: "SAME-V", on_off: false }),
      ],
    });
  }
  return windowResultFixture({
    diagnostic_anchor_c_text: "ANCHOR-STORE",
    weight_source: "SRC-WEIGHTS",
    omega_source: "SRC-OMEGA",
    cell_weights_version: "SRC-CELLS",
    reference_version: "SRC-REF",
    fallback_version: "SRC-FALLBACK",
    price_table_version: "SRC-PRICE",
    classification_rules_version: "SRC-RULES",
    authenticity_grade: "B",
    authenticity_source: "SRC-AUTH",
    authenticity_note: "NOTE-FROM-SHELL",
  });
}

test("shows the shell anchor string and not a hardcoded example value", async () => {
  install(async (cmd: string, args) => {
    if (cmd === WINDOW_RESULT_COMMAND) {
      return resultFor(sourceOf(args));
    }
    throw new Error(cmd);
  });

  render(<FrontierPane />);

  expect(await screen.findByText("ANCHOR-STORE")).toBeTruthy();
  expect(screen.queryByText("0.7875")).toBeNull();
  expect(screen.queryByText("演示夹具，非你的数据")).toBeNull();
  expect(invoke).toHaveBeenCalledWith(WINDOW_RESULT_COMMAND, { source: "store" });
});

test("example 6 shows a different shell string and the fixture label", async () => {
  install(async (cmd: string, args) => {
    if (cmd === WINDOW_RESULT_COMMAND) {
      return resultFor(sourceOf(args));
    }
    throw new Error(cmd);
  });

  render(<FrontierPane />);
  fireEvent.click(await screen.findByRole("button", { name: "例6" }));

  expect(await screen.findByText("ANCHOR-SIX")).toBeTruthy();
  expect(screen.getByText("MEASURED-SIX")).toBeTruthy();
  expect(screen.getByText("演示夹具，非你的数据")).toBeTruthy();
  expect(screen.queryByText("ANCHOR-STORE")).toBeNull();
  expect(screen.queryByText("0.7875")).toBeNull();
  expect(invoke).toHaveBeenCalledWith(WINDOW_RESULT_COMMAND, { source: "example6" });
});

test("a tie lists every official top id", async () => {
  install(async (cmd: string, args) => {
    if (cmd === WINDOW_RESULT_COMMAND) {
      return resultFor(sourceOf(args));
    }
    throw new Error(cmd);
  });

  render(<FrontierPane />);
  fireEvent.click(await screen.findByRole("button", { name: "并列" }));

  expect(await screen.findByText("并列整组")).toBeTruthy();
  expect(screen.getAllByText("X").length).toBeGreaterThan(0);
  expect(screen.getAllByText("Y").length).toBeGreaterThan(0);
});

test("matching V texts do not show a tie group unless the shell says tied", async () => {
  install(async (cmd: string) => {
    if (cmd === WINDOW_RESULT_COMMAND) {
      return resultFor("same-v");
    }
    throw new Error(cmd);
  });

  render(<FrontierPane />);

  expect(await screen.findAllByText("SAME-V")).toHaveLength(2);
  expect(screen.queryByText("并列整组")).toBeNull();
});

test("source fields from the shell are visible", async () => {
  install(async (cmd: string, args) => {
    if (cmd === WINDOW_RESULT_COMMAND) {
      return resultFor(sourceOf(args));
    }
    throw new Error(cmd);
  });

  render(<FrontierPane />);

  expect(await screen.findByText("SRC-WEIGHTS")).toBeTruthy();
  expect(screen.getByText("SRC-OMEGA")).toBeTruthy();
  expect(screen.getByText("SRC-CELLS")).toBeTruthy();
  expect(screen.getByText("SRC-REF")).toBeTruthy();
  expect(screen.getByText("SRC-FALLBACK")).toBeTruthy();
  expect(screen.getByText("SRC-PRICE")).toBeTruthy();
  expect(screen.getByText("SRC-RULES")).toBeTruthy();
  expect(screen.getByText("B · SRC-AUTH · NOTE-FROM-SHELL")).toBeTruthy();
  expect(screen.queryByText("未测定")).toBeNull();
});

test("shows an empty reason from the shell", async () => {
  install(async () =>
    windowResultFixture({
      arms: [],
      official_top: [],
      empty_reason: "EMPTY-REASON-FROM-SHELL",
      diagnostic_anchor_c_text: null,
    }),
  );

  render(<FrontierPane />);

  expect(await screen.findByText("EMPTY-REASON-FROM-SHELL")).toBeTruthy();
});

test("shows robust and recommended membership from the shell", async () => {
  install(async (cmd: string) => {
    if (cmd === WINDOW_RESULT_COMMAND) {
      return windowResultFixture({
        arms: [armView({ id: "own", on_off: true, on_rob: false, on_rec: true })],
      });
    }
    throw new Error(cmd);
  });

  render(<FrontierPane />);

  const row = (await screen.findByText("own")).closest("tr");
  expect(row).toBeTruthy();
  const cells = [...row!.querySelectorAll("td")].map((cell) => cell.textContent);
  expect(cells.slice(5)).toEqual(["是", "—", "是"]);
});

test("retries a failed read", async () => {
  let calls = 0;
  install(async (cmd: string) => {
    if (cmd !== WINDOW_RESULT_COMMAND) {
      throw new Error(cmd);
    }
    calls += 1;
    if (calls === 1) {
      throw new Error("unavailable");
    }
    return windowResultFixture({ diagnostic_anchor_c_text: "AFTER-RETRY" });
  });

  render(<FrontierPane />);
  fireEvent.click(await screen.findByRole("button", { name: "重试" }));

  expect(await screen.findByText("AFTER-RETRY")).toBeTruthy();
  expect(calls).toBe(2);
});

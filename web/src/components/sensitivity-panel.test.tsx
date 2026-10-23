import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { SensitivityPanel } from "@/components/sensitivity-panel";
import { SENSITIVITY_COMMAND } from "@/lib/sensitivity";
import { sensitivityChart, sensitivityView } from "@/test-sensitivity";

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

test("a withheld layer does not show the chart", async () => {
  vi.mocked(invoke).mockResolvedValue(
    sensitivityView({
      show_chart: false,
      withheld_text: "WITHHELD-FROM-CORE",
      chart: sensitivityChart(),
    }),
  );

  render(<SensitivityPanel windowSource="store" />);

  expect(await screen.findByText("WITHHELD-FROM-CORE")).toBeTruthy();
  expect(screen.queryByRole("img", { name: "胜者图" })).toBeNull();
  expect(screen.queryByText("COPY-FROM-CORE")).toBeNull();
  expect(screen.queryByText("SENTENCE-COST")).toBeNull();
});

test("no anchor hides the chart even when a sentence was sent", async () => {
  vi.mocked(invoke).mockResolvedValue(
    sensitivityView({
      show_chart: false,
      withheld: "no-reference",
      withheld_text: "NO-ANCHOR-FROM-CORE",
      chart: sensitivityChart(),
    }),
  );

  render(<SensitivityPanel windowSource="store" />);

  expect(await screen.findByText("NO-ANCHOR-FROM-CORE")).toBeTruthy();
  expect(screen.queryByRole("img", { name: "胜者图" })).toBeNull();
});

test("the chart uses shell winners and shell copy", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd !== SENSITIVITY_COMMAND) {
      throw new Error(cmd);
    }
    if (sourceOf(args) === "grid") {
      return sensitivityView({
        source: "grid",
        fixture: true,
        show_chart: true,
        withheld: null,
        withheld_text: null,
        chart: sensitivityChart(),
      });
    }
    return sensitivityView();
  });

  render(<SensitivityPanel windowSource="store" />);
  fireEvent.click(await screen.findByRole("button", { name: "网格" }));

  expect(await screen.findByRole("img", { name: "胜者图" })).toBeTruthy();
  expect(screen.getByText("Y COPY-FROM-CORE")).toBeTruthy();
  expect(screen.getByText("权重范围 MARK-FROM-CORE")).toBeTruthy();
  expect(screen.getByText("FLOOR-FROM-CORE")).toBeTruthy();
  expect(screen.getByText("Y 14/14")).toBeTruthy();
  expect(screen.getByText("SENTENCE-COST")).toBeTruthy();
  expect(screen.queryByText("SENTENCE-QUALITY")).toBeNull();

  const circles = document.querySelectorAll("circle");
  expect(circles).toHaveLength(2);
  const fills = [...circles].map((circle) => circle.getAttribute("fill"));
  expect(new Set(fills).size).toBe(2);
  expect(circles[0]?.getAttribute("data-winners")).toBe("Y");
  expect(circles[0]?.getAttribute("data-omega-s")).toBe("true");
  expect(circles[1]?.getAttribute("data-winners")).toBe("X");
  expect(circles[1]?.getAttribute("data-omega-b")).toBe("true");

  fireEvent.click(screen.getByRole("button", { name: "质量" }));
  expect(screen.getByText("SENTENCE-QUALITY")).toBeTruthy();
  expect(screen.queryByText("SENTENCE-COST")).toBeNull();
});

test("retries a failed sensitivity read", async () => {
  let calls = 0;
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd !== SENSITIVITY_COMMAND) {
      throw new Error(cmd);
    }
    calls += 1;
    if (calls === 1) {
      throw new Error("unavailable");
    }
    return sensitivityView({ withheld_text: "AFTER-RETRY" });
  });

  render(<SensitivityPanel windowSource="store" />);
  fireEvent.click(await screen.findByRole("button", { name: "重试" }));

  expect(await screen.findByText("AFTER-RETRY")).toBeTruthy();
  expect(screen.queryByRole("img", { name: "胜者图" })).toBeNull();
  expect(calls).toBe(2);
});

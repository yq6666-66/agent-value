import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { ProbePanel } from "@/components/probe-panel";
import {
  PROBE_PANEL_COMMAND,
  SET_PROBE_CONSENT_COMMAND,
  type ProbePanel as ProbeView,
} from "@/lib/probe";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

function panel(overrides: Partial<ProbeView> = {}): ProbeView {
  return {
    source: "store",
    fixture: false,
    decided: false,
    accepted: false,
    consent_prompt: "CONSENT-FROM-SHELL",
    monthly_budget_label: "BUDGET-FROM-SHELL",
    spent_label: "SPENT-FROM-SHELL",
    estimate_label: "ESTIMATE-FROM-SHELL",
    budget_decision: "allow",
    over_budget_prompt: null,
    observation: "pending",
    observation_label: "OBS-FROM-SHELL",
    grade_run_days_label: "DAYS-FROM-SHELL",
    launch: "blocked-consent",
    launch_label: "LAUNCH-FROM-SHELL",
    sends_http: false,
    arms: [],
    empty_reason: "EMPTY-FROM-SHELL",
    ...overrides,
  };
}

function sourceOf(args: unknown): unknown {
  if (args && typeof args === "object" && "source" in args) {
    return args.source;
  }
  return undefined;
}

test("asks for consent from the shell and does not launch a probe", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd === PROBE_PANEL_COMMAND && sourceOf(args) === "store") {
      return panel();
    }
    throw new Error(cmd);
  });

  render(<ProbePanel />);

  expect(await screen.findByText("CONSENT-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText("BUDGET-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText("OBS-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText("DAYS-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText("LAUNCH-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText("EMPTY-FROM-SHELL")).toBeTruthy();
  expect(screen.getByRole("button", { name: "同意" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "暂不" })).toBeTruthy();
  expect(screen.queryByText("仅供参考")).toBeNull();
  expect(screen.queryByText("10 元")).toBeNull();
  expect(screen.queryByText("2 个运行日")).toBeNull();
  expect(invoke).toHaveBeenCalledWith(PROBE_PANEL_COMMAND, { source: "store" });
  expect(invoke).not.toHaveBeenCalledWith(SET_PROBE_CONSENT_COMMAND, expect.anything());
});

test("consent is stored and no run command is called", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd === PROBE_PANEL_COMMAND) {
      return panel();
    }
    if (cmd === SET_PROBE_CONSENT_COMMAND) {
      return panel({
        decided: true,
        accepted: true,
        launch: "not-sent",
        launch_label: "SAVED-FROM-SHELL",
      });
    }
    throw new Error(String(cmd) + JSON.stringify(args));
  });

  render(<ProbePanel />);
  fireEvent.click(await screen.findByRole("button", { name: "同意" }));

  expect(await screen.findByText("SAVED-FROM-SHELL")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "同意" })).toBeNull();
  expect(invoke).toHaveBeenCalledWith(SET_PROBE_CONSENT_COMMAND, {
    accepted: true,
    source: "store",
  });
  const commands = vi.mocked(invoke).mock.calls.map((call) => call[0]);
  expect(commands.every((cmd) => cmd === PROBE_PANEL_COMMAND || cmd === SET_PROBE_CONSENT_COMMAND)).toBe(
    true,
  );
});

test("declining stores accepted false", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === PROBE_PANEL_COMMAND) {
      return panel();
    }
    if (cmd === SET_PROBE_CONSENT_COMMAND) {
      return panel({ decided: true, accepted: false, launch_label: "DECLINED-FROM-SHELL" });
    }
    throw new Error(cmd);
  });

  render(<ProbePanel />);
  fireEvent.click(await screen.findByRole("button", { name: "暂不" }));

  expect(await screen.findByText("DECLINED-FROM-SHELL")).toBeTruthy();
  expect(invoke).toHaveBeenCalledWith(SET_PROBE_CONSENT_COMMAND, {
    accepted: false,
    source: "store",
  });
});

test("shows the shell over-budget prompt and a shell grade note", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd === PROBE_PANEL_COMMAND && sourceOf(args) === "store") {
      return panel({
        decided: true,
        accepted: true,
        over_budget_prompt: "STOP-FROM-SHELL",
        budget_decision: "stop",
        empty_reason: null,
        arms: [
          {
            id: "arm-from-shell",
            grade: "GRADE-FROM-SHELL",
            reference_only: true,
            reference_label: "REF-ONLY-FROM-SHELL",
            labels: ["LABEL-FROM-SHELL"],
            classes: [
              {
                class: "deterministic",
                verdict: "VERDICT-FROM-SHELL",
                run_day: "DAY-FROM-SHELL",
                evidence: "EVIDENCE-FROM-SHELL",
              },
            ],
          },
        ],
      });
    }
    if (cmd === PROBE_PANEL_COMMAND && sourceOf(args) === "fixture") {
      return panel({
        source: "fixture",
        fixture: true,
        decided: true,
        arms: [],
        empty_reason: null,
        grade_run_days_label: "FIXTURE-DAYS",
      });
    }
    throw new Error(cmd);
  });

  render(<ProbePanel />);

  expect(await screen.findByText("STOP-FROM-SHELL")).toBeTruthy();
  expect(screen.getByRole("status").textContent).toBe("STOP-FROM-SHELL");
  expect(screen.getByText("GRADE-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText("REF-ONLY-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText("LABEL-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText("EVIDENCE-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText(/VERDICT-FROM-SHELL/)).toBeTruthy();
  expect(screen.queryByText("仅供参考")).toBeNull();
  expect(screen.queryByText("未测定")).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: "夹具回放" }));

  expect(await screen.findByText("演示夹具，非你的数据")).toBeTruthy();
  expect(screen.getByText("FIXTURE-DAYS")).toBeTruthy();
  expect(screen.queryByText("GRADE-FROM-SHELL")).toBeNull();
  expect(screen.queryByText("STOP-FROM-SHELL")).toBeNull();
});

test("a grade without a reference label does not invent 仅供参考", async () => {
  vi.mocked(invoke).mockResolvedValue(
    panel({
      decided: true,
      empty_reason: null,
      arms: [
        {
          id: "official",
          grade: null,
          reference_only: false,
          reference_label: null,
          labels: ["官方直连"],
          classes: [],
        },
      ],
    }),
  );

  render(<ProbePanel />);

  expect(await screen.findByText("官方直连")).toBeTruthy();
  expect(screen.queryByText("仅供参考")).toBeNull();
});

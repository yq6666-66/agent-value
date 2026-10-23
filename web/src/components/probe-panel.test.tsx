import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { ProbePanel } from "@/components/probe-panel";
import {
  PROBE_PANEL_COMMAND,
  PROBE_RUN_FAILED,
  PROBE_RUN_IN_PROGRESS,
  RUN_PROBE_ONCE_COMMAND,
  SET_PROBE_CONSENT_COMMAND,
  SET_PROBE_SCHEDULE_COMMAND,
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
    schedule_active: false,
    next_planned_label: null,
    schedule_status_label: null,
    schedule_hint: "HINT-FROM-SHELL",
    arms: [],
    run_targets: [],
    empty_reason: "EMPTY-FROM-SHELL",
    ...overrides,
  };
}

function readyPanel(overrides: Partial<ProbeView> = {}): ProbeView {
  return panel({
    decided: true,
    accepted: true,
    launch: "ready",
    sends_http: true,
    run_targets: [{ id: "arm-from-shell" }],
    empty_reason: null,
    ...overrides,
  });
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
  expect(screen.queryByText("已校准")).toBeNull();
  expect(screen.queryByText("10 元")).toBeNull();
  expect(screen.queryByText("2 个运行日")).toBeNull();
  expect(screen.queryByRole("button", { name: "跑一次" })).toBeNull();
  expect(screen.queryByRole("button", { name: "启用定时" })).toBeNull();
  expect(invoke).toHaveBeenCalledWith(PROBE_PANEL_COMMAND, { source: "store" });
  expect(invoke).not.toHaveBeenCalledWith(SET_PROBE_CONSENT_COMMAND, expect.anything());
  expect(invoke).not.toHaveBeenCalledWith(RUN_PROBE_ONCE_COMMAND, expect.anything());
  expect(invoke).not.toHaveBeenCalledWith(SET_PROBE_SCHEDULE_COMMAND, expect.anything());
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
  expect(screen.queryByRole("button", { name: "跑一次" })).toBeNull();
});

test.each([
  ["blocked-budget", { launch: "blocked-budget" }],
  ["blocked-already-run", { launch: "blocked-already-run" }],
  ["running", { launch: "running" }],
  ["completed", { launch: "completed" }],
  ["failed", { launch: "failed" }],
  ["ready before consent", { decided: false, accepted: false, launch: "ready" }],
  ["ready when the build cannot send", { launch: "ready", sends_http: false }],
] as const)("hides 跑一次 for %s", async (_label, overrides) => {
  vi.mocked(invoke).mockResolvedValue(readyPanel(overrides));

  render(<ProbePanel />);

  expect(await screen.findByText("LAUNCH-FROM-SHELL")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "跑一次" })).toBeNull();
  expect(invoke).not.toHaveBeenCalledWith(RUN_PROBE_ONCE_COMMAND, expect.anything());
});

test("hides 跑一次 on the fixture source", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd === PROBE_PANEL_COMMAND && sourceOf(args) === "fixture") {
      return readyPanel({
        source: "fixture",
        fixture: true,
        launch: "ready",
        launch_label: "FIXTURE-LAUNCH",
      });
    }
    if (cmd === PROBE_PANEL_COMMAND) {
      return readyPanel();
    }
    throw new Error(String(cmd));
  });

  render(<ProbePanel />);
  expect(await screen.findByRole("button", { name: "跑一次" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "夹具回放" }));

  expect(await screen.findByText("FIXTURE-LAUNCH")).toBeTruthy();
  expect(screen.getByText("演示夹具，非你的数据")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "跑一次" })).toBeNull();
  expect(invoke).not.toHaveBeenCalledWith(RUN_PROBE_ONCE_COMMAND, expect.anything());
});

test("shows 跑一次 when ready and does not run on open", async () => {
  vi.mocked(invoke).mockResolvedValue(readyPanel());

  render(<ProbePanel />);

  expect(await screen.findByRole("button", { name: "跑一次" })).toBeTruthy();
  expect(screen.getByLabelText("密钥变量名")).toBeTruthy();
  expect(screen.getByText("只填变量名，不填密钥。")).toBeTruthy();
  expect(screen.getByText("OBS-FROM-SHELL")).toBeTruthy();
  expect(screen.queryByText("已校准")).toBeNull();
  expect(screen.queryByText("仅供参考")).toBeNull();
  expect(invoke).not.toHaveBeenCalledWith(RUN_PROBE_ONCE_COMMAND, expect.anything());
});

test("跑一次 invokes the run command once and refreshes the store panel", async () => {
  let resolveRun: (value: { launch: string; detail: string }) => void = () => {};
  const pending = new Promise<{ launch: string; detail: string }>((resolve) => {
    resolveRun = resolve;
  });
  let panelCalls = 0;
  vi.mocked(invoke).mockImplementation(async (cmd: string, args) => {
    if (cmd === PROBE_PANEL_COMMAND && sourceOf(args) === "store") {
      panelCalls += 1;
      if (panelCalls === 1) {
        return readyPanel();
      }
      return readyPanel({
        launch: "completed",
        launch_label: "WROTE-FROM-SHELL",
        arms: [
          {
            id: "arm-from-shell",
            grade: "GRADE-FROM-SHELL",
            reference_only: true,
            reference_label: "REF-ONLY-FROM-SHELL",
            labels: [],
            classes: [],
          },
        ],
      });
    }
    if (cmd === RUN_PROBE_ONCE_COMMAND) {
      return pending;
    }
    throw new Error(String(cmd));
  });

  render(<ProbePanel />);
  fireEvent.change(await screen.findByLabelText("密钥变量名"), {
    target: { value: "RELAY_API_KEY" },
  });
  fireEvent.change(screen.getByLabelText("基址"), {
    target: { value: "https://relay.example" },
  });
  const button = screen.getByRole("button", { name: "跑一次" });
  fireEvent.click(button);
  fireEvent.click(button);

  expect(await screen.findByText(PROBE_RUN_IN_PROGRESS)).toBeTruthy();
  expect(screen.queryByRole("button", { name: "跑一次" })).toBeNull();
  expect(screen.queryByText("已校准")).toBeNull();
  const storeToggle = screen.getByRole("button", { name: "本机记录" });
  const fixtureToggle = screen.getByRole("button", { name: "夹具回放" });
  expect(storeToggle).toHaveProperty("disabled", true);
  expect(fixtureToggle).toHaveProperty("disabled", true);
  fireEvent.click(fixtureToggle);
  expect(screen.getByText("OBS-FROM-SHELL")).toBeTruthy();
  expect(screen.queryByText("演示夹具，非你的数据")).toBeNull();
  const runCalls = vi.mocked(invoke).mock.calls.filter((call) => call[0] === RUN_PROBE_ONCE_COMMAND);
  expect(runCalls).toEqual([
    [
      RUN_PROBE_ONCE_COMMAND,
      {
        input: {
          arm_id: "arm-from-shell",
          key_env: "RELAY_API_KEY",
          base_url: "https://relay.example",
        },
      },
    ],
  ]);

  resolveRun({ launch: "completed", detail: "DONE-FROM-SHELL" });

  expect(await screen.findByText("WROTE-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText("GRADE-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText("REF-ONLY-FROM-SHELL")).toBeTruthy();
  expect(screen.queryByText("DONE-FROM-SHELL")).toBeNull();
  expect(screen.queryByText(PROBE_RUN_IN_PROGRESS)).toBeNull();
  expect(screen.queryByRole("button", { name: "跑一次" })).toBeNull();
  expect(screen.queryByText("已校准")).toBeNull();
  expect(panelCalls).toBe(2);
  expect(screen.getByRole("button", { name: "本机记录" })).toHaveProperty("disabled", false);
  expect(screen.getByRole("button", { name: "夹具回放" })).toHaveProperty("disabled", false);
  expect(vi.mocked(invoke).mock.calls.filter((call) => call[0] === RUN_PROBE_ONCE_COMMAND)).toHaveLength(1);
});

test("a failed run shows the shell detail", async () => {
  let panelCalls = 0;
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === PROBE_PANEL_COMMAND) {
      panelCalls += 1;
      if (panelCalls === 1) {
        return readyPanel();
      }
      return readyPanel({ launch: "failed", launch_label: "FAIL-LAUNCH-FROM-SHELL" });
    }
    if (cmd === RUN_PROBE_ONCE_COMMAND) {
      return { launch: "failed", detail: "FAIL-DETAIL-FROM-SHELL" };
    }
    throw new Error(cmd);
  });

  render(<ProbePanel />);
  fireEvent.change(await screen.findByLabelText("密钥变量名"), {
    target: { value: "RELAY_API_KEY" },
  });
  fireEvent.change(screen.getByLabelText("基址"), {
    target: { value: "https://relay.example" },
  });
  fireEvent.click(screen.getByRole("button", { name: "跑一次" }));

  expect(await screen.findByText("FAIL-DETAIL-FROM-SHELL")).toBeTruthy();
  expect(await screen.findByText("FAIL-LAUNCH-FROM-SHELL")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "跑一次" })).toBeNull();
  expect(screen.queryByText("已校准")).toBeNull();
  expect(panelCalls).toBe(2);
  expect(vi.mocked(invoke).mock.calls.filter((call) => call[0] === RUN_PROBE_ONCE_COMMAND)).toHaveLength(1);
});

test("a rejected run does not show the rejection text", async () => {
  const secret = "sk-test-Ab1cdEf2Gh3ijKl4Mn5";
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === PROBE_PANEL_COMMAND) {
      return readyPanel();
    }
    if (cmd === RUN_PROBE_ONCE_COMMAND) {
      throw new Error(secret);
    }
    throw new Error(cmd);
  });

  render(<ProbePanel />);
  fireEvent.change(await screen.findByLabelText("密钥变量名"), {
    target: { value: "RELAY_API_KEY" },
  });
  fireEvent.change(screen.getByLabelText("基址"), {
    target: { value: "https://relay.example" },
  });
  fireEvent.click(screen.getByRole("button", { name: "跑一次" }));

  expect(await screen.findByText(PROBE_RUN_FAILED)).toBeTruthy();
  expect(document.body.textContent).not.toContain(secret);
  expect(vi.mocked(invoke).mock.calls.filter((call) => call[0] === RUN_PROBE_ONCE_COMMAND)).toHaveLength(1);
});

test("shows a read-only next plan and does not run when enabling the schedule", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === PROBE_PANEL_COMMAND) {
      return readyPanel({ schedule_hint: "HINT-FROM-SHELL" });
    }
    if (cmd === SET_PROBE_SCHEDULE_COMMAND) {
      return readyPanel({
        schedule_active: true,
        next_planned_label: "NEXT-FROM-SHELL",
        schedule_hint: "HINT-FROM-SHELL",
        launch: "ready",
      });
    }
    throw new Error(cmd);
  });

  render(<ProbePanel />);

  expect(await screen.findByText("HINT-FROM-SHELL")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "启用定时" })).toBeTruthy();
  expect(screen.queryByText("NEXT-FROM-SHELL")).toBeNull();
  fireEvent.change(screen.getByLabelText("密钥变量名"), {
    target: { value: "RELAY_API_KEY" },
  });
  fireEvent.change(screen.getByLabelText("基址"), {
    target: { value: "https://relay.example" },
  });
  fireEvent.click(screen.getByRole("button", { name: "启用定时" }));

  expect(await screen.findByText("NEXT-FROM-SHELL")).toBeTruthy();
  expect(screen.queryByRole("textbox", { name: "下次计划" })).toBeNull();
  expect(screen.getByRole("button", { name: "停止定时" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "启用定时" })).toBeNull();
  expect(invoke).toHaveBeenCalledWith(SET_PROBE_SCHEDULE_COMMAND, {
    input: {
      enabled: true,
      arm_id: "arm-from-shell",
      key_env: "RELAY_API_KEY",
      base_url: "https://relay.example",
    },
  });
  expect(invoke).not.toHaveBeenCalledWith(RUN_PROBE_ONCE_COMMAND, expect.anything());
});

test("stopping the schedule does not run a probe", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === PROBE_PANEL_COMMAND) {
      return readyPanel({
        schedule_active: true,
        next_planned_label: "NEXT-FROM-SHELL",
      });
    }
    if (cmd === SET_PROBE_SCHEDULE_COMMAND) {
      return readyPanel({ schedule_active: false, next_planned_label: null });
    }
    throw new Error(cmd);
  });

  render(<ProbePanel />);
  fireEvent.click(await screen.findByRole("button", { name: "停止定时" }));

  expect(await screen.findByRole("button", { name: "启用定时" })).toBeTruthy();
  expect(screen.queryByText("NEXT-FROM-SHELL")).toBeNull();
  expect(invoke).toHaveBeenCalledWith(SET_PROBE_SCHEDULE_COMMAND, {
    input: {
      enabled: false,
      arm_id: "",
      key_env: "",
      base_url: "",
    },
  });
  expect(invoke).not.toHaveBeenCalledWith(RUN_PROBE_ONCE_COMMAND, expect.anything());
});

test("a budget stop hides the schedule enable and shows the shell sentence", async () => {
  vi.mocked(invoke).mockResolvedValue(
    readyPanel({
      launch: "blocked-budget",
      budget_decision: "stop",
      schedule_active: false,
      next_planned_label: null,
      schedule_status_label: "SCHEDULE-STOP-FROM-SHELL",
      over_budget_prompt: "STOP-FROM-SHELL",
    }),
  );

  render(<ProbePanel />);

  expect(await screen.findByText("SCHEDULE-STOP-FROM-SHELL")).toBeTruthy();
  expect(screen.getByText("STOP-FROM-SHELL")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "启用定时" })).toBeNull();
  expect(screen.queryByRole("button", { name: "跑一次" })).toBeNull();
  expect(screen.queryByText("已校准")).toBeNull();
  expect(invoke).not.toHaveBeenCalledWith(SET_PROBE_SCHEDULE_COMMAND, expect.anything());
  expect(invoke).not.toHaveBeenCalledWith(RUN_PROBE_ONCE_COMMAND, expect.anything());
});

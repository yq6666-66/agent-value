import { invoke } from "@tauri-apps/api/core";

// Consent, budget, observation copy, grades, and verdicts come from the
// shell. This module checks the shape. It does not decide a grade or a
// budget.

export const PROBE_PANEL_COMMAND = "probe_panel";
export const SET_PROBE_CONSENT_COMMAND = "set_probe_consent";
export const RUN_PROBE_ONCE_COMMAND = "run_probe_once";

/// Same sentence as the shell `ProbeLaunch::Running` label.
export const PROBE_RUN_IN_PROGRESS = "正在探测。";

/// Shown when the command rejects. The rejection text is not rendered:
/// it might repeat something the user typed.
export const PROBE_RUN_FAILED = "这次探测没有完成。";

export const PROBE_SOURCES = ["store", "fixture"] as const;

export type ProbeSource = (typeof PROBE_SOURCES)[number];

export type ProbeClassView = {
  class: string;
  verdict: string;
  run_day: string;
  evidence: string;
};

export type ProbeRunTarget = {
  id: string;
};

export type ProbeRunOnceInput = {
  arm_id: string;
  key_env: string;
  base_url: string;
};

export type ProbeRunOnce = {
  launch: string;
  detail: string;
};

export type ProbeArmView = {
  id: string;
  grade: string | null;
  reference_only: boolean;
  reference_label: string | null;
  labels: string[];
  classes: ProbeClassView[];
};

export type ProbePanel = {
  source: string;
  fixture: boolean;
  decided: boolean;
  accepted: boolean;
  consent_prompt: string;
  monthly_budget_label: string;
  spent_label: string;
  estimate_label: string;
  budget_decision: string;
  over_budget_prompt: string | null;
  observation: string;
  observation_label: string;
  grade_run_days_label: string;
  launch: string;
  launch_label: string;
  sends_http: boolean;
  arms: ProbeArmView[];
  run_targets: ProbeRunTarget[];
  empty_reason: string | null;
};

function objectOf(value: unknown, what: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${what} returned a non-object`);
  }
  return value as Record<string, unknown>;
}

function text(value: unknown, what: string): string {
  if (typeof value !== "string") {
    throw new Error(`${what} returned an incomplete value`);
  }
  return value;
}

function optionalText(value: unknown, what: string): string | null {
  if (value === null) {
    return null;
  }
  return text(value, what);
}

function flag(value: unknown, what: string): boolean {
  if (typeof value !== "boolean") {
    throw new Error(`${what} returned an incomplete value`);
  }
  return value;
}

function stringList(value: unknown, what: string): string[] {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) {
    throw new Error(`${what} returned an incomplete value`);
  }
  return value as string[];
}

function classView(value: unknown): ProbeClassView {
  const row = objectOf(value, PROBE_PANEL_COMMAND);
  return {
    class: text(row.class, PROBE_PANEL_COMMAND),
    verdict: text(row.verdict, PROBE_PANEL_COMMAND),
    run_day: text(row.run_day, PROBE_PANEL_COMMAND),
    evidence: text(row.evidence, PROBE_PANEL_COMMAND),
  };
}

function runTarget(value: unknown): ProbeRunTarget {
  const row = objectOf(value, PROBE_PANEL_COMMAND);
  return { id: text(row.id, PROBE_PANEL_COMMAND) };
}

function armView(value: unknown): ProbeArmView {
  const row = objectOf(value, PROBE_PANEL_COMMAND);
  if (!Array.isArray(row.classes) || !Array.isArray(row.labels)) {
    throw new Error(`${PROBE_PANEL_COMMAND} returned an incomplete value`);
  }
  return {
    id: text(row.id, PROBE_PANEL_COMMAND),
    grade: optionalText(row.grade, PROBE_PANEL_COMMAND),
    reference_only: flag(row.reference_only, PROBE_PANEL_COMMAND),
    reference_label: optionalText(row.reference_label, PROBE_PANEL_COMMAND),
    labels: stringList(row.labels, PROBE_PANEL_COMMAND),
    classes: row.classes.map(classView),
  };
}

export function probePanel(value: unknown): ProbePanel {
  const row = objectOf(value, PROBE_PANEL_COMMAND);
  const arms = row.arms;
  const runTargets = row.run_targets;
  if (!Array.isArray(arms) || !Array.isArray(runTargets)) {
    throw new Error(`${PROBE_PANEL_COMMAND} returned an incomplete value`);
  }
  return {
    source: text(row.source, PROBE_PANEL_COMMAND),
    fixture: flag(row.fixture, PROBE_PANEL_COMMAND),
    decided: flag(row.decided, PROBE_PANEL_COMMAND),
    accepted: flag(row.accepted, PROBE_PANEL_COMMAND),
    consent_prompt: text(row.consent_prompt, PROBE_PANEL_COMMAND),
    monthly_budget_label: text(row.monthly_budget_label, PROBE_PANEL_COMMAND),
    spent_label: text(row.spent_label, PROBE_PANEL_COMMAND),
    estimate_label: text(row.estimate_label, PROBE_PANEL_COMMAND),
    budget_decision: text(row.budget_decision, PROBE_PANEL_COMMAND),
    over_budget_prompt: optionalText(row.over_budget_prompt, PROBE_PANEL_COMMAND),
    observation: text(row.observation, PROBE_PANEL_COMMAND),
    observation_label: text(row.observation_label, PROBE_PANEL_COMMAND),
    grade_run_days_label: text(row.grade_run_days_label, PROBE_PANEL_COMMAND),
    launch: text(row.launch, PROBE_PANEL_COMMAND),
    launch_label: text(row.launch_label, PROBE_PANEL_COMMAND),
    sends_http: flag(row.sends_http, PROBE_PANEL_COMMAND),
    arms: arms.map(armView),
    run_targets: runTargets.map(runTarget),
    empty_reason: optionalText(row.empty_reason, PROBE_PANEL_COMMAND),
  };
}

export async function fetchProbePanel(source: ProbeSource): Promise<ProbePanel> {
  const value: unknown = await invoke(PROBE_PANEL_COMMAND, { source });
  return probePanel(value);
}

export async function setProbeConsent(accepted: boolean, source: ProbeSource): Promise<ProbePanel> {
  const value: unknown = await invoke(SET_PROBE_CONSENT_COMMAND, { accepted, source });
  return probePanel(value);
}

/// One manual run. Does not send a token. `key_env` is the variable name.
/// Seed and claims stay in the shell (H3 typical path).
export async function runProbeOnce(input: ProbeRunOnceInput): Promise<ProbeRunOnce> {
  const value: unknown = await invoke(RUN_PROBE_ONCE_COMMAND, { input });
  const row = objectOf(value, RUN_PROBE_ONCE_COMMAND);
  return {
    launch: text(row.launch, RUN_PROBE_ONCE_COMMAND),
    detail: text(row.detail, RUN_PROBE_ONCE_COMMAND),
  };
}

/// The run control is offered only after consent, when this build can send
/// and launch is `ready`. Fixture replay is not a run. Blocked, running,
/// completed, and failed states hide it. The page does not call the run
/// command while loading.
export function probeRunOffered(view: ProbePanel): boolean {
  return view.source === "store" && view.accepted && view.sends_http && view.launch === "ready";
}

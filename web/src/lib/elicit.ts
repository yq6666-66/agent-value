import { invoke } from "@tauri-apps/api/core";

// Payloads are parsed and shown. This module does not compute weights,
// regions, or the next question.

export const ELICIT_START_COMMAND = "elicit_start";
export const ELICIT_NEXT_COMMAND = "elicit_next";
export const ELICIT_ANSWER_COMMAND = "elicit_answer";
export const ELICIT_SKIP_COMMAND = "elicit_skip";
export const ELICIT_REGIONS_COMMAND = "elicit_regions";
export const ELICIT_FINISH_COMMAND = "elicit_finish";
export const ELICIT_SAVE_COMMAND = "elicit_save";
export const WEIGHT_CONTEXT_COMMAND = "weight_context";
export const BEGIN_WINDOW_COMMAND = "begin_window";
export const EDIT_WEIGHTS_COMMAND = "edit_weights";

export type WeightTriple = {
  a: number;
  b: number;
  g: number;
};

export type ShownPair = {
  cost: [number, number];
  time: [number, number];
  defect: [number, number];
};

export type QuestionView = {
  index: number;
  rho: [number, number, number];
  shown: ShownPair;
};

export type StepView = {
  finished: boolean;
  answered: number;
  skipped: number[];
  wide_len: number;
  core_len: number;
  m: number;
  question: QuestionView | null;
};

export type RegionsView = {
  core: WeightTriple[];
  wide: WeightTriple[];
  m: number;
  point: WeightTriple | null;
};

export type ReportView = {
  regions: RegionsView;
  contradiction: boolean;
  contradicted: number[];
  ranges: [[number, number], [number, number], [number, number]];
};

export type VersionView = {
  id: number;
  a: number;
  b: number;
  g: number;
  source: string;
  trial: boolean;
  effective_from: number | null;
};

export type TrialView = {
  label: string;
  trial: VersionView;
  official: VersionView;
};

export type SaveView = {
  outcome: "trial" | "official";
  label: string | null;
  applied: VersionView;
  official: VersionView;
  point: WeightTriple;
};

export type WeightContext = {
  scope: string;
  in_window: boolean;
  window_index: number | null;
  official: VersionView;
  trial: TrialView | null;
};

export type BaseInput = {
  cost: number;
  time: number;
  defect: number;
};

export type AnswerInput = {
  question: number;
  level: number;
};

export type WeightInput = {
  a: number;
  b: number;
  g: number;
};

function objectOf(value: unknown, what: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${what} returned a non-object`);
  }
  return value as Record<string, unknown>;
}

function finiteNumber(value: unknown, what: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`${what} returned an incomplete value`);
  }
  return value;
}

function integer(value: unknown, what: string): number {
  const number = finiteNumber(value, what);
  if (!Number.isInteger(number)) {
    throw new Error(`${what} returned an incomplete value`);
  }
  return number;
}

function text(value: unknown, what: string): string {
  if (typeof value !== "string") {
    throw new Error(`${what} returned an incomplete value`);
  }
  return value;
}

function flag(value: unknown, what: string): boolean {
  if (typeof value !== "boolean") {
    throw new Error(`${what} returned an incomplete value`);
  }
  return value;
}

function pair(value: unknown, what: string): [number, number] {
  if (!Array.isArray(value) || value.length !== 2) {
    throw new Error(`${what} returned an incomplete value`);
  }
  return [finiteNumber(value[0], what), finiteNumber(value[1], what)];
}

function tripleNumbers(value: unknown, what: string): [number, number, number] {
  if (!Array.isArray(value) || value.length !== 3) {
    throw new Error(`${what} returned an incomplete value`);
  }
  return [
    finiteNumber(value[0], what),
    finiteNumber(value[1], what),
    finiteNumber(value[2], what),
  ];
}

function integerList(value: unknown, what: string): number[] {
  if (!Array.isArray(value)) {
    throw new Error(`${what} returned an incomplete value`);
  }
  return value.map((item) => integer(item, what));
}

export function weightTriple(value: unknown, what: string): WeightTriple {
  const row = objectOf(value, what);
  return {
    a: finiteNumber(row.a, what),
    b: finiteNumber(row.b, what),
    g: finiteNumber(row.g, what),
  };
}

function points(value: unknown, what: string): WeightTriple[] {
  if (!Array.isArray(value)) {
    throw new Error(`${what} returned an incomplete value`);
  }
  return value.map((item) => weightTriple(item, what));
}

function versionView(value: unknown, what: string): VersionView {
  const row = objectOf(value, what);
  const effective = row.effective_from;
  if (!(effective === null || (typeof effective === "number" && Number.isInteger(effective)))) {
    throw new Error(`${what} returned an incomplete value`);
  }
  return {
    id: integer(row.id, what),
    a: finiteNumber(row.a, what),
    b: finiteNumber(row.b, what),
    g: finiteNumber(row.g, what),
    source: text(row.source, what),
    trial: flag(row.trial, what),
    effective_from: effective,
  };
}

export function formatMetric(value: number): string {
  if (!Number.isFinite(value)) {
    throw new Error("metric is not finite");
  }
  return value.toFixed(6).replace(/\.?0+$/, "");
}

export function sourceLabel(source: string): string {
  switch (source) {
    case "default":
      return "默认";
    case "self-report":
      return "自报";
    case "confirmed-behavior-suggestion":
      return "行为建议已确认";
    case "manual":
      return "手动";
    default:
      return source;
  }
}

export function formatWeights(weights: WeightTriple): string {
  return `成本 ${formatMetric(weights.a)} · 时间 ${formatMetric(weights.b)} · 质量 ${formatMetric(weights.g)}`;
}

function questionView(value: unknown): QuestionView {
  const row = objectOf(value, "elicit question");
  const shown = objectOf(row.shown, "elicit question");
  const rho = tripleNumbers(row.rho, "elicit question");
  return {
    index: integer(row.index, "elicit question"),
    rho,
    shown: {
      cost: pair(shown.cost, "elicit question"),
      time: pair(shown.time, "elicit question"),
      defect: pair(shown.defect, "elicit question"),
    },
  };
}

export function stepView(value: unknown): StepView {
  const row = objectOf(value, "elicit step");
  return {
    finished: flag(row.finished, "elicit step"),
    answered: integer(row.answered, "elicit step"),
    skipped: integerList(row.skipped, "elicit step"),
    wide_len: integer(row.wide_len, "elicit step"),
    core_len: integer(row.core_len, "elicit step"),
    m: integer(row.m, "elicit step"),
    question: row.question === null ? null : questionView(row.question),
  };
}

export function regionsView(value: unknown): RegionsView {
  const row = objectOf(value, "elicit_regions");
  return {
    core: points(row.core, "elicit_regions"),
    wide: points(row.wide, "elicit_regions"),
    m: integer(row.m, "elicit_regions"),
    point: row.point === null ? null : weightTriple(row.point, "elicit_regions"),
  };
}

export function reportView(value: unknown): ReportView {
  const row = objectOf(value, "elicit_finish");
  const ranges = row.ranges;
  if (!Array.isArray(ranges) || ranges.length !== 3) {
    throw new Error("elicit_finish returned an incomplete value");
  }
  return {
    regions: regionsView(row.regions),
    contradiction: flag(row.contradiction, "elicit_finish"),
    contradicted: integerList(row.contradicted, "elicit_finish"),
    ranges: [
      pair(ranges[0], "elicit_finish"),
      pair(ranges[1], "elicit_finish"),
      pair(ranges[2], "elicit_finish"),
    ],
  };
}

export function trialView(value: unknown): TrialView {
  const row = objectOf(value, "trial");
  const label = text(row.label, "trial");
  if (label.length === 0) {
    throw new Error("trial returned an incomplete value");
  }
  return {
    label,
    trial: versionView(row.trial, "trial"),
    official: versionView(row.official, "trial"),
  };
}

export function saveView(value: unknown): SaveView {
  const row = objectOf(value, "elicit_save");
  const outcome = text(row.outcome, "elicit_save");
  if (outcome !== "trial" && outcome !== "official") {
    throw new Error("elicit_save returned an incomplete value");
  }
  const label = row.label;
  if (outcome === "trial") {
    if (typeof label !== "string" || label.length === 0) {
      throw new Error("elicit_save returned an incomplete value");
    }
  } else if (label !== null) {
    throw new Error("elicit_save returned an incomplete value");
  }
  return {
    outcome,
    label,
    applied: versionView(row.applied, "elicit_save"),
    official: versionView(row.official, "elicit_save"),
    point: weightTriple(row.point, "elicit_save"),
  };
}

export function weightContext(value: unknown): WeightContext {
  const row = objectOf(value, "weight_context");
  const windowIndex = row.window_index;
  if (!(windowIndex === null || (typeof windowIndex === "number" && Number.isInteger(windowIndex)))) {
    throw new Error("weight_context returned an incomplete value");
  }
  return {
    scope: text(row.scope, "weight_context"),
    in_window: flag(row.in_window, "weight_context"),
    window_index: windowIndex,
    official: versionView(row.official, "weight_context"),
    trial: row.trial === null ? null : trialView(row.trial),
  };
}

export async function startSession(base: BaseInput | null): Promise<StepView> {
  return stepView(await invoke(ELICIT_START_COMMAND, { base }));
}

export async function fetchNext(): Promise<StepView> {
  return stepView(await invoke(ELICIT_NEXT_COMMAND));
}

export async function answerQuestion(answer: AnswerInput): Promise<StepView> {
  return stepView(await invoke(ELICIT_ANSWER_COMMAND, { answer }));
}

export async function skipQuestion(question: number): Promise<StepView> {
  return stepView(await invoke(ELICIT_SKIP_COMMAND, { question }));
}

export async function fetchRegions(): Promise<RegionsView> {
  return regionsView(await invoke(ELICIT_REGIONS_COMMAND));
}

export async function fetchReport(): Promise<ReportView> {
  return reportView(await invoke(ELICIT_FINISH_COMMAND));
}

export async function saveReport(): Promise<SaveView> {
  return saveView(await invoke(ELICIT_SAVE_COMMAND));
}

export async function fetchWeightContext(): Promise<WeightContext> {
  return weightContext(await invoke(WEIGHT_CONTEXT_COMMAND));
}

export async function beginWindow(): Promise<WeightContext> {
  return weightContext(await invoke(BEGIN_WINDOW_COMMAND));
}

export async function editWeights(weights: WeightInput): Promise<TrialView> {
  return trialView(await invoke(EDIT_WEIGHTS_COMMAND, { weights }));
}

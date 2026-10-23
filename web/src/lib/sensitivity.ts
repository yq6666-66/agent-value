import { invoke } from "@tauri-apps/api/core";

// The shell returns the core's winner lists, Ω copy, and flip sentences.
// This module checks the shape. It does not decide who wins.

export const SENSITIVITY_COMMAND = "sensitivity";

export const SENSITIVITY_SOURCES = [
  "store",
  "example6",
  "example7",
  "example8",
  "tie",
  "first",
  "grid",
] as const;

export type SensitivitySource = (typeof SENSITIVITY_SOURCES)[number];

export type SensitivityPoint = {
  a: number;
  b: number;
  g: number;
  winners: string[];
  in_omega_s: boolean;
  in_omega_b: boolean;
};

export type ShareView = {
  id: string;
  fraction: string;
};

export type SentenceView = {
  fixed: string;
  text: string;
};

export type SensitivityChart = {
  points: SensitivityPoint[];
  current_winners: string[];
  omega_mark: string;
  omega_copy: string | null;
  everywhere_id: string | null;
  shares: ShareView[];
  behaviour_mark: string | null;
  behaviour_points: number | null;
  sentences: SentenceView[];
  no_flip_text: string | null;
  floor_copy: string | null;
};

export type SensitivityView = {
  source: string;
  fixture: boolean;
  show_chart: boolean;
  withheld: string | null;
  withheld_text: string | null;
  empty_reason: string | null;
  chart: SensitivityChart | null;
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

function finiteNumber(value: unknown, what: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new Error(`${what} returned an incomplete value`);
  }
  return value;
}

function integer(value: unknown, what: string): number {
  if (typeof value !== "number" || !Number.isInteger(value)) {
    throw new Error(`${what} returned an incomplete value`);
  }
  return value;
}

function optionalInteger(value: unknown, what: string): number | null {
  if (value === null) {
    return null;
  }
  return integer(value, what);
}

function stringList(value: unknown, what: string): string[] {
  if (!Array.isArray(value)) {
    throw new Error(`${what} returned an incomplete value`);
  }
  return value.map((item) => text(item, what));
}

function point(value: unknown): SensitivityPoint {
  const row = objectOf(value, SENSITIVITY_COMMAND);
  return {
    a: finiteNumber(row.a, SENSITIVITY_COMMAND),
    b: finiteNumber(row.b, SENSITIVITY_COMMAND),
    g: finiteNumber(row.g, SENSITIVITY_COMMAND),
    winners: stringList(row.winners, SENSITIVITY_COMMAND),
    in_omega_s: flag(row.in_omega_s, SENSITIVITY_COMMAND),
    in_omega_b: flag(row.in_omega_b, SENSITIVITY_COMMAND),
  };
}

function share(value: unknown): ShareView {
  const row = objectOf(value, SENSITIVITY_COMMAND);
  return {
    id: text(row.id, SENSITIVITY_COMMAND),
    fraction: text(row.fraction, SENSITIVITY_COMMAND),
  };
}

function sentence(value: unknown): SentenceView {
  const row = objectOf(value, SENSITIVITY_COMMAND);
  return {
    fixed: text(row.fixed, SENSITIVITY_COMMAND),
    text: text(row.text, SENSITIVITY_COMMAND),
  };
}

function objectList<T>(value: unknown, map: (item: unknown) => T): T[] {
  if (!Array.isArray(value)) {
    throw new Error(`${SENSITIVITY_COMMAND} returned an incomplete value`);
  }
  return value.map(map);
}

function chartOf(value: unknown): SensitivityChart {
  const row = objectOf(value, SENSITIVITY_COMMAND);
  return {
    points: objectList(row.points, point),
    current_winners: stringList(row.current_winners, SENSITIVITY_COMMAND),
    omega_mark: text(row.omega_mark, SENSITIVITY_COMMAND),
    omega_copy: optionalText(row.omega_copy, SENSITIVITY_COMMAND),
    everywhere_id: optionalText(row.everywhere_id, SENSITIVITY_COMMAND),
    shares: objectList(row.shares, share),
    behaviour_mark: optionalText(row.behaviour_mark, SENSITIVITY_COMMAND),
    behaviour_points: optionalInteger(row.behaviour_points, SENSITIVITY_COMMAND),
    sentences: objectList(row.sentences, sentence),
    no_flip_text: optionalText(row.no_flip_text, SENSITIVITY_COMMAND),
    floor_copy: optionalText(row.floor_copy, SENSITIVITY_COMMAND),
  };
}

export function sensitivityView(value: unknown): SensitivityView {
  const row = objectOf(value, SENSITIVITY_COMMAND);
  const showChart = flag(row.show_chart, SENSITIVITY_COMMAND);
  if (showChart && row.chart === null) {
    throw new Error(`${SENSITIVITY_COMMAND} returned an incomplete value`);
  }
  return {
    source: text(row.source, SENSITIVITY_COMMAND),
    fixture: flag(row.fixture, SENSITIVITY_COMMAND),
    show_chart: showChart,
    withheld: optionalText(row.withheld, SENSITIVITY_COMMAND),
    withheld_text: optionalText(row.withheld_text, SENSITIVITY_COMMAND),
    empty_reason: optionalText(row.empty_reason, SENSITIVITY_COMMAND),
    chart: showChart ? chartOf(row.chart) : null,
  };
}

export async function fetchSensitivity(source: SensitivitySource): Promise<SensitivityView> {
  const value: unknown = await invoke(SENSITIVITY_COMMAND, { source });
  return sensitivityView(value);
}

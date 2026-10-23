import { invoke } from "@tauri-apps/api/core";

// The shell prints every score. This module checks the shape and keeps the
// strings. It does not round, compare, or recompute them.

export const WINDOW_RESULT_COMMAND = "window_result";

export const WINDOW_SOURCES = ["store", "example6", "example7", "example8", "tie"] as const;

export type WindowSource = (typeof WINDOW_SOURCES)[number];

export type ArmView = {
  id: string;
  n_comp: number;
  n_valid: number;
  n_fail: number;
  eligible: boolean;
  failed_gates: string[];
  cost_text: string | null;
  time_text: string | null;
  quality_text: string | null;
  v_text: string | null;
  diagnostic_c_text: string | null;
  diagnostic_admitted: boolean;
  not_admitted: string[];
  on_off: boolean;
  on_rob: boolean;
  on_rec: boolean;
  coarse_bill: boolean;
  cost_source: string | null;
};

export type WindowResult = {
  source: string;
  fixture: boolean;
  scope: string;
  window_index: number | null;
  is_first: boolean;
  first_window_label: string | null;
  weight_source: string | null;
  weight_a: string | null;
  weight_b: string | null;
  weight_g: string | null;
  omega_source: string | null;
  cell_weights_version: string | null;
  reference_version: string | null;
  fallback_version: string | null;
  price_table_version: string | null;
  classification_rules_version: string | null;
  authenticity_grade: string;
  authenticity_source: string;
  authenticity_undetermined: boolean;
  log_integrity_source: string;
  anchor_tier: string | null;
  anchor_headers: string[];
  tier_ii_legend: string | null;
  diagnostic_anchor_c_text: string | null;
  diagnostic_anchor_t_text: string | null;
  diagnostic_anchor_q_text: string | null;
  measured_anchor_c_text: string | null;
  reference_id: string | null;
  arms: ArmView[];
  eligible: string[];
  frontier_off: string[];
  frontier_rob: string[];
  frontier_rec: string[];
  coarse_bill: string[];
  official_top: string[];
  tied: boolean;
  robust_alternative: string[];
  v_withheld: string | null;
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

function armView(value: unknown): ArmView {
  const row = objectOf(value, WINDOW_RESULT_COMMAND);
  return {
    id: text(row.id, WINDOW_RESULT_COMMAND),
    n_comp: integer(row.n_comp, WINDOW_RESULT_COMMAND),
    n_valid: integer(row.n_valid, WINDOW_RESULT_COMMAND),
    n_fail: integer(row.n_fail, WINDOW_RESULT_COMMAND),
    eligible: flag(row.eligible, WINDOW_RESULT_COMMAND),
    failed_gates: stringList(row.failed_gates, WINDOW_RESULT_COMMAND),
    cost_text: optionalText(row.cost_text, WINDOW_RESULT_COMMAND),
    time_text: optionalText(row.time_text, WINDOW_RESULT_COMMAND),
    quality_text: optionalText(row.quality_text, WINDOW_RESULT_COMMAND),
    v_text: optionalText(row.v_text, WINDOW_RESULT_COMMAND),
    diagnostic_c_text: optionalText(row.diagnostic_c_text, WINDOW_RESULT_COMMAND),
    diagnostic_admitted: flag(row.diagnostic_admitted, WINDOW_RESULT_COMMAND),
    not_admitted: stringList(row.not_admitted, WINDOW_RESULT_COMMAND),
    on_off: flag(row.on_off, WINDOW_RESULT_COMMAND),
    on_rob: flag(row.on_rob, WINDOW_RESULT_COMMAND),
    on_rec: flag(row.on_rec, WINDOW_RESULT_COMMAND),
    coarse_bill: flag(row.coarse_bill, WINDOW_RESULT_COMMAND),
    cost_source: optionalText(row.cost_source, WINDOW_RESULT_COMMAND),
  };
}

export function windowResult(value: unknown): WindowResult {
  const row = objectOf(value, WINDOW_RESULT_COMMAND);
  if (!Array.isArray(row.arms)) {
    throw new Error(`${WINDOW_RESULT_COMMAND} returned an incomplete value`);
  }
  return {
    source: text(row.source, WINDOW_RESULT_COMMAND),
    fixture: flag(row.fixture, WINDOW_RESULT_COMMAND),
    scope: text(row.scope, WINDOW_RESULT_COMMAND),
    window_index: optionalInteger(row.window_index, WINDOW_RESULT_COMMAND),
    is_first: flag(row.is_first, WINDOW_RESULT_COMMAND),
    first_window_label: optionalText(row.first_window_label, WINDOW_RESULT_COMMAND),
    weight_source: optionalText(row.weight_source, WINDOW_RESULT_COMMAND),
    weight_a: optionalText(row.weight_a, WINDOW_RESULT_COMMAND),
    weight_b: optionalText(row.weight_b, WINDOW_RESULT_COMMAND),
    weight_g: optionalText(row.weight_g, WINDOW_RESULT_COMMAND),
    omega_source: optionalText(row.omega_source, WINDOW_RESULT_COMMAND),
    cell_weights_version: optionalText(row.cell_weights_version, WINDOW_RESULT_COMMAND),
    reference_version: optionalText(row.reference_version, WINDOW_RESULT_COMMAND),
    fallback_version: optionalText(row.fallback_version, WINDOW_RESULT_COMMAND),
    price_table_version: optionalText(row.price_table_version, WINDOW_RESULT_COMMAND),
    classification_rules_version: optionalText(
      row.classification_rules_version,
      WINDOW_RESULT_COMMAND,
    ),
    authenticity_grade: text(row.authenticity_grade, WINDOW_RESULT_COMMAND),
    authenticity_source: text(row.authenticity_source, WINDOW_RESULT_COMMAND),
    authenticity_undetermined: flag(row.authenticity_undetermined, WINDOW_RESULT_COMMAND),
    log_integrity_source: text(row.log_integrity_source, WINDOW_RESULT_COMMAND),
    anchor_tier: optionalText(row.anchor_tier, WINDOW_RESULT_COMMAND),
    anchor_headers: stringList(row.anchor_headers, WINDOW_RESULT_COMMAND),
    tier_ii_legend: optionalText(row.tier_ii_legend, WINDOW_RESULT_COMMAND),
    diagnostic_anchor_c_text: optionalText(row.diagnostic_anchor_c_text, WINDOW_RESULT_COMMAND),
    diagnostic_anchor_t_text: optionalText(row.diagnostic_anchor_t_text, WINDOW_RESULT_COMMAND),
    diagnostic_anchor_q_text: optionalText(row.diagnostic_anchor_q_text, WINDOW_RESULT_COMMAND),
    measured_anchor_c_text: optionalText(row.measured_anchor_c_text, WINDOW_RESULT_COMMAND),
    reference_id: optionalText(row.reference_id, WINDOW_RESULT_COMMAND),
    arms: row.arms.map(armView),
    eligible: stringList(row.eligible, WINDOW_RESULT_COMMAND),
    frontier_off: stringList(row.frontier_off, WINDOW_RESULT_COMMAND),
    frontier_rob: stringList(row.frontier_rob, WINDOW_RESULT_COMMAND),
    frontier_rec: stringList(row.frontier_rec, WINDOW_RESULT_COMMAND),
    coarse_bill: stringList(row.coarse_bill, WINDOW_RESULT_COMMAND),
    official_top: stringList(row.official_top, WINDOW_RESULT_COMMAND),
    tied: flag(row.tied, WINDOW_RESULT_COMMAND),
    robust_alternative: stringList(row.robust_alternative, WINDOW_RESULT_COMMAND),
    v_withheld: optionalText(row.v_withheld, WINDOW_RESULT_COMMAND),
    empty_reason: optionalText(row.empty_reason, WINDOW_RESULT_COMMAND),
  };
}

export async function fetchWindowResult(source: WindowSource): Promise<WindowResult> {
  const value: unknown = await invoke(WINDOW_RESULT_COMMAND, { source });
  return windowResult(value);
}

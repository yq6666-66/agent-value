import { invoke } from "@tauri-apps/api/core";

export const LIST_ARMS_COMMAND = "list_arms";
export const UPSERT_ARM_COMMAND = "upsert_arm";
export const STORE_DATA_PATH_COMMAND = "store_data_path";
export const EXPORT_STORE_COMMAND = "export_store";
export const DELETE_ALL_COMMAND = "delete_all";

/** Arm fields the shell is allowed to show. No key or prompt field is copied. */
export type ArmRow = {
  id: number;
  model: string;
  provider_kind: string;
  host: string;
  protocol: string;
  client: string;
};

export type ArmInput = {
  model: string;
  provider_kind: string;
  host: string;
  protocol: string;
  client: string;
};

export type ExportReport = {
  schema_version: number;
  table_count: number;
  row_count: number;
  path: string | null;
};

function armRow(value: unknown): ArmRow {
  if (typeof value !== "object" || value === null) {
    throw new Error("list_arms returned a non-object");
  }
  const row = value as Record<string, unknown>;
  const id = row.id;
  const model = row.model;
  const providerKind = row.provider_kind;
  const host = row.host;
  const protocol = row.protocol;
  const client = row.client;
  if (
    typeof id !== "number" ||
    typeof model !== "string" ||
    typeof providerKind !== "string" ||
    typeof host !== "string" ||
    typeof protocol !== "string" ||
    typeof client !== "string"
  ) {
    throw new Error("list_arms returned an incomplete arm");
  }
  return {
    id,
    model,
    provider_kind: providerKind,
    host,
    protocol,
    client,
  };
}

export async function fetchArms(): Promise<ArmRow[]> {
  const value: unknown = await invoke(LIST_ARMS_COMMAND);
  if (!Array.isArray(value)) {
    throw new Error("list_arms did not return a list");
  }
  return value.map(armRow);
}

export async function upsertArm(input: ArmInput): Promise<ArmRow> {
  return armRow(await invoke(UPSERT_ARM_COMMAND, { input }));
}

export async function fetchDataPath(): Promise<string> {
  const value: unknown = await invoke(STORE_DATA_PATH_COMMAND);
  if (typeof value !== "string" || value.length === 0) {
    throw new Error("store_data_path returned an empty value");
  }
  return value;
}

export async function exportStore(): Promise<ExportReport> {
  const value: unknown = await invoke(EXPORT_STORE_COMMAND);
  if (typeof value !== "object" || value === null) {
    throw new Error("export_store returned a non-object");
  }
  const row = value as Record<string, unknown>;
  const schemaVersion = row.schema_version;
  const tableCount = row.table_count;
  const rowCount = row.row_count;
  const path = row.path;
  if (
    typeof schemaVersion !== "number" ||
    typeof tableCount !== "number" ||
    typeof rowCount !== "number" ||
    !(path === null || typeof path === "string")
  ) {
    throw new Error("export_store returned an incomplete report");
  }
  return {
    schema_version: schemaVersion,
    table_count: tableCount,
    row_count: rowCount,
    path,
  };
}

export async function deleteAll(): Promise<void> {
  await invoke(DELETE_ALL_COMMAND);
}

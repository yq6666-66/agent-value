import { invoke } from "@tauri-apps/api/core";

export const WINDOW_CHROME_COMMAND = "window_chrome";

export type WindowSample = "first" | "later";

/** Chrome the shell returns for one sample window. No score is included. */
export type WindowChrome = {
  window_index: number;
  is_first: boolean;
  first_window_label: string | null;
};

export async function fetchWindowChrome(sample: WindowSample): Promise<WindowChrome> {
  const value: unknown = await invoke(WINDOW_CHROME_COMMAND, { sample });
  if (typeof value !== "object" || value === null) {
    throw new Error("window_chrome returned a non-object");
  }
  const row = value as Record<string, unknown>;
  const windowIndex = row.window_index;
  const isFirst = row.is_first;
  const label = row.first_window_label;
  if (typeof windowIndex !== "number" || typeof isFirst !== "boolean") {
    throw new Error("window_chrome returned an incomplete value");
  }
  if (!(label === null || typeof label === "string")) {
    throw new Error("window_chrome returned an incomplete value");
  }
  return {
    window_index: windowIndex,
    is_first: isFirst,
    first_window_label: label,
  };
}

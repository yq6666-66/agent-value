import { invoke } from "@tauri-apps/api/core";

/** Tauri command that runs the example-6 diagnostic path in the core. */
export const DEMO_EXAMPLE6_LAYER_COMMAND = "demo_example6_layer";

/**
 * Read example 6's diagnostic `A_C` through the desktop shell.
 * The returned text is displayed as-is. This module does not parse,
 * round, or recompute it.
 */
export async function fetchDemoExample6Layer(): Promise<string> {
  const value: unknown = await invoke(DEMO_EXAMPLE6_LAYER_COMMAND);
  if (typeof value !== "string" || value.length === 0) {
    throw new Error("demo_example6_layer returned an empty value");
  }
  return value;
}

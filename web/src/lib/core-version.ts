import { invoke } from "@tauri-apps/api/core";

/** Tauri command registered in src-tauri. The web UI does not know the version. */
export const CORE_VERSION_COMMAND = "core_version";

/**
 * Read `agent-value-core`'s `CORE_VERSION` through the desktop shell.
 * The returned string is displayed as-is. This module does not format,
 * default, or invent a version.
 */
export async function fetchCoreVersion(): Promise<string> {
  const value: unknown = await invoke(CORE_VERSION_COMMAND);
  if (typeof value !== "string" || value.length === 0) {
    throw new Error("core_version returned an empty value");
  }
  return value;
}

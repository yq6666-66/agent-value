import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { CORE_VERSION_COMMAND, fetchCoreVersion } from "@/lib/core-version";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

test("returns the string from the core_version command unchanged", async () => {
  vi.mocked(invoke).mockResolvedValue("9.9.9-from-core");

  await expect(fetchCoreVersion()).resolves.toBe("9.9.9-from-core");
  expect(invoke).toHaveBeenCalledWith(CORE_VERSION_COMMAND);
});

test("rejects an empty string instead of inventing a version", async () => {
  vi.mocked(invoke).mockResolvedValue("");

  await expect(fetchCoreVersion()).rejects.toThrow(/empty/);
});

test("rejects a non-string instead of stringifying it", async () => {
  vi.mocked(invoke).mockResolvedValue(1);

  await expect(fetchCoreVersion()).rejects.toThrow(/empty/);
});

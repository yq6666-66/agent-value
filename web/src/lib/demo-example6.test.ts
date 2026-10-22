import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { DEMO_EXAMPLE6_LAYER_COMMAND, fetchDemoExample6Layer } from "@/lib/demo-example6";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

test("returns the anchor text from demo_example6_layer unchanged", async () => {
  vi.mocked(invoke).mockResolvedValue("4.321-from-core");

  await expect(fetchDemoExample6Layer()).resolves.toBe("4.321-from-core");
  expect(invoke).toHaveBeenCalledWith(DEMO_EXAMPLE6_LAYER_COMMAND);
});

test("rejects a number instead of formatting it", async () => {
  vi.mocked(invoke).mockResolvedValue(0.7875);

  await expect(fetchDemoExample6Layer()).rejects.toThrow(/empty/);
});

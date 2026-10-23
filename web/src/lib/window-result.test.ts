import { expect, test } from "vitest";

import { windowResult } from "@/lib/window-result";
import { windowResultFixture } from "@/test-window-result";

test("keeps the shell's printed anchor and rejects a raw number", () => {
  const parsed = windowResult(
    windowResultFixture({ diagnostic_anchor_c_text: "0.7875-from-shell" }),
  );
  expect(parsed.diagnostic_anchor_c_text).toBe("0.7875-from-shell");

  const numeric = {
    ...windowResultFixture(),
    diagnostic_anchor_c_text: 0.7875,
  };
  expect(() => windowResult(numeric)).toThrow(/incomplete/);
});

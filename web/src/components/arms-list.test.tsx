import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { ArmsList } from "@/components/arms-list";
import { LIST_ARMS_COMMAND, UPSERT_ARM_COMMAND } from "@/lib/store";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const shownFromInvoke = {
  id: 7,
  model: "shown-from-invoke",
  provider_kind: "official",
  host: "api.example.test",
  protocol: "anthropic-messages",
  client: "desktop",
};

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

test("lists whatever list_arms returns, then refreshes after upsert", async () => {
  let listed = 0;
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === LIST_ARMS_COMMAND) {
      listed += 1;
      return listed === 1 ? [] : [shownFromInvoke];
    }
    if (cmd === UPSERT_ARM_COMMAND) {
      return shownFromInvoke;
    }
    throw new Error(cmd);
  });

  render(<ArmsList refreshToken={0} />);

  expect(await screen.findByText("还没有臂")).toBeTruthy();
  expect(screen.queryByLabelText(/密钥|api key|secret/i)).toBeNull();
  expect(screen.queryByText("api_key")).toBeNull();

  fireEvent.change(screen.getByLabelText("模型"), { target: { value: "typed-model" } });
  fireEvent.change(screen.getByLabelText("主机"), { target: { value: "api.example.test" } });
  fireEvent.change(screen.getByLabelText("客户端"), { target: { value: "desktop" } });
  fireEvent.click(screen.getByRole("button", { name: "添加" }));

  expect(await screen.findByText(/shown-from-invoke/)).toBeTruthy();
  expect(screen.queryByText("还没有臂")).toBeNull();
  expect(invoke).toHaveBeenCalledWith(UPSERT_ARM_COMMAND, {
    input: {
      model: "typed-model",
      provider_kind: "official",
      host: "api.example.test",
      protocol: "anthropic-messages",
      client: "desktop",
    },
  });
  expect(listed).toBe(2);
});

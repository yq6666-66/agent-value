import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { App } from "@/App";
import { CORE_VERSION_COMMAND } from "@/lib/core-version";
import { DEMO_EXAMPLE6_LAYER_COMMAND } from "@/lib/demo-example6";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

function stubCommands(core: unknown, demo: unknown) {
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === CORE_VERSION_COMMAND) {
      if (core instanceof Error) {
        throw core;
      }
      return core;
    }
    if (cmd === DEMO_EXAMPLE6_LAYER_COMMAND) {
      if (demo instanceof Error) {
        throw demo;
      }
      return demo;
    }
    throw new Error(cmd);
  });
}

test("shows the core_version string from invoke, not a frontend constant", async () => {
  stubCommands("9.9.9-from-core", "4.321-from-core");

  render(<App />);

  expect(await screen.findByText("9.9.9-from-core")).toBeTruthy();
  expect(screen.getByText("已连接")).toBeTruthy();
  expect(screen.queryByText("0.0.0")).toBeNull();
  expect(invoke).toHaveBeenCalledWith(CORE_VERSION_COMMAND);
});

test("shows the example-6 anchor from invoke, not a frontend constant", async () => {
  stubCommands("9.9.9-from-core", "4.321-from-core");

  render(<App />);

  expect(await screen.findByText("4.321-from-core")).toBeTruthy();
  expect(screen.getByText("演示夹具，非你的数据")).toBeTruthy();
  expect(screen.queryByText("0.7875")).toBeNull();
  expect(invoke).toHaveBeenCalledWith(DEMO_EXAMPLE6_LAYER_COMMAND);
});

test("shows a disconnected status when the shell commands fail", async () => {
  const failure = new Error("unavailable");
  stubCommands(failure, failure);

  render(<App />);

  expect((await screen.findAllByText("未能读取")).length).toBe(2);
  expect(screen.getByText("未连接")).toBeTruthy();
  expect(screen.queryByText("9.9.9-from-core")).toBeNull();
  expect(screen.queryByText("4.321-from-core")).toBeNull();
  expect(screen.queryByText("0.7875")).toBeNull();
});

test("retries by calling core_version again", async () => {
  let coreCalls = 0;
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === DEMO_EXAMPLE6_LAYER_COMMAND) {
      return "4.321-from-core";
    }
    if (cmd === CORE_VERSION_COMMAND) {
      coreCalls += 1;
      if (coreCalls === 1) {
        throw new Error("unavailable");
      }
      return "0.0.0-from-core";
    }
    throw new Error(cmd);
  });

  render(<App />);

  fireEvent.click(await screen.findByRole("button", { name: "重试" }));

  expect(await screen.findByText("0.0.0-from-core")).toBeTruthy();
  expect(screen.getByText("已连接")).toBeTruthy();
  expect(screen.getByText("4.321-from-core")).toBeTruthy();
  expect(coreCalls).toBe(2);
  expect(invoke).toHaveBeenCalledWith(CORE_VERSION_COMMAND);
  expect(invoke).toHaveBeenCalledWith(DEMO_EXAMPLE6_LAYER_COMMAND);
});

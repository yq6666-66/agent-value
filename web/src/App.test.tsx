import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { App } from "@/App";
import { CORE_VERSION_COMMAND } from "@/lib/core-version";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

test("shows the core_version string from invoke, not a frontend constant", async () => {
  vi.mocked(invoke).mockResolvedValue("9.9.9-from-core");

  render(<App />);

  expect(await screen.findByText("9.9.9-from-core")).toBeTruthy();
  expect(screen.getByText("已连接")).toBeTruthy();
  expect(screen.queryByText("0.0.0")).toBeNull();
  expect(invoke).toHaveBeenCalledWith(CORE_VERSION_COMMAND);
});

test("shows a disconnected status when the shell command fails", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("unavailable"));

  render(<App />);

  expect(await screen.findByText("未能读取")).toBeTruthy();
  expect(screen.getByText("未连接")).toBeTruthy();
  expect(screen.queryByText("9.9.9-from-core")).toBeNull();
});

test("retries by calling core_version again", async () => {
  vi.mocked(invoke).mockRejectedValueOnce(new Error("unavailable"));
  vi.mocked(invoke).mockResolvedValueOnce("0.0.0-from-core");

  render(<App />);

  fireEvent.click(await screen.findByRole("button", { name: "重试" }));

  expect(await screen.findByText("0.0.0-from-core")).toBeTruthy();
  expect(screen.getByText("已连接")).toBeTruthy();
  expect(invoke).toHaveBeenCalledTimes(2);
  expect(invoke).toHaveBeenNthCalledWith(1, CORE_VERSION_COMMAND);
  expect(invoke).toHaveBeenNthCalledWith(2, CORE_VERSION_COMMAND);
});

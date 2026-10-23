import { fireEvent, render, screen } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, expect, test, vi } from "vitest";

import { SettingsPanel } from "@/components/settings-panel";
import { DELETE_ALL_COMMAND, EXPORT_STORE_COMMAND, STORE_DATA_PATH_COMMAND } from "@/lib/store";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
});

test("shows the data path and export result from invoke", async () => {
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === STORE_DATA_PATH_COMMAND) {
      return "/data/from-shell.sqlite";
    }
    if (cmd === EXPORT_STORE_COMMAND) {
      return {
        schema_version: 2,
        table_count: 4,
        row_count: 1,
        path: "/data/from-shell-export.json",
      };
    }
    if (cmd === DELETE_ALL_COMMAND) {
      return null;
    }
    throw new Error(cmd);
  });

  const onWiped = vi.fn();
  render(<SettingsPanel onWiped={onWiped} />);

  expect(await screen.findByText("/data/from-shell.sqlite")).toBeTruthy();
  expect(screen.getByText(/导出不含提示正文/)).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "导出" }));
  expect(await screen.findByText(/\/data\/from-shell-export\.json/)).toBeTruthy();
  expect(screen.getByText(/已导出 4 张表/)).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: "删除全部" }));
  fireEvent.click(screen.getByRole("button", { name: "确认删除" }));
  expect(await screen.findByRole("button", { name: "删除全部" })).toBeTruthy();
  expect(invoke).toHaveBeenCalledWith(DELETE_ALL_COMMAND);
  expect(onWiped).toHaveBeenCalledOnce();
});

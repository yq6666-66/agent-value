import { useCallback, useEffect, useState } from "react";
import { Settings } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { deleteAll, exportStore, fetchDataPath, type ExportReport } from "@/lib/store";

type PathState =
  | { status: "loading" }
  | { status: "ready"; path: string }
  | { status: "error" };

export function SettingsPanel({ onWiped }: { onWiped: () => void }) {
  const [pathState, setPathState] = useState<PathState>({ status: "loading" });
  const [requestId, setRequestId] = useState(0);
  const [exportReport, setExportReport] = useState<ExportReport | null>(null);
  const [exportError, setExportError] = useState(false);
  const [confirmWipe, setConfirmWipe] = useState(false);
  const [wiping, setWiping] = useState(false);
  const [wipeError, setWipeError] = useState(false);

  useEffect(() => {
    let cancelled = false;
    fetchDataPath().then(
      (path) => {
        if (!cancelled) {
          setPathState({ status: "ready", path });
        }
      },
      () => {
        if (!cancelled) {
          setPathState({ status: "error" });
        }
      },
    );
    return () => {
      cancelled = true;
    };
  }, [requestId]);

  const retry = useCallback(() => {
    setPathState({ status: "loading" });
    setRequestId((id) => id + 1);
  }, []);

  async function onExport() {
    setExportError(false);
    try {
      setExportReport(await exportStore());
    } catch {
      setExportReport(null);
      setExportError(true);
    }
  }

  async function onConfirmWipe() {
    setWiping(true);
    setWipeError(false);
    try {
      await deleteAll();
      setConfirmWipe(false);
      setExportReport(null);
      onWiped();
    } catch {
      setWipeError(true);
    } finally {
      setWiping(false);
    }
  }

  const pathText =
    pathState.status === "ready"
      ? pathState.path
      : pathState.status === "loading"
        ? "读取中"
        : "未能读取路径";

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Settings aria-hidden className="size-4" />
          设置
        </CardTitle>
        <CardDescription>数据只在本地。导出不含提示正文。</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <dl className="grid grid-cols-[max-content_1fr] items-baseline gap-x-6 gap-y-3">
          <dt className="text-muted-foreground">数据路径</dt>
          <dd className="font-mono text-sm break-all">{pathText}</dd>
        </dl>
        {pathState.status === "error" ? (
          <div>
            <Button type="button" variant="outline" onClick={retry}>
              重试
            </Button>
          </div>
        ) : null}
        <div className="flex flex-wrap gap-2">
          <Button type="button" variant="outline" onClick={onExport}>
            导出
          </Button>
          {confirmWipe ? (
            <>
              <Button type="button" variant="destructive" disabled={wiping} onClick={onConfirmWipe}>
                确认删除
              </Button>
              <Button type="button" variant="outline" onClick={() => setConfirmWipe(false)}>
                取消
              </Button>
            </>
          ) : (
            <Button type="button" variant="destructive" onClick={() => setConfirmWipe(true)}>
              删除全部
            </Button>
          )}
        </div>
        {exportReport ? (
          <p className="text-sm break-all">
            已导出 {exportReport.table_count} 张表
            {exportReport.path ? `，文件 ${exportReport.path}` : ""}
          </p>
        ) : null}
        {exportError ? <p className="text-sm text-destructive">未能导出</p> : null}
        {wipeError ? <p className="text-sm text-destructive">未能删除</p> : null}
      </CardContent>
    </Card>
  );
}

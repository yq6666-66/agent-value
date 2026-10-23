import { useCallback, useEffect, useState } from "react";
import { Info } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { fetchCoreVersion } from "@/lib/core-version";

type CoreVersionState =
  | { status: "loading" }
  | { status: "ready"; version: string }
  | { status: "error" };

export function AboutStatus() {
  const [state, setState] = useState<CoreVersionState>({ status: "loading" });
  const [requestId, setRequestId] = useState(0);

  useEffect(() => {
    let cancelled = false;
    fetchCoreVersion().then(
      (version) => {
        if (!cancelled) {
          setState({ status: "ready", version });
        }
      },
      () => {
        if (!cancelled) {
          setState({ status: "error" });
        }
      },
    );
    return () => {
      cancelled = true;
    };
  }, [requestId]);

  const retry = useCallback(() => {
    setState({ status: "loading" });
    setRequestId((id) => id + 1);
  }, []);

  const versionText =
    state.status === "ready" ? state.version : state.status === "loading" ? "读取中" : "未能读取";
  const statusText =
    state.status === "ready" ? "已连接" : state.status === "loading" ? "连接中" : "未连接";

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Info aria-hidden className="size-4" />
          关于
        </CardTitle>
        <CardDescription>本地核心的版本。界面只负责显示。</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <dl className="grid grid-cols-[max-content_1fr] items-center gap-x-6 gap-y-3">
          <dt className="text-muted-foreground">核心版本</dt>
          <dd className="font-mono text-sm" aria-live="polite">
            {versionText}
          </dd>
          <dt className="text-muted-foreground">状态</dt>
          <dd>
            <Badge variant={state.status === "error" ? "destructive" : "secondary"}>
              {statusText}
            </Badge>
          </dd>
        </dl>
        {state.status === "error" ? (
          <div>
            <Button type="button" variant="outline" onClick={retry}>
              重试
            </Button>
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}

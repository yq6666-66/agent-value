import { useCallback, useEffect, useState } from "react";
import { FlaskConical } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { fetchDemoExample6Layer } from "@/lib/demo-example6";

type DemoState =
  | { status: "loading" }
  | { status: "ready"; aC: string }
  | { status: "error" };

export function DemoFixture() {
  const [state, setState] = useState<DemoState>({ status: "loading" });
  const [requestId, setRequestId] = useState(0);

  useEffect(() => {
    let cancelled = false;
    fetchDemoExample6Layer().then(
      (aC) => {
        if (!cancelled) {
          setState({ status: "ready", aC });
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

  const valueText =
    state.status === "ready" ? state.aC : state.status === "loading" ? "读取中" : "未能读取";

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <FlaskConical aria-hidden className="size-4" />
          诊断锚点
        </CardTitle>
        <CardDescription>演示夹具，非你的数据</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <dl className="grid grid-cols-[max-content_1fr] items-center gap-x-6 gap-y-3">
          <dt className="text-muted-foreground">A_C</dt>
          <dd className="font-mono text-sm break-all" aria-live="polite">
            {valueText}
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

import { useEffect, useState } from "react";
import { Scale } from "lucide-react";

import { Button } from "@/components/ui/button";
import { fetchWindowChrome, type WindowChrome, type WindowSample } from "@/lib/window-chrome";

type ChromeState =
  | { status: "loading" }
  | { status: "ready"; chrome: WindowChrome }
  | { status: "error" };

export function WeightPanel() {
  const [sample, setSample] = useState<WindowSample>("first");
  const [state, setState] = useState<ChromeState>({ status: "loading" });
  const [requestId, setRequestId] = useState(0);

  useEffect(() => {
    let cancelled = false;
    fetchWindowChrome(sample).then(
      (chrome) => {
        if (!cancelled) {
          setState({ status: "ready", chrome });
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
  }, [sample, requestId]);

  const label = state.status === "ready" ? state.chrome.first_window_label : null;

  return (
    <section className="flex h-full min-h-0 flex-col gap-4 overflow-auto p-4" aria-label="按你的权重">
      <h2 className="flex items-center gap-2 text-base font-medium">
        <Scale aria-hidden className="size-4" />
        按你的权重
      </h2>
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant={sample === "first" ? "default" : "outline"}
          aria-pressed={sample === "first"}
          onClick={() => setSample("first")}
        >
          第一窗
        </Button>
        <Button
          type="button"
          variant={sample === "later" ? "default" : "outline"}
          aria-pressed={sample === "later"}
          onClick={() => setSample("later")}
        >
          下一窗
        </Button>
      </div>
      {state.status === "loading" ? <p className="text-sm text-muted-foreground">读取中</p> : null}
      {state.status === "error" ? (
        <div className="flex flex-col items-start gap-3">
          <p className="text-sm text-destructive">未能读取窗口</p>
          <Button type="button" variant="outline" onClick={() => setRequestId((id) => id + 1)}>
            重试
          </Button>
        </div>
      ) : null}
      {state.status === "ready" && label !== null ? (
        <p className="text-sm leading-6">{label}</p>
      ) : null}
      {state.status === "ready" && label === null ? (
        <dl className="grid grid-cols-[max-content_1fr] items-baseline gap-x-6 gap-y-2">
          <dt className="text-muted-foreground">V</dt>
          <dd className="text-sm text-muted-foreground">尚未接入</dd>
        </dl>
      ) : null}
    </section>
  );
}

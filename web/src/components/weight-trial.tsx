import { useEffect, useState } from "react";
import { SlidersHorizontal } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  beginWindow,
  editWeights,
  fetchWeightContext,
  formatWeights,
  type TrialView,
  type WeightContext,
} from "@/lib/elicit";

const fieldClass =
  "h-8 w-full rounded-lg border border-input bg-background px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

type LoadState =
  | { status: "loading" }
  | { status: "ready"; context: WeightContext }
  | { status: "error" };

function parseField(value: string): number | null {
  const trimmed = value.trim();
  if (trimmed.length === 0) {
    return null;
  }
  const parsed = Number(trimmed);
  if (!Number.isFinite(parsed)) {
    return null;
  }
  return parsed;
}

function failureText(err: unknown): string {
  const text = err instanceof Error ? err.message : String(err);
  if (text.includes("no-window-in-progress")) {
    return "当前没有进行中的窗，没有写入正式权重";
  }
  return "未能保存试算";
}

export function WeightTrial() {
  const [state, setState] = useState<LoadState>({ status: "loading" });
  const [requestId, setRequestId] = useState(0);
  const [cost, setCost] = useState("");
  const [time, setTime] = useState("");
  const [quality, setQuality] = useState("");
  const [pending, setPending] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [trial, setTrial] = useState<TrialView | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchWeightContext().then(
      (context) => {
        if (!cancelled) {
          setState({ status: "ready", context });
          setTrial(context.trial);
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

  async function onBegin() {
    setPending(true);
    setFormError(null);
    try {
      const context = await beginWindow();
      setState({ status: "ready", context });
      setTrial(context.trial);
    } catch {
      setFormError("未能开始本窗");
    } finally {
      setPending(false);
    }
  }

  async function onEdit() {
    if (state.status !== "ready" || !state.context.in_window) {
      return;
    }
    const weights = {
      a: parseField(cost),
      b: parseField(time),
      g: parseField(quality),
    };
    if (weights.a === null || weights.b === null || weights.g === null) {
      setFormError("请填写三个数字");
      return;
    }
    setPending(true);
    setFormError(null);
    try {
      setTrial(await editWeights({ a: weights.a, b: weights.b, g: weights.g }));
    } catch (err) {
      setFormError(failureText(err));
    } finally {
      setPending(false);
    }
  }

  const official =
    trial?.official ?? (state.status === "ready" ? state.context.official : null);

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <SlidersHorizontal aria-hidden className="size-4" />
          窗中改权重
        </CardTitle>
        <CardDescription>窗中的改动只出试算，不覆盖本窗正式权重。</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {state.status === "loading" ? <p className="text-sm text-muted-foreground">读取中</p> : null}
        {state.status === "error" ? (
          <div className="flex flex-col items-start gap-3">
            <p className="text-sm text-destructive">未能读取权重</p>
            <Button type="button" variant="outline" onClick={() => setRequestId((id) => id + 1)}>
              重试
            </Button>
          </div>
        ) : null}
        {state.status === "ready" && !state.context.in_window ? (
          <div className="flex flex-col items-start gap-3">
            <p className="text-sm leading-6">
              尚未开窗。当前正式权重 {formatWeights(state.context.official)}。这时改权重不会写入。
            </p>
            <Button type="button" onClick={onBegin} disabled={pending}>
              开始本窗
            </Button>
          </div>
        ) : null}
        {state.status === "ready" && state.context.in_window && official ? (
          <div className="flex flex-col gap-3">
            <p>本窗正式 {formatWeights(official)}</p>
            {trial ? (
              <div className="flex flex-col gap-2">
                <p>{trial.label}</p>
                <p>试算 {formatWeights(trial.trial)}</p>
                <p>本窗正式权重未改。</p>
              </div>
            ) : null}
            <label className="flex flex-col gap-1 text-sm">
              成本
              <input
                aria-label="成本权重"
                className={fieldClass}
                value={cost}
                onChange={(event) => setCost(event.target.value)}
                inputMode="decimal"
                autoComplete="off"
              />
            </label>
            <label className="flex flex-col gap-1 text-sm">
              时间
              <input
                aria-label="时间权重"
                className={fieldClass}
                value={time}
                onChange={(event) => setTime(event.target.value)}
                inputMode="decimal"
                autoComplete="off"
              />
            </label>
            <label className="flex flex-col gap-1 text-sm">
              质量
              <input
                aria-label="质量权重"
                className={fieldClass}
                value={quality}
                onChange={(event) => setQuality(event.target.value)}
                inputMode="decimal"
                autoComplete="off"
              />
            </label>
            <Button type="button" onClick={onEdit} disabled={pending}>
              保存试算
            </Button>
          </div>
        ) : null}
        {formError ? <p className="text-sm text-destructive">{formError}</p> : null}
      </CardContent>
    </Card>
  );
}

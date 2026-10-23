import { useEffect, useState } from "react";
import { Grid2x2 } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  fetchSensitivity,
  type SensitivityChart,
  type SensitivityPoint,
  type SensitivitySource,
  type SensitivityView,
} from "@/lib/sensitivity";
import type { WindowSource } from "@/lib/window-result";

type PanelState =
  | { status: "loading" }
  | { status: "ready"; view: SensitivityView }
  | { status: "error" };

const EXTRAS: { id: Extract<SensitivitySource, "first" | "grid">; label: string }[] = [
  { id: "first", label: "第一窗" },
  { id: "grid", label: "网格" },
];

const FILLS = [
  "oklch(0.62 0.14 250)",
  "oklch(0.66 0.14 145)",
  "oklch(0.70 0.14 85)",
  "oklch(0.62 0.16 25)",
  "oklch(0.60 0.14 320)",
  "oklch(0.64 0.12 200)",
];

function groupKey(winners: string[]): string {
  return winners.join("\u0000");
}

function fillsFor(points: SensitivityPoint[]): Map<string, string> {
  const fills = new Map<string, string>();
  for (const point of points) {
    const key = groupKey(point.winners);
    if (!fills.has(key)) {
      const fill = FILLS[fills.size % FILLS.length];
      if (fill !== undefined) {
        fills.set(key, fill);
      }
    }
  }
  return fills;
}

function WinnerChart({ chart }: { chart: SensitivityChart }) {
  const fills = fillsFor(chart.points);
  const [sentenceIndex, setSentenceIndex] = useState(0);
  const sentence = chart.sentences[sentenceIndex] ?? chart.sentences[0];

  return (
    <div className="flex flex-col gap-3">
      {chart.everywhere_id !== null && chart.omega_copy !== null ? (
        <p className="text-sm leading-6">
          {chart.everywhere_id} {chart.omega_copy}
        </p>
      ) : null}
      <p className="text-sm text-muted-foreground">权重范围 {chart.omega_mark}</p>
      {chart.behaviour_mark !== null ? (
        <p className="text-sm text-muted-foreground">
          {chart.behaviour_mark}
          {chart.behaviour_points !== null ? ` ${chart.behaviour_points}` : ""}
        </p>
      ) : null}
      {chart.floor_copy !== null ? <p className="text-sm leading-6">{chart.floor_copy}</p> : null}
      {chart.current_winners.length > 0 ? (
        <p className="text-sm">当前 {chart.current_winners.join("、")}</p>
      ) : null}
      {chart.shares.length > 0 ? (
        <ul className="text-sm">
          {chart.shares.map((share) => (
            <li key={share.id}>
              {share.id} {share.fraction}
            </li>
          ))}
        </ul>
      ) : null}
      <p className="text-xs text-muted-foreground">横轴是成本权重，纵轴是时间权重。</p>
      <svg role="img" aria-label="胜者图" viewBox="0 0 100 100" className="h-64 w-full">
        {chart.points.map((point, index) => (
          <circle
            key={`${groupKey(point.winners)}-${index}`}
            cx={8 + point.a * 84}
            cy={92 - point.b * 84}
            r={point.in_omega_b ? 2.2 : 1.7}
            fill={fills.get(groupKey(point.winners))}
            stroke={point.in_omega_s ? "currentColor" : "none"}
            strokeWidth={point.in_omega_s ? 0.7 : 0}
            data-winners={point.winners.join(",")}
            data-omega-s={point.in_omega_s ? "true" : "false"}
            data-omega-b={point.in_omega_b ? "true" : "false"}
          >
            <title>{point.winners.join("、")}</title>
          </circle>
        ))}
      </svg>
      <ul className="flex flex-wrap gap-3 text-xs text-muted-foreground">
        {[...fills.entries()].map(([key, fill]) => (
          <li key={key} className="flex items-center gap-1">
            <span className="inline-block size-2 rounded-full" style={{ background: fill }} />
            {key.split("\u0000").join("、")}
          </li>
        ))}
      </ul>
      {chart.sentences.length > 0 ? (
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap gap-2" role="group" aria-label="翻转句">
            {chart.sentences.map((item, index) => (
              <Button
                key={`${item.fixed}-${index}`}
                type="button"
                variant={index === sentenceIndex ? "default" : "outline"}
                aria-pressed={index === sentenceIndex}
                onClick={() => setSentenceIndex(index)}
              >
                {item.fixed}
              </Button>
            ))}
          </div>
          {sentence !== undefined ? <p className="text-sm leading-6">{sentence.text}</p> : null}
        </div>
      ) : null}
      {chart.no_flip_text !== null ? (
        <p className="text-sm leading-6">{chart.no_flip_text}</p>
      ) : null}
    </div>
  );
}

export function SensitivityPanel({ windowSource }: { windowSource: WindowSource }) {
  const [extra, setExtra] = useState<Extract<SensitivitySource, "first" | "grid"> | null>(null);
  const [state, setState] = useState<PanelState>({ status: "loading" });
  const [requestId, setRequestId] = useState(0);
  const source: SensitivitySource = extra ?? windowSource;

  useEffect(() => {
    let cancelled = false;
    fetchSensitivity(source).then(
      (view) => {
        if (!cancelled) {
          setState({ status: "ready", view });
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
  }, [source, requestId]);

  function choose(next: Extract<SensitivitySource, "first" | "grid">) {
    setExtra((current) => (current === next ? null : next));
    setState({ status: "loading" });
  }

  const view = state.status === "ready" ? state.view : null;

  return (
    <section className="flex flex-col gap-3 border-t pt-4" aria-label="敏感度">
      <h3 className="flex items-center gap-2 text-sm font-medium">
        <Grid2x2 aria-hidden className="size-4" />
        敏感度
      </h3>
      <p className="text-sm text-muted-foreground">胜者和句子来自核心库。这里不重算。</p>
      <div className="flex flex-wrap gap-2" role="group" aria-label="敏感度来源">
        {EXTRAS.map((item) => (
          <Button
            key={item.id}
            type="button"
            size="sm"
            variant={extra === item.id ? "default" : "outline"}
            aria-pressed={extra === item.id}
            onClick={() => choose(item.id)}
          >
            {item.label}
          </Button>
        ))}
      </div>
      {state.status === "loading" ? <p className="text-sm text-muted-foreground">读取中</p> : null}
      {state.status === "error" ? (
        <div className="flex flex-col items-start gap-3">
          <p className="text-sm text-destructive">未能读取敏感度</p>
          <Button type="button" variant="outline" onClick={() => setRequestId((id) => id + 1)}>
            重试
          </Button>
        </div>
      ) : null}
      {view !== null && view.fixture ? <p className="text-sm">演示夹具，非你的数据</p> : null}
      {view !== null && !view.show_chart ? (
        <>
          {view.empty_reason !== null ? (
            <p className="text-sm leading-6">{view.empty_reason}</p>
          ) : null}
          {view.withheld_text !== null ? (
            <p className="text-sm leading-6">{view.withheld_text}</p>
          ) : null}
        </>
      ) : null}
      {view !== null && view.show_chart && view.chart !== null ? (
        <WinnerChart key={`${source}-${requestId}`} chart={view.chart} />
      ) : null}
    </section>
  );
}

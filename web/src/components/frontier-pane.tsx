import { useEffect, useState } from "react";
import { ChartNoAxesCombined } from "lucide-react";

import { SensitivityPanel } from "@/components/sensitivity-panel";
import { Button } from "@/components/ui/button";
import {
  fetchWindowResult,
  type WindowResult,
  type WindowSource,
} from "@/lib/window-result";

type ResultState =
  | { status: "loading" }
  | { status: "ready"; result: WindowResult }
  | { status: "error" };

const SOURCES: { id: WindowSource; label: string }[] = [
  { id: "store", label: "本窗" },
  { id: "example6", label: "例6" },
  { id: "example7", label: "例7" },
  { id: "example8", label: "例8" },
  { id: "tie", label: "并列" },
];

function shown(value: string | null): string {
  return value ?? "—";
}

function SourceLine({ label, value }: { label: string; value: string | null }) {
  return (
    <>
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="text-sm break-all">{shown(value)}</dd>
    </>
  );
}

export function FrontierPane({ refreshToken = 0 }: { refreshToken?: number }) {
  const [source, setSource] = useState<WindowSource>("store");
  const [state, setState] = useState<ResultState>({ status: "loading" });
  const [requestId, setRequestId] = useState(0);

  useEffect(() => {
    let cancelled = false;
    fetchWindowResult(source).then(
      (result) => {
        if (!cancelled) {
          setState({ status: "ready", result });
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
  }, [source, requestId, refreshToken]);

  function choose(next: WindowSource) {
    setSource(next);
    setState({ status: "loading" });
  }

  const result = state.status === "ready" ? state.result : null;

  return (
    <section className="flex h-full min-h-0 flex-col gap-3 overflow-auto p-4" aria-label="前沿">
      <h2 className="flex items-center gap-2 text-base font-medium">
        <ChartNoAxesCombined aria-hidden className="size-4" />
        前沿
      </h2>
      <p className="text-sm text-muted-foreground">数字来自核心库。这里不计算分数。</p>
      <div className="flex flex-wrap gap-2" role="group" aria-label="数据来源">
        {SOURCES.map((item) => (
          <Button
            key={item.id}
            type="button"
            variant={source === item.id ? "default" : "outline"}
            aria-pressed={source === item.id}
            onClick={() => choose(item.id)}
          >
            {item.label}
          </Button>
        ))}
      </div>
      {state.status === "loading" ? <p className="text-sm text-muted-foreground">读取中</p> : null}
      {state.status === "error" ? (
        <div className="flex flex-col items-start gap-3">
          <p className="text-sm text-destructive">未能读取</p>
          <Button type="button" variant="outline" onClick={() => setRequestId((id) => id + 1)}>
            重试
          </Button>
        </div>
      ) : null}
      {result !== null ? (
        <>
          {result.fixture ? <p className="text-sm">演示夹具，非你的数据</p> : null}
          {result.empty_reason !== null ? (
            <p className="text-sm leading-6">{result.empty_reason}</p>
          ) : null}
          <dl className="grid grid-cols-[max-content_1fr] items-baseline gap-x-6 gap-y-2">
            <SourceLine label="权重来源" value={result.weight_source} />
            <SourceLine label="格子权重来源" value={result.omega_source} />
            <SourceLine label="格子权重版本" value={result.cell_weights_version} />
            <SourceLine label="参照" value={result.reference_version} />
            <SourceLine label="兜底" value={result.fallback_version} />
            <SourceLine label="价格表" value={result.price_table_version} />
            <SourceLine label="分类规则" value={result.classification_rules_version} />
            <dt className="text-muted-foreground">真实性</dt>
            <dd className="text-sm break-all">
              {result.authenticity_note
                ? `${result.authenticity_grade} · ${result.authenticity_source} · ${result.authenticity_note}`
                : `${result.authenticity_grade} · ${result.authenticity_source}`}
            </dd>
            <dt className="text-muted-foreground">日志</dt>
            <dd className="text-sm break-all">{result.log_integrity_source}</dd>
            <SourceLine label="成本权重" value={result.weight_a} />
            <SourceLine label="时间权重" value={result.weight_b} />
            <SourceLine label="质量权重" value={result.weight_g} />
            <SourceLine label="诊断 A_C" value={result.diagnostic_anchor_c_text} />
            <SourceLine label="诊断 A_T" value={result.diagnostic_anchor_t_text} />
            <SourceLine label="诊断 A_Q" value={result.diagnostic_anchor_q_text} />
            <SourceLine label="实测 A_C" value={result.measured_anchor_c_text} />
            <SourceLine label="锚点层级" value={result.anchor_tier} />
          </dl>
          {result.anchor_headers.length > 0 ? (
            <ul className="text-sm leading-6">
              {result.anchor_headers.map((header) => (
                <li key={header}>{header}</li>
              ))}
            </ul>
          ) : null}
          {result.tier_ii_legend !== null ? (
            <p className="text-sm leading-6 text-muted-foreground">{result.tier_ii_legend}</p>
          ) : null}
          {result.tied ? (
            <div className="flex flex-col gap-1">
              <p className="text-sm font-medium">并列整组</p>
              <ul className="text-sm">
                {result.official_top.map((id) => (
                  <li key={id}>{id}</li>
                ))}
              </ul>
            </div>
          ) : null}
          {result.v_withheld !== null ? (
            <p className="text-sm leading-6">{result.v_withheld}</p>
          ) : null}
          <div className="overflow-auto rounded-lg ring-1 ring-foreground/10">
            <table className="w-full text-sm">
              <thead className="border-b text-xs text-muted-foreground">
                <tr>
                  <th className="px-3 py-2 text-left font-normal">臂</th>
                  <th className="px-3 py-2 text-left font-normal">成本</th>
                  <th className="px-3 py-2 text-left font-normal">时间</th>
                  <th className="px-3 py-2 text-left font-normal">质量</th>
                  <th className="px-3 py-2 text-left font-normal">V</th>
                  <th className="px-3 py-2 text-left font-normal">前沿</th>
                  <th className="px-3 py-2 text-left font-normal">稳健</th>
                  <th className="px-3 py-2 text-left font-normal">推荐</th>
                </tr>
              </thead>
              <tbody>
                {result.arms.length === 0 ? (
                  <tr>
                    <td className="px-3 py-6 text-muted-foreground" colSpan={8}>
                      空
                    </td>
                  </tr>
                ) : (
                  result.arms.map((arm) => (
                    <tr key={arm.id} className="border-b last:border-b-0">
                      <td className="px-3 py-2">{arm.id}</td>
                      <td className="px-3 py-2 font-mono">{shown(arm.cost_text)}</td>
                      <td className="px-3 py-2 font-mono">{shown(arm.time_text)}</td>
                      <td className="px-3 py-2 font-mono">{shown(arm.quality_text)}</td>
                      <td className="px-3 py-2 font-mono">{shown(arm.v_text)}</td>
                      <td className="px-3 py-2">{arm.on_off ? "是" : "—"}</td>
                      <td className="px-3 py-2">{arm.on_rob ? "是" : "—"}</td>
                      <td className="px-3 py-2">{arm.on_rec ? "是" : "—"}</td>
                    </tr>
                  ))
                )}
              </tbody>
            </table>
          </div>
        </>
      ) : null}
      <SensitivityPanel windowSource={source} refreshToken={refreshToken} />
    </section>
  );
}

import { useEffect, useRef, useState, type FormEvent } from "react";
import { Radar } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  PROBE_RUN_FAILED,
  PROBE_RUN_IN_PROGRESS,
  fetchProbePanel,
  probeRunOffered,
  runProbeOnce,
  setProbeConsent,
  type ProbePanel,
  type ProbeSource,
} from "@/lib/probe";

const fieldClass =
  "h-8 w-full rounded-lg border border-input bg-background px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

type PanelState =
  | { status: "loading" }
  | { status: "ready"; view: ProbePanel }
  | { status: "error" }
  | { status: "save-error"; view: ProbePanel };

const SOURCES: { id: ProbeSource; label: string }[] = [
  { id: "store", label: "本机记录" },
  { id: "fixture", label: "夹具回放" },
];

export function ProbePanel({ refreshToken = 0 }: { refreshToken?: number }) {
  const [source, setSource] = useState<ProbeSource>("store");
  const [state, setState] = useState<PanelState>({ status: "loading" });
  const [requestId, setRequestId] = useState(0);
  const [saving, setSaving] = useState(false);
  const [keyEnv, setKeyEnv] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [armId, setArmId] = useState("");
  const [runPhase, setRunPhase] = useState<"idle" | "running">("idle");
  const [runDetail, setRunDetail] = useState<string | null>(null);
  const runningRef = useRef(false);

  useEffect(() => {
    let cancelled = false;
    setState({ status: "loading" });
    fetchProbePanel(source).then(
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
  }, [source, requestId, refreshToken]);

  function choose(next: ProbeSource) {
    setRunDetail(null);
    setSource(next);
    setState({ status: "loading" });
  }

  async function chooseConsent(accepted: boolean) {
    setSaving(true);
    try {
      const view = await setProbeConsent(accepted, source);
      if (view.source === source) {
        setState({ status: "ready", view });
      }
    } catch {
      setState((current) =>
        current.status === "ready" || current.status === "save-error"
          ? { status: "save-error", view: current.view }
          : { status: "error" },
      );
    } finally {
      setSaving(false);
    }
  }

  const view =
    (state.status === "ready" || state.status === "save-error") && state.view.source === source
      ? state.view
      : null;
  const showLoading = state.status === "loading" || (state.status !== "error" && view === null);
  const offerRun = view !== null && probeRunOffered(view) && runPhase !== "running";
  const selectedArm =
    view !== null && view.run_targets.some((item) => item.id === armId)
      ? armId
      : (view?.run_targets[0]?.id ?? "");
  const canSubmit = selectedArm !== "" && keyEnv.trim() !== "" && baseUrl.trim() !== "";

  async function onRun(event: FormEvent) {
    event.preventDefault();
    if (runningRef.current || view === null || !probeRunOffered(view)) {
      return;
    }
    const key_env = keyEnv.trim();
    const base_url = baseUrl.trim();
    if (selectedArm === "" || key_env === "" || base_url === "") {
      return;
    }
    runningRef.current = true;
    setRunPhase("running");
    setRunDetail(null);
    try {
      const outcome = await runProbeOnce({
        arm_id: selectedArm,
        key_env,
        base_url,
      });
      setRunDetail(outcome.launch === "completed" ? null : outcome.detail);
      try {
        const next = await fetchProbePanel("store");
        setSource("store");
        setState({ status: "ready", view: next });
      } catch {
        setState({ status: "error" });
      }
    } catch {
      setRunDetail(PROBE_RUN_FAILED);
    } finally {
      runningRef.current = false;
      setRunPhase("idle");
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Radar aria-hidden className="size-4" />
          探针
        </CardTitle>
        <CardDescription>同意、预算和档位来自本地记录与核心库。</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="flex flex-wrap gap-2" role="group" aria-label="探针来源">
          {SOURCES.map((item) => (
            <Button
              key={item.id}
              type="button"
              size="sm"
              variant={source === item.id ? "default" : "outline"}
              aria-pressed={source === item.id}
              onClick={() => choose(item.id)}
            >
              {item.label}
            </Button>
          ))}
        </div>
        {showLoading ? <p className="text-sm text-muted-foreground">读取中</p> : null}
        {state.status === "error" ? (
          <div className="flex flex-col items-start gap-3">
            <p className="text-sm text-destructive">未能读取探针</p>
            <Button type="button" variant="outline" onClick={() => setRequestId((id) => id + 1)}>
              重试
            </Button>
          </div>
        ) : null}
        {runPhase === "running" ? (
          <p className="text-sm leading-6" role="status">
            {PROBE_RUN_IN_PROGRESS}
          </p>
        ) : null}
        {runDetail !== null ? (
          <p className="text-sm leading-6" role="status">
            {runDetail}
          </p>
        ) : null}
        {view !== null && view.fixture ? <p className="text-sm">演示夹具，非你的数据</p> : null}
        {view !== null ? (
          <div className="flex flex-col gap-3">
            {!view.decided ? (
              <div className="flex flex-col items-start gap-3">
                <p className="text-sm leading-6">{view.consent_prompt}</p>
                <div className="flex flex-wrap gap-2">
                  <Button type="button" disabled={saving} onClick={() => chooseConsent(true)}>
                    同意
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    disabled={saving}
                    onClick={() => chooseConsent(false)}
                  >
                    暂不
                  </Button>
                </div>
              </div>
            ) : null}
            <p className="text-sm leading-6">{view.launch_label}</p>
            {offerRun ? (
              <form className="flex flex-col gap-3" onSubmit={onRun}>
                {view.run_targets.length === 0 ? (
                  <p className="text-sm leading-6">还没有可选的臂。</p>
                ) : (
                  <div className="flex flex-col gap-1">
                    <label htmlFor="probe-run-arm" className="text-sm">
                      臂
                    </label>
                    <select
                      id="probe-run-arm"
                      className={fieldClass}
                      value={selectedArm}
                      onChange={(event) => setArmId(event.target.value)}
                    >
                      {view.run_targets.map((target) => (
                        <option key={target.id} value={target.id}>
                          {target.id}
                        </option>
                      ))}
                    </select>
                  </div>
                )}
                <div className="flex flex-col gap-1">
                  <label htmlFor="probe-key-env" className="text-sm">
                    密钥变量名
                  </label>
                  <input
                    id="probe-key-env"
                    className={fieldClass}
                    value={keyEnv}
                    onChange={(event) => setKeyEnv(event.target.value)}
                    placeholder="RELAY_API_KEY"
                    autoComplete="off"
                    spellCheck={false}
                  />
                  <p className="text-sm text-muted-foreground">只填变量名，不填密钥。</p>
                </div>
                <div className="flex flex-col gap-1">
                  <label htmlFor="probe-base-url" className="text-sm">
                    基址
                  </label>
                  <input
                    id="probe-base-url"
                    className={fieldClass}
                    value={baseUrl}
                    onChange={(event) => setBaseUrl(event.target.value)}
                    placeholder="https://example.test"
                    autoComplete="off"
                    spellCheck={false}
                  />
                </div>
                <div>
                  <Button type="submit" disabled={!canSubmit}>
                    跑一次
                  </Button>
                </div>
              </form>
            ) : null}
            <p className="text-sm leading-6">{view.observation_label}</p>
            <p className="text-sm leading-6">{view.grade_run_days_label}</p>
            <p className="text-sm leading-6">{view.monthly_budget_label}</p>
            <p className="text-sm leading-6">{view.spent_label}</p>
            <p className="text-sm leading-6">{view.estimate_label}</p>
            {view.over_budget_prompt !== null ? (
              <p className="text-sm leading-6" role="status">
                {view.over_budget_prompt}
              </p>
            ) : null}
            {state.status === "save-error" ? (
              <p className="text-sm text-destructive">未能保存同意</p>
            ) : null}
            {view.empty_reason !== null ? (
              <p className="text-sm leading-6">{view.empty_reason}</p>
            ) : null}
            {view.arms.map((arm) => (
              <div key={arm.id} className="flex flex-col gap-2">
                <p className="text-sm font-medium">{arm.id}</p>
                <div className="flex flex-wrap items-center gap-2">
                  {arm.grade !== null ? <span className="text-sm">{arm.grade}</span> : null}
                  {arm.reference_label !== null ? (
                    <Badge variant="outline">{arm.reference_label}</Badge>
                  ) : null}
                </div>
                {arm.labels.length > 0 ? (
                  <p className="text-sm leading-6">{arm.labels.join(" · ")}</p>
                ) : null}
                <ul className="text-sm leading-6">
                  {arm.classes.map((item) => (
                    <li key={`${arm.id}-${item.class}-${item.run_day}`}>
                      {item.class} {item.verdict} {item.run_day}
                      <span className="block text-muted-foreground">{item.evidence}</span>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}

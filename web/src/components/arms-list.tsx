import { useCallback, useEffect, useState, type FormEvent } from "react";
import { Waypoints } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { fetchArms, upsertArm, type ArmRow } from "@/lib/store";

type ListState =
  | { status: "loading" }
  | { status: "ready"; arms: ArmRow[] }
  | { status: "error" };

const fieldClass =
  "h-8 w-full rounded-lg border border-input bg-background px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50";

export function ArmsList({ refreshToken }: { refreshToken: number }) {
  const [state, setState] = useState<ListState>({ status: "loading" });
  const [requestId, setRequestId] = useState(0);
  const [model, setModel] = useState("");
  const [providerKind, setProviderKind] = useState("official");
  const [host, setHost] = useState("");
  const [protocol, setProtocol] = useState("anthropic-messages");
  const [client, setClient] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState(false);

  const load = useCallback(() => {
    setState({ status: "loading" });
    setRequestId((id) => id + 1);
  }, []);

  useEffect(() => {
    let cancelled = false;
    fetchArms().then(
      (arms) => {
        if (!cancelled) {
          setState({ status: "ready", arms });
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
  }, [requestId, refreshToken]);

  async function onSubmit(event: FormEvent) {
    event.preventDefault();
    setSaving(true);
    setSaveError(false);
    try {
      await upsertArm({
        model,
        provider_kind: providerKind,
        host,
        protocol,
        client,
      });
      setModel("");
      setHost("");
      setClient("");
      load();
    } catch {
      setSaveError(true);
    } finally {
      setSaving(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <Waypoints aria-hidden className="size-4" />
          臂
        </CardTitle>
        <CardDescription>存在这台电脑上。这里不保存 API key，也不计算分数。</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <form className="grid gap-3 sm:grid-cols-2" onSubmit={onSubmit}>
          <label className="flex flex-col gap-1 text-sm">
            模型
            <input
              className={fieldClass}
              value={model}
              onChange={(event) => setModel(event.target.value)}
              placeholder="example-model"
              autoComplete="off"
              required
            />
          </label>
          <label className="flex flex-col gap-1 text-sm">
            提供方
            <select
              className={fieldClass}
              value={providerKind}
              onChange={(event) => setProviderKind(event.target.value)}
            >
              <option value="official">官方直连</option>
              <option value="relay">中转</option>
              <option value="subscription">订阅</option>
            </select>
          </label>
          <label className="flex flex-col gap-1 text-sm">
            主机
            <input
              className={fieldClass}
              value={host}
              onChange={(event) => setHost(event.target.value)}
              placeholder="api.example.test"
              autoComplete="off"
              required
            />
          </label>
          <label className="flex flex-col gap-1 text-sm">
            协议
            <select
              className={fieldClass}
              value={protocol}
              onChange={(event) => setProtocol(event.target.value)}
            >
              <option value="anthropic-messages">anthropic-messages</option>
              <option value="openai-chat-completions">openai-chat-completions</option>
              <option value="openai-responses">openai-responses</option>
            </select>
          </label>
          <label className="flex flex-col gap-1 text-sm">
            客户端
            <input
              className={fieldClass}
              value={client}
              onChange={(event) => setClient(event.target.value)}
              placeholder="desktop"
              autoComplete="off"
              required
            />
          </label>
          <div className="flex items-end">
            <Button type="submit" disabled={saving}>
              添加
            </Button>
          </div>
        </form>
        {saveError ? <p className="text-sm text-destructive">未能保存</p> : null}
        {state.status === "loading" ? <p className="text-sm text-muted-foreground">读取中</p> : null}
        {state.status === "error" ? (
          <div className="flex flex-col items-start gap-3">
            <p className="text-sm text-destructive">未能读取臂</p>
            <Button type="button" variant="outline" onClick={load}>
              重试
            </Button>
          </div>
        ) : null}
        {state.status === "ready" && state.arms.length === 0 ? (
          <p className="text-sm text-muted-foreground">还没有臂</p>
        ) : null}
        {state.status === "ready" && state.arms.length > 0 ? (
          <ul className="flex flex-col gap-2">
            {state.arms.map((arm) => (
              <li key={arm.id} className="font-mono text-sm break-all">
                {arm.model} · {arm.provider_kind} · {arm.host} · {arm.protocol} · {arm.client}
              </li>
            ))}
          </ul>
        ) : null}
      </CardContent>
    </Card>
  );
}

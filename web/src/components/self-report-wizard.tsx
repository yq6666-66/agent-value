import { useState } from "react";
import { ListChecks } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  answerQuestion,
  fetchRegions,
  fetchReport,
  fetchWeightContext,
  formatMetric,
  formatWeights,
  saveReport,
  sourceLabel,
  startSession,
  type QuestionView,
  type ReportView,
  type RegionsView,
  type SaveView,
  type StepView,
  type WeightContext,
} from "@/lib/elicit";

const LEVELS: { level: number; label: string }[] = [
  { level: 2, label: "强烈偏好甲" },
  { level: 1, label: "稍偏甲" },
  { level: 0, label: "差不多" },
  { level: -1, label: "稍偏乙" },
  { level: -2, label: "强烈偏好乙" },
];

type Phase =
  | { status: "idle" }
  | { status: "question"; step: StepView }
  | {
      status: "result";
      step: StepView;
      regions: RegionsView;
      report: ReportView;
      context: WeightContext;
    }
  | { status: "saved"; save: SaveView }
  | { status: "error" };

function OptionColumn({
  title,
  cost,
  time,
  defect,
}: {
  title: string;
  cost: number;
  time: number;
  defect: number;
}) {
  return (
    <div className="flex flex-col gap-1 rounded-lg bg-muted/40 p-3">
      <p className="font-medium">{title}</p>
      <p>成本 {formatMetric(cost)} 元</p>
      <p>时间 {formatMetric(time)} 秒</p>
      <p>缺陷率 {formatMetric(defect)}</p>
    </div>
  );
}

function QuestionCard({
  question,
  step,
  pending,
  error,
  onAnswer,
}: {
  question: QuestionView;
  step: StepView;
  pending: boolean;
  error: boolean;
  onAnswer: (level: number) => void;
}) {
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">
        已答 {step.answered} 题 · 宽区域 {step.wide_len} 个点 · 题号 {question.index}
      </p>
      {step.skipped.length > 0 ? (
        <p className="text-sm text-muted-foreground">已跳过 {step.skipped.join("、")}</p>
      ) : null}
      <p className="text-sm leading-6">
        甲、乙两边的数字由核心库给出。选更接近你取舍的一档。
      </p>
      <div className="grid grid-cols-2 gap-3">
        <OptionColumn
          title="甲"
          cost={question.shown.cost[0]}
          time={question.shown.time[0]}
          defect={question.shown.defect[0]}
        />
        <OptionColumn
          title="乙"
          cost={question.shown.cost[1]}
          time={question.shown.time[1]}
          defect={question.shown.defect[1]}
        />
      </div>
      <p className="text-sm text-muted-foreground">
        比值 成本 {formatMetric(question.rho[0])} · 时间 {formatMetric(question.rho[1])} · 缺陷{" "}
        {formatMetric(question.rho[2])}
      </p>
      <div className="flex flex-wrap gap-2">
        {LEVELS.map((level) => (
          <Button
            key={level.level}
            type="button"
            variant="outline"
            disabled={pending}
            onClick={() => onAnswer(level.level)}
          >
            {level.label}
          </Button>
        ))}
      </div>
      {error ? <p className="text-sm text-destructive">未能记下这一档</p> : null}
    </div>
  );
}

export function SelfReportWizard() {
  const [phase, setPhase] = useState<Phase>({ status: "idle" });
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);

  async function showResult(step: StepView) {
    const [regions, report, context] = await Promise.all([
      fetchRegions(),
      fetchReport(),
      fetchWeightContext(),
    ]);
    setPhase({ status: "result", step, regions, report, context });
  }

  function applyStep(step: StepView) {
    if (step.finished) {
      return showResult(step);
    }
    if (step.question === null) {
      setPhase({ status: "error" });
      return undefined;
    }
    setPhase({ status: "question", step });
    return undefined;
  }

  async function onStart() {
    setPending(true);
    setError(false);
    try {
      await applyStep(await startSession(null));
    } catch {
      setPhase({ status: "error" });
    } finally {
      setPending(false);
    }
  }

  async function onAnswer(level: number) {
    if (phase.status !== "question" || phase.step.question === null) {
      return;
    }
    setPending(true);
    setError(false);
    try {
      await applyStep(
        await answerQuestion({ question: phase.step.question.index, level }),
      );
    } catch {
      setError(true);
    } finally {
      setPending(false);
    }
  }

  async function onSave() {
    setPending(true);
    setError(false);
    try {
      setPhase({ status: "saved", save: await saveReport() });
    } catch {
      setError(true);
    } finally {
      setPending(false);
    }
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex items-center gap-2">
          <ListChecks aria-hidden className="size-4" />
          五档自报
        </CardTitle>
        <CardDescription>题面写在这里。下一题、区域和重心从核心库读取。</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {phase.status === "idle" ? (
          <Button type="button" onClick={onStart} disabled={pending}>
            开始答题
          </Button>
        ) : null}
        {phase.status === "question" && phase.step.question !== null ? (
          <QuestionCard
            question={phase.step.question}
            step={phase.step}
            pending={pending}
            error={error}
            onAnswer={onAnswer}
          />
        ) : null}
        {phase.status === "result" ? (
          <div className="flex flex-col gap-3">
            <p>已答 {phase.step.answered} 题</p>
            {phase.regions.point ? (
              <p>重心 {formatWeights(phase.regions.point)}</p>
            ) : (
              <p>核心库没有返回重心</p>
            )}
            <p>
              核心区域 {phase.regions.core.length} 个点，宽区域 {phase.regions.wide.length} 个点，偏差{" "}
              {phase.regions.m}
            </p>
            <p className="text-sm text-muted-foreground">
              宽区域 成本 {formatMetric(phase.report.ranges[0][0])}–{formatMetric(phase.report.ranges[0][1])}
              {" · "}时间 {formatMetric(phase.report.ranges[1][0])}–{formatMetric(phase.report.ranges[1][1])}
              {" · "}质量 {formatMetric(phase.report.ranges[2][0])}–{formatMetric(phase.report.ranges[2][1])}
            </p>
            {phase.report.contradiction ? (
              <p>
                回答互相矛盾
                {phase.report.contradicted.length > 0
                  ? `：题号 ${phase.report.contradicted.join("、")}`
                  : ""}
              </p>
            ) : null}
            {phase.context.in_window ? (
              <p className="text-sm leading-6">
                本窗已开始。保存只会生成试算，不会改本窗正式权重。
              </p>
            ) : (
              <p className="text-sm leading-6">尚未开窗。保存会写成正式自报，从下一未冻结窗起生效。</p>
            )}
            <div className="flex flex-wrap gap-2">
              <Button type="button" onClick={onSave} disabled={pending}>
                保存自报
              </Button>
              <Button type="button" variant="outline" onClick={onStart} disabled={pending}>
                重新开始
              </Button>
            </div>
            {error ? <p className="text-sm text-destructive">未能保存</p> : null}
          </div>
        ) : null}
        {phase.status === "saved" && phase.save.outcome === "trial" ? (
          <div className="flex flex-col gap-2">
            <p>{phase.save.label}</p>
            <p>
              本窗正式 {formatWeights(phase.save.official)} · 来源 {sourceLabel(phase.save.official.source)}
            </p>
            <p>
              试算 {formatWeights(phase.save.applied)} · 来源 {sourceLabel(phase.save.applied.source)}
            </p>
            <p>本窗正式权重未改。</p>
            <Button type="button" variant="outline" onClick={onStart} disabled={pending}>
              重新开始
            </Button>
          </div>
        ) : null}
        {phase.status === "saved" && phase.save.outcome === "official" ? (
          <div className="flex flex-col gap-2">
            <p>已写入正式自报，从下一未冻结窗起生效。</p>
            <p>
              正式 {formatWeights(phase.save.applied)} · 来源 {sourceLabel(phase.save.applied.source)}
            </p>
            <Button type="button" variant="outline" onClick={onStart} disabled={pending}>
              重新开始
            </Button>
          </div>
        ) : null}
        {phase.status === "error" ? (
          <div className="flex flex-col items-start gap-3">
            <p className="text-sm text-destructive">未能读取自报</p>
            <Button type="button" variant="outline" onClick={onStart}>
              重试
            </Button>
          </div>
        ) : null}
      </CardContent>
    </Card>
  );
}

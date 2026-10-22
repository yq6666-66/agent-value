import { ChartNoAxesCombined } from "lucide-react";

// Structural placeholder. Scores, when they exist, come from the shell.
export function FrontierPane() {
  return (
    <section className="flex h-full min-h-0 flex-col gap-3 overflow-auto p-4" aria-label="前沿">
      <h2 className="flex items-center gap-2 text-base font-medium">
        <ChartNoAxesCombined aria-hidden className="size-4" />
        前沿
      </h2>
      <p className="text-sm leading-6 text-muted-foreground">
        主区域。还没有前沿数据。这里不计算分数。
      </p>
      <div className="grid flex-1 grid-rows-[auto_1fr] overflow-hidden rounded-lg ring-1 ring-foreground/10">
        <div className="grid grid-cols-3 border-b px-3 py-2 text-xs text-muted-foreground">
          <span>臂</span>
          <span>成本</span>
          <span>时间</span>
        </div>
        <p className="px-3 py-6 text-sm text-muted-foreground">空</p>
      </div>
    </section>
  );
}

import type { ReactNode } from "react";
import { Group, Panel, Separator } from "react-resizable-panels";

import { FrontierPane } from "@/components/frontier-pane";
import { WeightPanel } from "@/components/weight-panel";

export function ShellLayout({
  children,
  refreshToken = 0,
}: {
  children: ReactNode;
  refreshToken?: number;
}) {
  return (
    <Group orientation="vertical" className="min-h-0 flex-1" id="shell">
      <Panel id="primary" defaultSize="62%" minSize="28%">
        <Group orientation="horizontal" className="h-full" id="primary-split">
          <Panel id="frontier" defaultSize="68%" minSize="30%">
            <FrontierPane />
          </Panel>
          <Separator id="primary-separator" className="w-1.5 bg-border" />
          <Panel id="weights" defaultSize="32%" minSize="22%">
            <WeightPanel refreshToken={refreshToken} />
          </Panel>
        </Group>
      </Panel>
      <Separator id="shell-separator" className="h-1.5 bg-border" />
      <Panel id="records" defaultSize="38%" minSize="16%" className="overflow-auto">
        {children}
      </Panel>
    </Group>
  );
}

import { useState } from "react";

import { AboutStatus } from "@/components/about-status";
import { ArmsList } from "@/components/arms-list";
import { DemoFixture } from "@/components/demo-fixture";
import { SettingsPanel } from "@/components/settings-panel";
import { ShellLayout } from "@/components/shell-layout";

// Display only. Versions, the demo anchor, stored arms, and window chrome
// come from the shell. This file does not compute scores.
export function App() {
  const [refreshToken, setRefreshToken] = useState(0);

  return (
    <div className="flex h-svh flex-col bg-background text-foreground">
      <header className="shrink-0 border-b">
        <div className="flex items-baseline justify-between gap-4 px-4 py-3">
          <h1 className="text-lg font-medium tracking-tight">agent-value</h1>
          <p className="text-sm text-muted-foreground">本地桌面工具</p>
        </div>
      </header>
      <ShellLayout>
        <div className="flex flex-col gap-6 p-6">
          <ArmsList refreshToken={refreshToken} />
          <SettingsPanel onWiped={() => setRefreshToken((token) => token + 1)} />
          <DemoFixture />
          <AboutStatus />
        </div>
      </ShellLayout>
    </div>
  );
}

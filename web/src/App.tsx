import { useState } from "react";

import { AboutStatus } from "@/components/about-status";
import { ArmsList } from "@/components/arms-list";
import { DemoFixture } from "@/components/demo-fixture";
import { SettingsPanel } from "@/components/settings-panel";

// Display only. Versions, the demo anchor, and stored arms come from the
// shell. This file does not compute scores.
export function App() {
  const [refreshToken, setRefreshToken] = useState(0);

  return (
    <div className="flex min-h-svh flex-col bg-background text-foreground">
      <header className="border-b">
        <div className="mx-auto flex w-full max-w-3xl items-baseline justify-between gap-4 px-6 py-4">
          <h1 className="text-lg font-medium tracking-tight">agent-value</h1>
          <p className="text-sm text-muted-foreground">本地桌面工具</p>
        </div>
      </header>
      <main className="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-6 py-8">
        <ArmsList refreshToken={refreshToken} />
        <SettingsPanel onWiped={() => setRefreshToken((token) => token + 1)} />
        <DemoFixture />
        <AboutStatus />
      </main>
    </div>
  );
}

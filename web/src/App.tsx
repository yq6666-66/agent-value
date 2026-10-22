import { AboutStatus } from "@/components/about-status";
import { DemoFixture } from "@/components/demo-fixture";

// Display only. Versions and scores come from agent-value-core through Tauri
// commands. This file does not compute them.
export function App() {
  return (
    <div className="flex min-h-svh flex-col bg-background text-foreground">
      <header className="border-b">
        <div className="mx-auto flex w-full max-w-3xl items-baseline justify-between gap-4 px-6 py-4">
          <h1 className="text-lg font-medium tracking-tight">agent-value</h1>
          <p className="text-sm text-muted-foreground">本地桌面工具</p>
        </div>
      </header>
      <main className="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-6 px-6 py-8">
        <section className="flex flex-col gap-2">
          <h2 className="text-base font-medium">占位界面</h2>
          <p className="text-sm leading-6 text-muted-foreground">
            还没有接入你的任务。界面只展示本地核心给出的结果，不在这里计算分数。
          </p>
        </section>
        <DemoFixture />
        <AboutStatus />
      </main>
    </div>
  );
}

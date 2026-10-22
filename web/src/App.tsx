import { AboutStatus } from "@/components/about-status";

// Display only. Every string shown as a version comes from agent-value-core
// through the Tauri `core_version` command. This file does not compute one.
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
            还没有开始评分。界面只展示本地核心给出的结果，不在这里计算分数。
          </p>
        </section>
        <AboutStatus />
      </main>
    </div>
  );
}

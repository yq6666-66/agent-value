// Display only. The web UI never computes scores; every number it will show
// comes from agent-value-core through the desktop shell.
export function App() {
  return (
    <main style={{ fontFamily: "system-ui, sans-serif", padding: "2rem" }}>
      <h1>agent-value</h1>
      <p>Skeleton only. Nothing is scored yet.</p>
    </main>
  );
}

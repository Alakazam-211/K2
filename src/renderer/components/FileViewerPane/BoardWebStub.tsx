// A Board file on the hosted web client: it has no Zen and fetches no widget bundles, so it says so instead of drawing a
// broken pane (prd-board-tab-v1 §8). It reads and writes nothing.

export function BoardWebStub(): React.JSX.Element {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 text-xs text-[var(--color-text-muted)]" data-board-web-stub="">
      <div className="text-[var(--color-text-primary)]">Board needs the desktop app</div>
      <div>Open this file in K2 on your computer. Agents can still work on it.</div>
      <a href="https://k2.dev/download" target="_blank" rel="noreferrer" className="flex h-5 items-center border border-[var(--color-border)] px-2 text-[var(--color-text-primary)]">
        Download K2
      </a>
    </div>
  )
}

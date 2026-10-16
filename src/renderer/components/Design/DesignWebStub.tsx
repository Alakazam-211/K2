// A Design tab on the hosted web client (D-A15): the web client has no
// Browser pane, so a Design tab from the shared layout says so instead of
// drawing a broken pane, and points at the room's Thread.

export interface DesignWebStubProps {
  /** Opens the room's Thread (the web client's own navigation). */
  onOpenThread?: () => void
}

export function DesignWebStub({ onOpenThread }: DesignWebStubProps): React.JSX.Element {
  return (
    <div className="flex flex-col items-center justify-center gap-2 h-full text-xs text-[var(--color-text-muted)]">
      <div className="text-[var(--color-text-primary)]">Design needs the desktop app</div>
      <div>Notes sent from Design show up in this workspace's Thread.</div>
      {onOpenThread ? (
        <button
          type="button"
          onClick={onOpenThread}
          className="px-2 h-5 border border-[var(--color-border)] cursor-pointer"
        >
          Open the Thread
        </button>
      ) : null}
    </div>
  )
}

// One federated (`agent::host`) connection row in Settings → Workspaces →
// Connections. A row whose workspace left its server (0159,
// `remoteWorkspaceGone`) is muted and says so: handles are never reused, so
// sends through it fail until an owner or admin re-adds it, which binds it
// to the workspace that has that name now.

import type { RemoteConnectionEntry } from '@/lib/federation'
import { remoteConnectionGoneNote } from '@/lib/federation-gone'

export function FederatedConnectionRow({
  entry,
  canReAdd,
  busy,
  onRemove,
  onReAdd,
}: {
  entry: RemoteConnectionEntry
  /** Owner or admin (the daemon refuses anyone else). */
  canReAdd: boolean
  /** A re-add of this row is in flight. */
  busy: boolean
  onRemove(): void
  onReAdd(): void
}): React.JSX.Element {
  const gone = entry.gone
  return (
    <div
      data-federated-connection={entry.address}
      data-gone={gone ? 'true' : undefined}
      className="flex items-center gap-2 px-3 py-1.5 border-b border-[var(--color-border)] last:border-b-0"
    >
      <span
        className={`w-2 h-2 flex-shrink-0 rounded-full ${gone ? 'bg-[var(--color-neutral)]' : 'bg-[var(--color-accent)]'}`}
      />
      <span className={`flex-1 min-w-0 ${gone ? 'opacity-60' : ''}`}>
        <span
          className={`block text-xs truncate ${gone ? 'text-[var(--color-text-muted)] line-through' : 'text-[var(--color-text-primary)]'}`}
          title={entry.address}
        >
          {entry.address}
        </span>
        {gone && (
          <span className="block text-[10px] text-[var(--color-text-muted)] leading-relaxed">
            {remoteConnectionGoneNote(entry.host)}
            {!canReAdd && ' Only an owner or admin can re-add it.'}
          </span>
        )}
      </span>
      <span className="text-[9px] text-[var(--color-text-muted)] uppercase tracking-wider">
        {gone ? 'gone' : 'federated'}
      </span>
      {gone && canReAdd && (
        <button
          type="button"
          onClick={onReAdd}
          disabled={busy}
          className="px-1.5 py-0.5 text-[10px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer disabled:opacity-50 flex-shrink-0"
          title="Connect to the workspace that has this name on that server now"
        >
          {busy ? 'Re-adding…' : 'Re-add'}
        </button>
      )}
      <button
        type="button"
        onClick={onRemove}
        className="w-5 h-5 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] transition-colors no-drag cursor-pointer flex-shrink-0"
        title="Remove federated connection"
      >
        <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth="1.5">
          <line x1="1" y1="1" x2="7" y2="7" />
          <line x1="7" y1="1" x2="1" y2="7" />
        </svg>
      </button>
    </div>
  )
}

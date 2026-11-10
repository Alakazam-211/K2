// One federated (`agent::host`) connection row in Settings → Workspaces →
// Connections. A row whose workspace left its server (0159,
// `remoteWorkspaceGone`) is muted and says so: handles are never reused, so
// sends through it fail until an owner or admin re-adds it, which binds it
// to the workspace that has that name now.

import type { RemoteConnectionEntry } from '@/lib/federation'
import { remoteConnectionGoneNote } from '@/lib/federation-gone'
import { Button } from '@/components/ui'

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
      className="flex items-center gap-3 px-4 py-2.5"
    >
      <span
        className={`w-2.5 h-2.5 flex-shrink-0 rounded-[var(--radius-badge)] ${gone ? 'bg-[var(--color-neutral)]' : 'bg-[var(--color-accent)]'}`}
      />
      <span className={`flex-1 min-w-0 ${gone ? 'opacity-60' : ''}`}>
        <span
          className={`block type-row truncate ${gone ? 'text-[var(--color-text-secondary)] line-through' : 'text-[var(--color-text-primary)]'}`}
          title={entry.address}
        >
          {entry.address}
        </span>
        {gone && (
          <span className="block type-hint text-[var(--color-text-secondary)] leading-relaxed">
            {remoteConnectionGoneNote(entry.host)}
            {!canReAdd && ' Only an owner or admin can re-add it.'}
          </span>
        )}
      </span>
      <span className="type-section text-[var(--color-text-secondary)] uppercase tracking-wider">
        {gone ? 'gone' : 'paired'}
      </span>
      {gone && canReAdd && (
        <Button
          variant="quiet"
          onClick={onReAdd}
          disabled={busy}
          className="flex-shrink-0"
          title="Connect to the workspace that has this name on that server now"
        >
          {busy ? 'Re-adding…' : 'Re-add'}
        </Button>
      )}
      <Button variant="quiet" onClick={onRemove} className="flex-shrink-0" title="Remove connection">
        Remove
      </Button>
    </div>
  )
}

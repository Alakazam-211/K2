// "was Viewer" row notice + the explicit "Enable as Member" action
// (prd-remove-viewer-role-v1.md RV16). Rendered under a user row whose
// GET /cli/users entry has `wasViewer: true` and is still disabled.

import { WAS_VIEWER_NOTE } from './connect-user-roles'

export function WasViewerNotice({
  username,
  busy,
  onEnableAsMember,
}: {
  username: string
  busy: boolean
  onEnableAsMember: (username: string) => void
}): React.JSX.Element {
  return (
    <div
      className="flex items-center justify-between gap-3 px-2 py-1.5 border border-[var(--color-border)] bg-[var(--color-bg-elevated)]"
      data-testid={`was-viewer-${username}`}
    >
      <span className="text-[10px] text-[var(--color-text-secondary)]">{WAS_VIEWER_NOTE}</span>
      <button
        type="button"
        disabled={busy}
        onClick={() => onEnableAsMember(username)}
        aria-label={`Enable ${username} as Member`}
        className="flex-shrink-0 px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60"
      >
        {busy ? 'Enabling…' : 'Enable as Member'}
      </button>
    </div>
  )
}

/** Small badge next to the username. */
export function WasViewerBadge(): React.JSX.Element {
  return (
    <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-text-muted)]/15 text-[var(--color-text-muted)]">
      was Viewer
    </span>
  )
}

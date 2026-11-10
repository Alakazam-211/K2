// "was Viewer" row notice + the explicit "Enable as Member" action
// (prd-remove-viewer-role-v1.md RV16). Rendered under a user row whose
// GET /cli/users entry has `wasViewer: true` and is still disabled.

import { WAS_VIEWER_NOTE } from './connect-user-roles'
import { Tag } from '@/components/ui/Tag'
import { Button } from '@/components/ui/Button'

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
      className="mt-2 flex items-center justify-between gap-4"
      data-testid={`was-viewer-${username}`}
    >
      <span className="type-hint text-[var(--color-text-secondary)] leading-relaxed">{WAS_VIEWER_NOTE}</span>
      <Button
        variant="accent"
        className="flex-shrink-0"
        disabled={busy}
        onClick={() => onEnableAsMember(username)}
        aria-label={`Enable ${username} as Member`}
      >
        {busy ? 'Enabling…' : 'Enable as Member'}
      </Button>
    </div>
  )
}

/** Small badge next to the username. */
export function WasViewerBadge(): React.JSX.Element {
  return (
    <Tag tone="muted">
      was Viewer
    </Tag>
  )
}

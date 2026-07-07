// Presence S4 — the per-row "Edit" grant toggle in the presence modal.
//
// Shown ONLY on viewer-role roster rows, ONLY when the person looking at
// the modal can manage users (owner or admin — the same gate as the
// daemon's `require_manage` on POST /cli/presence/grant). The component
// self-gates and renders null otherwise, so PresenceModal mounts it
// unconditionally in every row's actions area.
//
// Actor-role resolution duplicates K2ConnectSection's `refreshWhoami`
// approach on purpose (sibling-slice isolation — S3's kick button does
// the same): one `GET /cli/auth/whoami` per modal open, deduped across
// the rows via a module-level in-flight promise.
//
// The switch REFLECTS the roster's `grantedEdit` (live — the daemon
// re-broadcasts the roster on every grant change, including the
// last-disconnect auto-revoke). Toggling optimistically flips a local
// pending value, POSTs `presence/grant`, and on failure reverts to the
// roster truth + surfaces the daemon's message inline.

import { useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { RosterUser } from '@/stores/presence'

// One whoami per modal-open: the rows all mount together and share the
// in-flight promise; it clears after settling so the NEXT open re-checks
// (the viewer's own role can change between opens).
let whoamiInflight: Promise<boolean> | null = null

async function actorCanManage(): Promise<boolean> {
  if (!whoamiInflight) {
    whoamiInflight = daemonCliGet<{ owner?: boolean; role?: string }>('auth/whoami')
      .then((d) => d.owner === true || d.role === 'owner' || d.role === 'admin')
      .catch(() => false)
      .finally(() => {
        whoamiInflight = null
      })
  }
  return whoamiInflight
}

interface PresenceGrantToggleProps {
  user: RosterUser
}

export default function PresenceGrantToggle({
  user,
}: PresenceGrantToggleProps): React.JSX.Element | null {
  const [canManage, setCanManage] = useState(false)
  // Optimistic value while a POST is in flight (null = mirror the roster).
  const [pending, setPending] = useState<boolean | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let alive = true
    void actorCanManage().then((ok) => {
      if (alive) setCanManage(ok)
    })
    return () => {
      alive = false
    }
  }, [])

  // The roster broadcast is the source of truth: once it catches up with
  // the optimistic value, drop the override and mirror the store again.
  useEffect(() => {
    if (pending !== null && user.grantedEdit === pending) setPending(null)
  }, [pending, user.grantedEdit])

  // Viewer-role rows only; only for a managing (owner/admin) onlooker.
  if (user.role !== 'viewer' || !canManage) return null

  const granted = pending ?? user.grantedEdit
  const busy = pending !== null

  const toggle = async (): Promise<void> => {
    const next = !granted
    setPending(next)
    setError(null)
    try {
      await daemonCliPost('presence/grant', { username: user.user, granted: next })
      // Keep the optimistic value; the presence_changed broadcast (or the
      // catch-up effect above) reconciles it away.
    } catch (err) {
      // Revert to the roster truth + surface the daemon's message.
      setPending(null)
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  return (
    <label
      className="flex items-center gap-1.5 cursor-pointer select-none no-drag"
      title={error ?? (granted ? 'Revoke edit access' : 'Grant edit access')}
    >
      {error && (
        <span
          className="max-w-[120px] truncate text-[9px] text-[var(--color-status-error-soft)]"
          title={error}
        >
          {error}
        </span>
      )}
      <span className="text-[10px] text-[var(--color-text-secondary)]">Edit</span>
      <button
        role="switch"
        aria-checked={granted}
        aria-label={`Edit access for ${user.user}`}
        disabled={busy}
        onClick={() => void toggle()}
        className="relative flex-shrink-0 transition-colors no-drag cursor-pointer disabled:cursor-wait"
        style={{
          width: 26,
          height: 14,
          borderRadius: 7,
          border: '1px solid var(--color-border)',
          background: granted ? 'var(--color-accent)' : 'var(--color-bg-elevated)',
          opacity: busy ? 0.6 : 1,
        }}
      >
        <span
          aria-hidden="true"
          style={{
            position: 'absolute',
            top: 1,
            left: granted ? 13 : 1,
            width: 10,
            height: 10,
            borderRadius: '50%',
            background: granted ? 'white' : 'var(--color-text-muted)',
            transition: 'left 120ms ease',
          }}
        />
      </button>
    </label>
  )
}

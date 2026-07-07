// Presence S3 — the per-row Kick button for the presence modal
// (presence/multiplayer arc, PRD §5.2). Own file by design: the modal
// mounts exactly one <PresenceKickButton user={u} /> in each row's
// actions area, and everything else (viewer-role resolution, the kick
// matrix, the inline confirm, the POST) lives here so sibling slices
// can slot their own controls next to it without touching this code.
//
// Visibility mirrors the daemon's kick matrix (PRD §4, enforced
// server-side in `presence.rs::handle_kick` — the UI is advisory):
//   - owner  → any non-owner row;
//   - admin  → member/viewer rows only (NOT fellow admins, NOT owner);
//   - member/viewer/unknown → nothing.
//
// The VIEWER's role comes from `GET /cli/auth/whoami` (the
// K2ConnectSection `refreshWhoami` approach — S2's presence store
// doesn't carry it). It's resolved once per modal open, NOT in a render
// path: a module-scope single-flight cache means the N row-buttons that
// mount together share ONE fetch, a short TTL keeps a reopened modal
// fresh, and a host switch invalidates immediately (per-daemon state,
// the presence-store convention).
//
// Clicking swaps the button for an INLINE confirm (the app's
// remove-a-user convention — red confirm + muted cancel, in place; the
// surrounding modal already mirrors ConfirmDialog's overlay/Escape
// conventions). Confirming POSTs `presence/kick {username}`; on success
// nothing is touched locally — the kicked sockets deregister and the
// roster updates itself via `presence_changed` events. A daemon error
// surfaces inline, verbatim.

import { useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { onActiveHostChange } from '@/stores/connect-host'
import type { RosterUser } from '@/stores/presence'

/** Wire roles the kick matrix reasons about. `viewer` ships with S4 but
 *  is gated here already so the matrix doesn't need a second pass. */
type WireRole = 'owner' | 'admin' | 'member' | 'viewer'

/** The kick matrix, client-side mirror of `handle_kick` (PRD §4):
 *  owner kicks any non-owner; admin kicks member/viewer only. Pure —
 *  unit-tested directly. */
export function canKick(viewerRole: string | null, targetRole: string): boolean {
  if (targetRole === 'owner') return false
  if (viewerRole === 'owner') return true
  if (viewerRole === 'admin') return targetRole === 'member' || targetRole === 'viewer'
  return false
}

// ── Viewer-role resolution (module-scope single-flight + TTL) ─────────────
//
// All row buttons mount together when the modal opens; the first one
// starts the whoami fetch and the rest await the SAME promise. The TTL
// (10s) makes "once per modal open" hold in practice without a render-path
// fetch or a prop drilled through PresenceModal.

const WHOAMI_TTL_MS = 10_000

let whoamiCache: { at: number; promise: Promise<WireRole | null> } | null = null

export function fetchViewerRole(): Promise<WireRole | null> {
  const now = Date.now()
  if (!whoamiCache || now - whoamiCache.at > WHOAMI_TTL_MS) {
    whoamiCache = {
      at: now,
      promise: daemonCliGet<{ role?: string; owner?: boolean }>('auth/whoami')
        .then((data): WireRole | null => {
          if (data.role === 'owner' || data.role === 'admin' || data.role === 'member' || data.role === 'viewer') {
            return data.role
          }
          // Pre-#629 daemons return no role; `owner:true` still means owner.
          return data.owner ? 'owner' : null
        })
        .catch(() => null),
    }
  }
  return whoamiCache.promise
}

// Per-daemon identity — a host switch invalidates the cached role
// (presence-store convention, #625 seam).
onActiveHostChange(() => {
  whoamiCache = null
})

/** Test-only: reset the module cache between vitest cases. */
export function __resetWhoamiCacheForTests(): void {
  whoamiCache = null
}

interface PresenceKickButtonProps {
  user: RosterUser
}

export default function PresenceKickButton({ user }: PresenceKickButtonProps): React.JSX.Element | null {
  const [viewerRole, setViewerRole] = useState<WireRole | null>(null)
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void fetchViewerRole().then((role) => {
      if (!cancelled) setViewerRole(role)
    })
    return () => {
      cancelled = true
    }
  }, [])

  if (!canKick(viewerRole, user.role)) return null

  const kick = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await daemonCliPost('presence/kick', { username: user.user })
      // Success: nothing to do — the kicked sockets deregister and the
      // roster (this row included) updates via presence_changed events.
      setConfirming(false)
    } catch (e) {
      // Surface the daemon's message verbatim (daemonCliPost already
      // unwraps the {"error":"..."} body).
      setError(e instanceof Error ? e.message : String(e))
      setConfirming(false)
    } finally {
      setBusy(false)
    }
  }

  if (error) {
    return (
      <span className="flex items-center gap-1.5">
        <span className="max-w-[140px] truncate text-[10px] text-[var(--color-status-error-soft)]" title={error}>
          {error}
        </span>
        <button
          onClick={() => setError(null)}
          className="text-[10px] text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer"
        >
          Dismiss
        </button>
      </span>
    )
  }

  if (confirming) {
    // Inline confirm — the users-list "Confirm remove / Cancel" pattern.
    return (
      <span className="flex items-center gap-1.5">
        <button
          onClick={() => void kick()}
          disabled={busy}
          className="text-[10px] text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer disabled:opacity-60"
        >
          {busy ? 'Kicking…' : 'Confirm kick'}
        </button>
        <button
          onClick={() => setConfirming(false)}
          disabled={busy}
          className="text-[10px] text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer disabled:opacity-60"
        >
          Cancel
        </button>
      </span>
    )
  }

  return (
    <button
      onClick={() => setConfirming(true)}
      title={`Disconnect ${user.user} and revoke their sessions`}
      className="px-1.5 py-px text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:border-[#c53030] hover:text-[#c53030] transition-colors no-drag cursor-pointer"
    >
      Kick
    </button>
  )
}

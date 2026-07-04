// Presence S2 — the "who's connected" modal, opened from the top-bar
// roster. READ-ONLY this slice: it lists every roster row (avatar, name,
// role chip, window count, workspaces, connected-since) and live-updates
// as `presence_changed` events replace the store's roster.
//
// Each row ends in a right-aligned ACTIONS area that is intentionally
// empty for now — S3 slots the Kick button and S4 the edit-grant toggle
// into it without any relayout.
//
// Overlay/escape/close conventions mirror ConfirmDialog (backdrop
// mousedown closes, Escape closes via a capture-phase window listener,
// fixed-centered surface on the same z-band).

import { useEffect } from 'react'
import { usePresenceStore, type RosterUser } from '@/stores/presence'
import PresenceAvatar, { ROLE_COLORS, presenceDisplayName } from './PresenceAvatar'
import PresenceKickButton from './PresenceKickButton'
import PresenceGrantToggle from './PresenceGrantToggle'

interface PresenceModalProps {
  onClose: () => void
}

/** Compact relative time for "connected since" (unix seconds). */
export function formatConnectedSince(connectedAtSecs: number, nowMs: number = Date.now()): string {
  const deltaSecs = Math.max(0, Math.floor(nowMs / 1000) - connectedAtSecs)
  if (deltaSecs < 60) return 'just now'
  if (deltaSecs < 3600) return `${Math.floor(deltaSecs / 60)}m ago`
  if (deltaSecs < 86400) return `${Math.floor(deltaSecs / 3600)}h ago`
  return `${Math.floor(deltaSecs / 86400)}d ago`
}

function basename(path: string): string {
  const parts = path.split('/').filter(Boolean)
  return parts[parts.length - 1] ?? path
}

export default function PresenceModal({ onClose }: PresenceModalProps): React.JSX.Element {
  // Live subscription — a presence_changed whole-set replace re-renders
  // the open modal (rows appear/disappear as users connect/disconnect).
  const roster = usePresenceStore((s) => s.roster)

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', handleKeyDown, true)
    return () => window.removeEventListener('keydown', handleKeyDown, true)
  }, [onClose])

  return (
    <>
      {/* Semi-transparent backdrop */}
      <div
        style={{
          position: 'fixed',
          inset: 0,
          zIndex: 99998,
          background: 'rgba(0, 0, 0, 0.5)',
        }}
        onMouseDown={(e) => {
          e.stopPropagation()
          onClose()
        }}
      />

      {/* Dialog */}
      <div
        className="no-drag"
        style={{
          position: 'fixed',
          top: '50%',
          left: '50%',
          transform: 'translate(-50%, -50%)',
          zIndex: 99999,
          width: 480,
          maxWidth: 'calc(100vw - 48px)',
          maxHeight: 'calc(100vh - 96px)',
          display: 'flex',
          flexDirection: 'column',
          background: 'var(--color-bg-surface)',
          border: '1px solid var(--color-border)',
          boxShadow: '0 8px 32px rgba(0, 0, 0, 0.6), 0 2px 8px rgba(0, 0, 0, 0.4)',
        }}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)]">
          <div className="text-sm font-semibold text-[var(--color-text-primary)]">
            Connected users
            <span className="ml-2 text-xs font-normal text-[var(--color-text-muted)]">
              {roster.length}
            </span>
          </div>
          <button
            onClick={onClose}
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
            title="Close (Esc)"
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <line x1="2" y1="2" x2="10" y2="10" />
              <line x1="10" y1="2" x2="2" y2="10" />
            </svg>
          </button>
        </div>

        {/* Rows */}
        <div className="flex-1 overflow-y-auto py-1">
          {roster.map((u) => (
            <PresenceRow key={u.user} user={u} />
          ))}
          {roster.length === 0 && (
            <div className="px-4 py-6 text-center text-xs text-[var(--color-text-muted)]">
              Nobody connected.
            </div>
          )}
        </div>
      </div>
    </>
  )
}

function PresenceRow({ user }: { user: RosterUser }): React.JSX.Element {
  const displayName = presenceDisplayName(user.user)
  const roleColor = ROLE_COLORS[user.role] ?? ROLE_COLORS.member

  return (
    <div className="flex items-center gap-3 px-4 py-2.5 hover:bg-[var(--color-bg-elevated)] transition-colors">
      <PresenceAvatar name={user.user} role={user.role} size={32} />

      {/* Identity + where they're working */}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="text-xs font-medium text-[var(--color-text-primary)] truncate">
            {displayName}
          </span>
          {/* Role chip, colored per ROLE_COLORS */}
          <span
            className="flex-shrink-0 px-1.5 py-px text-[9px] font-bold uppercase tracking-wide"
            style={{
              color: roleColor,
              border: `1px solid ${roleColor}`,
              borderRadius: 3,
            }}
          >
            {user.role}
          </span>
        </div>
        <div className="mt-0.5 flex items-center gap-1.5 text-[10px] text-[var(--color-text-muted)]">
          <span className="flex-shrink-0">
            {user.windowCount} {user.windowCount === 1 ? 'window' : 'windows'}
          </span>
          <span className="flex-shrink-0">·</span>
          <span className="flex-shrink-0">{formatConnectedSince(user.connectedAt)}</span>
          {user.workspaces.length > 0 && (
            <>
              <span className="flex-shrink-0">·</span>
              <span className="truncate">
                {user.workspaces.map((path, i) => (
                  <span key={path} title={path}>
                    {i > 0 && ', '}
                    {basename(path)}
                  </span>
                ))}
              </span>
            </>
          )}
        </div>
      </div>

      {/* Right-aligned actions area — each moderation control self-gates
          (grant toggle: viewer rows + managing onlooker only; kick:
          viewer's role may kick this row). */}
      <div className="flex flex-shrink-0 items-center gap-1.5">
        <PresenceGrantToggle user={user} />
        <PresenceKickButton user={user} />
      </div>
    </div>
  )
}

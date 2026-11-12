// Presence S2 — the "who's connected" modal, opened from the top-bar
// roster. READ-ONLY this slice: it lists every roster row (avatar, name,
// role chip, window count, workspaces, connected-since) and live-updates
// as `presence_changed` events replace the store's roster.
//
// Each row ends in a right-aligned ACTIONS area holding the S3 Kick
// button. (The S4 edit-grant toggle went with the Viewer role —
// prd-remove-viewer-role-v1.md.)
//
// Overlay/escape/close conventions mirror ConfirmDialog (Esc, the × and a
// press outside close it via DialogFrame onCancel, fixed-centered surface
// on the same z-band).

import { createPortal } from 'react-dom'
import { DialogCloseButton, DialogFrame, DialogScrim } from '@/components/ui'
import { usePresenceStore, type RosterUser } from '@/stores/presence'
import PresenceAvatar, { presenceDisplayName, useSelfPresenceUser } from './PresenceAvatar'
import PresenceKickButton from './PresenceKickButton'
import { personDisplayName, realName } from '@/lib/person-name'

interface PresenceModalProps {
  onClose: () => void
  /** 0.43.2 Z34: another server's roster (a focused Home room). Given, the
   *  modal lists it and shows no Kick: Kick acts on the window's server. */
  roster?: RosterUser[]
  /** That server's label, for the header. */
  serverLabel?: string | null
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

export default function PresenceModal({ onClose, roster: otherRoster, serverLabel }: PresenceModalProps): React.JSX.Element {
  // Live subscription — a presence_changed whole-set replace re-renders
  // the open modal (rows appear/disappear as users connect/disconnect).
  const windowRoster = usePresenceStore((s) => s.roster)
  const readOnly = otherRoster !== undefined
  const roster = otherRoster ?? windowRoster


  // A modal covers the window, so it renders at the top of <body>: inside
  // the top bar's frosted capsule (Glass), whose backdrop-filter makes it a
  // containing block and a stacking context, it was placed against the
  // capsule and painted under the tiles.
  return createPortal(
    <>
      {/* Semi-transparent backdrop */}
      <DialogScrim />

      {/* Dialog */}
      <DialogFrame
        onCancel={onClose}
        closeButton="inline"
        style={{
          width: 480,
          // In the window at any app zoom (globals.css --k2-vw / --k2-vh).
          maxWidth: 'calc(var(--k2-vw) - 48px)',
          maxHeight: 'calc(var(--k2-vh) - 96px)',
          display: 'flex',
          flexDirection: 'column',
        }}
      >
        {/* Header */}
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)]">
          <div className="text-sm font-semibold text-[var(--color-text-primary)]">
            {readOnly && serverLabel ? `Connected to ${serverLabel}` : 'Connected users'}
            <span className="ml-2 text-xs font-normal text-[var(--color-text-muted)]">
              {roster.length}
            </span>
          </div>
          <DialogCloseButton className="-mr-2" onClick={onClose} />
        </div>

        {/* Rows */}
        <div className="flex-1 overflow-y-auto py-1">
          {roster.map((u) => (
            <PresenceRow key={u.user} user={u} readOnly={readOnly} />
          ))}
          {roster.length === 0 && (
            <div className="px-4 py-6 text-center text-xs text-[var(--color-text-muted)]">
              Nobody connected.
            </div>
          )}
        </div>
      </DialogFrame>
    </>,
    document.body,
  )
}

function PresenceRow({ user, readOnly }: { user: RosterUser; readOnly: boolean }): React.JSX.Element {
  const displayName = personDisplayName(user.user, user)
  const real = realName(user)
  // The modal lists another server's roster read-only: "you" is only known
  // on the window's own server.
  const self = useSelfPresenceUser()
  const you = !readOnly && user.user === self

  return (
    <div className="flex items-center gap-3 px-4 py-2.5 hover:bg-[var(--color-bg-elevated)] transition-colors">
      <PresenceAvatar name={user.user} role={user.role} size={32} you={you} color={user.color} names={user} />

      {/* Identity + where they're working */}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="text-xs font-medium text-[var(--color-text-primary)] truncate">
            {displayName}
          </span>
          {real ? (
            <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]" data-testid="presence-row-username">
              {presenceDisplayName(user.user)}
            </span>
          ) : null}
          {you &&<span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">(you)</span>}
          {/* Role chip: neutral (role is the avatar's corner mark, never a
              colour; plan-agent-status-unify-v1 §3.5). */}
          <span
            className="flex-shrink-0 px-1.5 py-px text-[9px] font-bold uppercase tracking-wide text-[var(--color-text-secondary)] border border-[var(--color-border-strong)]"
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

      {/* Right-aligned actions area — the kick control self-gates (the
          onlooker's role may kick this row). Another server's roster has
          no actions (Z34). */}
      {readOnly ? null : (
        <div className="flex flex-shrink-0 items-center gap-1.5">
          <PresenceKickButton user={user} />
        </div>
      )}
    </div>
  )
}

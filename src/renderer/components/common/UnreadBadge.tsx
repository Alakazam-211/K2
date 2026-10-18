// Unread dot / count and the mute toggle (thread-read-v1, 0.46.1).
//
// Renders the daemon's per-person read state (`stores/unread.ts`) with the
// app's existing badge look (the page-tab badge pill, the old project
// unread dot):
//   - unread: an accent pill with the count ("99+" above 99), or a dot;
//   - muted:  the same, grey (still shown, left out of the totals);
//   - waiting (choice / secret cards waiting on you): an amber marker in
//     the needs-you colour, separate from unread. Reading never clears it.

import { useState } from 'react'
import { BellIcon, BellOffIcon } from '@/components/common/BellIcons'
import { useToastStore } from '@/stores/toast'
import {
  unreadCountText,
  unreadSupported,
  useUnreadEntry,
  useUnreadStore,
  useWorkspaceThreadUnread,
  type UnreadKind,
} from '@/stores/unread'
import type { ServerScope } from '@/kessel/server-scope'

const PILL =
  'min-w-[14px] h-[14px] inline-flex items-center justify-center text-[8px] font-bold text-[var(--color-on-accent)] rounded-full px-0.5'
const DOT = 'inline-block w-1.5 h-1.5 rounded-full'
const UNREAD_BG = 'bg-[var(--color-accent)]'
const MUTED_BG = 'bg-[var(--color-text-muted)]'
const WAITING_BG = 'bg-[var(--color-status-needs-you)]'

export function UnreadBadge({
  unread,
  waiting = 0,
  muted = false,
  variant = 'count',
  className = '',
  label,
}: {
  unread: number
  waiting?: number
  muted?: boolean
  /** `count`: a pill with the number. `dot`: a small dot (rails, tight rows). */
  variant?: 'count' | 'dot'
  className?: string
  /** What is unread, for the tooltip ("project messages"). */
  label?: string
}): React.JSX.Element | null {
  const unreadText = unreadCountText(unread)
  const waitingText = unreadCountText(waiting)
  if (unreadText === null && waitingText === null) return null
  const what = label ?? 'messages'
  return (
    <span
      className={`inline-flex items-center gap-0.5 flex-shrink-0 ${className}`}
      data-unread-badge=""
    >
      {waitingText !== null && (
        <span
          data-unread-waiting={waitingText}
          title={`${waiting} waiting on you`}
          className={`${variant === 'dot' ? DOT : PILL} ${WAITING_BG}`}
        >
          {variant === 'dot' ? null : waitingText}
        </span>
      )}
      {unreadText !== null && (
        <span
          data-unread-count={unreadText}
          data-muted={muted ? 'true' : 'false'}
          title={muted ? `${unreadText} unread ${what} (muted)` : `${unreadText} unread ${what}`}
          className={`${variant === 'dot' ? DOT : PILL} ${muted ? MUTED_BG : UNREAD_BG}`}
        >
          {variant === 'dot' ? null : unreadText}
        </span>
      )}
    </span>
  )
}

/** One conversation's badge, straight from the unread store. */
export function ConversationUnreadBadge({
  kind,
  id,
  scope,
  variant,
  className,
  label,
}: {
  kind: UnreadKind
  id: string | null | undefined
  scope?: ServerScope
  variant?: 'count' | 'dot'
  className?: string
  label?: string
}): React.JSX.Element | null {
  const entry = useUnreadEntry(kind, id, scope)
  if (!entry) return null
  return (
    <UnreadBadge
      unread={entry.unread}
      waiting={entry.waiting}
      muted={entry.muted}
      variant={variant}
      className={className}
      label={label}
    />
  )
}

/** An agent row's Thread badge: its workspace's Threads summed (window
 *  server only). Grey when all of the unread is muted. */
export function WorkspaceThreadUnreadBadge({
  projectId,
  className,
}: {
  projectId: string
  className?: string
}): React.JSX.Element | null {
  const sum = useWorkspaceThreadUnread(projectId)
  if (!sum) return null
  return (
    <UnreadBadge
      unread={sum.unread}
      waiting={sum.waiting}
      muted={sum.muted}
      className={className}
      label="Thread messages"
    />
  )
}

/** A menu row's words for the mute toggle, and the value picking it sets
 *  (context menus, the Thread view menu). */
export function muteMenuState(
  muted: boolean,
  noun: string,
): { label: string; next: boolean } {
  return muted ? { label: `Unmute ${noun}`, next: false } : { label: `Mute ${noun}`, next: true }
}

/** Mute / unmute, toasting a failure (the store already rolled back by
 *  refetching). */
export async function toggleMute(kind: UnreadKind, id: string, next: boolean, noun: string): Promise<void> {
  try {
    await useUnreadStore.getState().setMuted(kind, id, next)
  } catch (err) {
    useToastStore
      .getState()
      .addToast(
        `${next ? 'Mute' : 'Unmute'} ${noun} failed: ${err instanceof Error ? err.message : String(err)}`,
        'error',
      )
  }
}

/** A small bell button that mutes this conversation for the person. Hidden
 *  on a server without `thread-read-v1` or another server than the
 *  window's. */
export function MuteToggleButton({
  kind,
  id,
  noun,
  scope,
}: {
  kind: UnreadKind
  id: string | null | undefined
  /** "project chat", "Thread". */
  noun: string
  scope?: ServerScope
}): React.JSX.Element | null {
  const entry = useUnreadEntry(kind, id, scope)
  const [busy, setBusy] = useState(false)
  if (!id || !unreadSupported() || (scope !== undefined && !scope.isWindowHost())) return null
  const muted = entry?.muted === true
  const { label, next } = muteMenuState(muted, noun)
  return (
    <button
      type="button"
      role="switch"
      aria-checked={!muted}
      aria-label={label}
      title={muted ? `${noun[0].toUpperCase()}${noun.slice(1)} muted — click to unmute` : label}
      data-unread-mute={muted ? 'muted' : 'on'}
      disabled={busy}
      onClick={() => {
        setBusy(true)
        void toggleMute(kind, id, next, noun).finally(() => setBusy(false))
      }}
      className={`flex-shrink-0 p-0.5 no-drag cursor-pointer disabled:opacity-50 ${
        muted ? 'text-[var(--color-text-muted)]' : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
      }`}
    >
      {muted ? <BellOffIcon /> : <BellIcon />}
    </button>
  )
}

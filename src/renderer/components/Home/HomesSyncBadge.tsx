// homes-sync-v1 (0.46.1) — the small badge on the Home tab icon that says
// which server this device's Homes sync with (Rosson, ticket 4f537560).
// The desktop syncs with its own daemon, so the badge is a tiny computer;
// a different home server would show its initial. Its colour says how sync
// is going (accent: in step; muted: paused or changes waiting; warning:
// removed or a problem). Hidden while sync is off or unsupported.

import React from 'react'
import { statusLine, useHomesSyncStatus, type HomesSyncStatus } from '@/lib/homes-sync/runner'

export type HomesSyncBadgeTone = 'ok' | 'paused' | 'problem'

export interface HomesSyncBadgeView {
  tone: HomesSyncBadgeTone
  /** `computer` for this computer's daemon, else the server's initial. */
  glyph: 'computer' | string
  title: string
}

/** What the badge shows for a status; null hides it. */
export function homesSyncBadgeView(s: HomesSyncStatus): HomesSyncBadgeView | null {
  if (s.phase === 'off' || s.phase === 'unsupported') return null
  const tone: HomesSyncBadgeTone =
    s.phase === 'removed' || s.phase === 'error'
      ? 'problem'
      : s.phase === 'offline' || s.pending > 0
        ? 'paused'
        : 'ok'
  const glyph = s.serverLabel === 'This computer' ? 'computer' : (s.serverLabel.trim()[0] ?? '?').toUpperCase()
  return { tone, glyph, title: statusLine(s) }
}

const TONE_CLASS: Record<HomesSyncBadgeTone, string> = {
  ok: 'bg-[var(--color-accent)]',
  paused: 'bg-[var(--color-text-muted)]',
  problem: 'bg-[var(--color-status-needs-you)]',
}

export function HomesSyncBadge(): React.JSX.Element | null {
  const status = useHomesSyncStatus()
  const view = homesSyncBadgeView(status)
  if (!view) return null
  return (
    <span
      data-testid="homes-sync-badge"
      data-tone={view.tone}
      title={view.title}
      aria-label={view.title}
      className={`absolute bottom-0 right-0.5 flex h-[10px] w-[10px] items-center justify-center rounded-full text-[7px] font-bold leading-none text-[var(--color-on-accent)] ring-1 ring-[var(--color-bg)] ${TONE_CLASS[view.tone]}`}
    >
      {view.glyph === 'computer' ? (
        <svg width="7" height="7" viewBox="0 0 8 8" fill="none" aria-hidden="true">
          <rect x="1" y="1.5" width="6" height="4" rx="0.5" stroke="currentColor" strokeWidth="1" />
          <path d="M3 7h2" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
        </svg>
      ) : (
        view.glyph
      )}
    </span>
  )
}

// Usage v2 (Rosson 2026-10-16) — the Usage page's plan limits: one row per
// tool for its default login ("Claude · 58% · resets in 3d"), linking to
// Settings → Language Models → Subs / Tokens, where every login's bars live.
// Owner / Admin only (the route's floor); a refusal or an older daemon
// simply leaves the card out.

import React, { useEffect, useState } from 'react'
import { limitTone, type LimitTone } from '@/lib/subscription-usage'
import { limitsStrip } from '@/lib/usage-limits'
import { openSettingsPage } from '@/lib/open-settings-page'
import { useUsageLimitsStore, watchUsageLimits } from '@/stores/usage-limits'
import type { UsageTarget } from '@/stores/subscription-usage'
import { Button } from '@/components/ui'
import { SettingRow, SettingsGroup } from '../../controls/SettingControls'

/** The meter's fill: the accent while there's room, then the warning and
 *  error tokens. The words beside it say the same. */
const TONE_FILL: Record<LimitTone, string> = {
  ok: 'var(--color-accent)',
  warn: 'var(--color-status-warn)',
  over: 'var(--color-status-error)',
}

/** "resets 3h" → "Resets in 3h"; other notes read as they are. */
function noteText(note: string): string {
  if (note.startsWith('resets ')) {
    const when = note.slice('resets '.length)
    return when === 'now' ? 'Resets now' : `Resets in ${when}`
  }
  return note ? note[0].toUpperCase() + note.slice(1) : ''
}

export function UsageLimitsStrip({
  target,
  canLink,
}: {
  target: UsageTarget
  /** The link opens Settings on the window's server; a room's server has no
   *  Subs / Tokens page in this window. */
  canLink: boolean
}): React.JSX.Element | null {
  const entry = useUsageLimitsStore((s) => s.entries[target.key])
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => watchUsageLimits(target), [target])
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 30_000)
    return () => clearInterval(id)
  }, [])
  const items = limitsStrip(entry?.doc ?? null, now)
  if (!entry?.doc || items.length === 0) return null
  return (
    <div data-testid="usage-limits-strip">
      <SettingsGroup
        title="Plan limits"
        hint="How close each tool is to its plan’s limit, on its default login."
        card
        action={
          canLink ? (
            <Button variant="quiet" data-testid="usage-limits-strip-link" onClick={() => void openSettingsPage('llm-tokens')}>
              See every login
            </Button>
          ) : undefined
        }
      >
        {items.map((item) => {
          const tone = item.percent !== null ? limitTone(item.percent) : 'ok'
          const hint = item.warn ? (
            <span className="text-[var(--color-status-warn-text)]">{noteText(item.note) || 'Needs attention'}</span>
          ) : (
            noteText(item.note)
          )
          return (
            <SettingRow key={item.tool} label={item.display} hint={hint || undefined} testId={`usage-limits-strip-${item.tool}`}>
              {item.percent !== null ? (
                // The meter gives way (to 2rem) on a narrow line; the figure stays.
                <span className="flex items-center gap-3 min-w-0 max-w-full">
                  <span
                    role="meter"
                    aria-label={`${item.display} limit used`}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={item.percent}
                    className="relative block h-1.5 w-40 min-w-8 shrink overflow-hidden rounded-[var(--radius-pill)] bg-[var(--color-wash-2)]"
                  >
                    <span className="absolute inset-y-0 left-0" style={{ width: `${Math.min(100, item.percent)}%`, background: TONE_FILL[tone] }} />
                  </span>
                  <span className="w-12 flex-shrink-0 text-right type-row tabular-nums text-[var(--color-text-primary)]">{item.percent}%</span>
                </span>
              ) : (
                <span className="type-hint text-[var(--color-text-secondary)]">No reading</span>
              )}
            </SettingRow>
          )
        })}
      </SettingsGroup>
    </div>
  )
}

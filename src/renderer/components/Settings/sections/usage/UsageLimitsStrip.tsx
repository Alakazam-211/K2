// Usage v2 (Rosson 2026-10-16) — the Usage page's slim limits strip:
// "Claude 62% · resets 3h · Codex 18% …" for each tool's default login,
// linking to Settings → Language Models → Subs / Tokens, where every
// login's bars live. Owner / Admin only (the route's floor); a refusal or an
// older daemon simply leaves the strip out.

import React, { useEffect, useState } from 'react'
import { limitTone, limitToneColor } from '@/lib/subscription-usage'
import { limitsStrip } from '@/lib/usage-limits'
import { openSettingsPage } from '@/lib/open-settings-page'
import { useUsageLimitsStore, watchUsageLimits } from '@/stores/usage-limits'
import type { UsageTarget } from '@/stores/subscription-usage'

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
    <div
      className="mb-6 flex flex-wrap items-center gap-x-3 gap-y-1 border border-[var(--color-border)] px-3 py-2 text-[11px]"
      data-testid="usage-limits-strip"
    >
      <span className="text-[var(--color-text-muted)]">Limits</span>
      {items.map((item) => (
        <span key={item.tool} className="inline-flex items-center gap-1" data-testid={`usage-limits-strip-${item.tool}`}>
          <span className="text-[var(--color-text-primary)]">{item.display}</span>
          {item.percent !== null ? (
            <span className="tabular-nums" style={{ color: limitTone(item.percent) === 'ok' ? undefined : limitToneColor(limitTone(item.percent)) }}>
              {item.percent}%
            </span>
          ) : null}
          {item.note ? <span className="text-[var(--color-text-muted)]">· {item.note}</span> : null}
          {item.warn ? (
            <span className="text-amber-400" aria-label="needs attention">
              ⚠
            </span>
          ) : null}
        </span>
      ))}
      {canLink ? (
        <button
          type="button"
          className="ml-auto text-[var(--color-accent)] hover:underline"
          data-testid="usage-limits-strip-link"
          onClick={() => void openSettingsPage('llm-tokens')}
        >
          Every login on Subs / Tokens →
        </button>
      ) : null}
    </div>
  )
}

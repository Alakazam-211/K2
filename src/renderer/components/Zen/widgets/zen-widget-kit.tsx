// prd-zen-mode-v1 S6 — small pieces the built-in widgets share: bridge
// hooks (rows, a conversation's Thread) and the widgets' stylesheet.
//
// Widgets style themselves with `--zen-*` variables only (Z20): the theme
// engine (S5) sets them on the Zen root. Motion reads the engine's
// `--zen-anim-<name>-duration` / `-ease` with gentle defaults, and
// `prefers-reduced-motion` stops every widget animation.

import { useEffect, useState } from 'react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenAgentRow, ZenThreadView } from '@/lib/zen/zen-data'

/** The Home's rows, live (`agents.list` + `agents.subscribe`). */
export function useZenRows(bridge: ZenWidgetBridge): ZenAgentRow[] {
  const [rows, setRows] = useState<ZenAgentRow[]>(() => bridge.call('agents.list') as ZenAgentRow[])
  useEffect(() => {
    const unsubscribe = bridge.call('agents.subscribe', (next: ZenAgentRow[]) => setRows(next))
    if (typeof unsubscribe !== 'function') throw new Error('zen: agents.subscribe returned no unsubscribe')
    return unsubscribe as () => void
  }, [bridge])
  return rows
}

/** One conversation's Thread, live (`thread.subscribe`). */
export function useZenThread(bridge: ZenWidgetBridge, address: string): ZenThreadView | null {
  const [view, setView] = useState<ZenThreadView | null>(null)
  useEffect(() => {
    setView(null)
    const unsubscribe = bridge.call('thread.subscribe', address, (next: ZenThreadView) => setView(next))
    if (typeof unsubscribe !== 'function') throw new Error('zen: thread.subscribe returned no unsubscribe')
    return unsubscribe as () => void
  }, [bridge, address])
  return view
}

/** Seconds since the epoch, ticking every 30 s (relative times). */
export function useNowSec(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    const id = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [])
  return now
}

/** "2m", "1h", "3d", or "" — the list's compact time. */
export function shortAge(thenSec: number | null, nowSec: number): string {
  if (thenSec === null) return ''
  const d = Math.max(0, nowSec - thenSec)
  if (d < 60) return 'now'
  if (d < 3600) return `${Math.floor(d / 60)}m`
  if (d < 86_400) return `${Math.floor(d / 3600)}h`
  return `${Math.floor(d / 86_400)}d`
}

/** First letter (or two initials) for an agent's or person's avatar. */
export function initials(name: string): string {
  const parts = name.trim().split(/[\s._/-]+/).filter(Boolean)
  if (parts.length === 0) return '?'
  if (parts.length === 1) return parts[0].slice(0, 1).toUpperCase()
  return (parts[0][0] + parts[1][0]).toUpperCase()
}

const anim = (name: string, ms: number, ease = 'cubic-bezier(0.22, 1, 0.36, 1)'): string =>
  `var(--zen-anim-${name}-duration, ${ms}ms) var(--zen-anim-${name}-ease, ${ease})`

/** The built-in widgets' stylesheet (scoped under the Zen root). */
export const ZEN_WIDGET_CSS = `
[data-zen-root] [data-zen-agent-row] { transition: background-color ${anim('rowMove', 160)}; }
[data-zen-root] [data-zen-agent-row]:hover { background: var(--zen-surface-raised); }
[data-zen-root] [data-zen-agent-row][data-selected] { background: var(--zen-surface-raised); box-shadow: inset 0 0 0 1px var(--zen-border); }
[data-zen-root] [data-zen-agent-row] { animation: zen-row-in ${anim('rowIn', 220)} both; }
[data-zen-root] [data-zen-message] { animation: zen-message-in ${anim('messageIn', 220)} both; }
[data-zen-root] [data-zen-conversation-body] { animation: zen-fade-in ${anim('conversationSwitch', 180)} both; }
[data-zen-root] [data-zen-pulse] { animation: zen-pulse var(--zen-anim-workingPulse-duration, 1400ms) var(--zen-anim-workingPulse-ease, ease-in-out) infinite; }
[data-zen-root] [data-zen-typing] span { animation: zen-typing var(--zen-anim-workingPulse-duration, 1400ms) var(--zen-anim-workingPulse-ease, ease-in-out) infinite; }
[data-zen-root] [data-zen-typing] span:nth-child(2) { animation-delay: 160ms; }
[data-zen-root] [data-zen-typing] span:nth-child(3) { animation-delay: 320ms; }
[data-zen-root] [data-zen-soft-button] { transition: background-color ${anim('rowMove', 140)}, opacity ${anim('rowMove', 140)}; }
[data-zen-root] [data-zen-soft-button]:hover:not(:disabled) { background: var(--zen-surface-raised); }
[data-zen-root] [data-zen-soft-button]:disabled { opacity: 0.45; }
[data-zen-root] [data-zen-compose-input]::placeholder { color: var(--zen-text-muted); }
[data-zen-root] [data-zen-compose-input]:focus { outline: none; }
[data-zen-root] [data-zen-home-menu] [data-zen-home-option]:hover { background: var(--zen-surface); }
@keyframes zen-row-in { from { opacity: 0; transform: translateY(4px); } to { opacity: 1; transform: none; } }
@keyframes zen-message-in { from { opacity: 0; transform: translateY(6px) scale(0.98); } to { opacity: 1; transform: none; } }
@keyframes zen-fade-in { from { opacity: 0; } to { opacity: 1; } }
@keyframes zen-pulse { 0%, 100% { opacity: 1; transform: scale(1); } 50% { opacity: 0.45; transform: scale(0.82); } }
@keyframes zen-typing { 0%, 80%, 100% { opacity: 0.25; transform: translateY(0); } 40% { opacity: 1; transform: translateY(-2px); } }
@media (prefers-reduced-motion: reduce) {
  [data-zen-root] [data-zen-agent-row], [data-zen-root] [data-zen-message], [data-zen-root] [data-zen-conversation-body],
  [data-zen-root] [data-zen-pulse], [data-zen-root] [data-zen-typing] span { animation: none !important; transition: none !important; }
}
`

export function ZenWidgetStyles(): React.JSX.Element {
  return <style data-zen-widget-styles="">{ZEN_WIDGET_CSS}</style>
}

// prd-zen-gardens-v1 G25 and Rosson 2026-10-04 — "+ New Garden" in the
// Garden switcher's menu; Rosson 2026-10-08 — it opens the New Garden
// modal (`ZenNewGardenModal`): a name and a browsable list of the catalog
// defaults (the Diary first, then Start with the default and Start empty
// and ask my agent). Pick one, name it, Create. For K2's own catalog
// Gardens the create click is the consent: one plain sentence in the
// modal, no separate grant or scope dialog. The modal outlives this menu
// row (it closes the menu as it opens), so `ZenNewGardenHost` draws it.

import { useEffect, useRef } from 'react'
import { ZEN_OPEN_NEW_GARDEN_EVENT } from '@/lib/zen/zen-sync'
import type { ZenGardenTemplate, ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { loadZenTemplates } from '@/lib/zen/zen-templates'
import { openZenNewGarden } from '@/lib/zen/zen-new-garden'

/** The two starts as K2 has always offered them (the fallback list). */
export const ZEN_NEW_GARDEN_STARTS: ReadonlyArray<{
  start: ZenGardenTemplate
  ask: boolean
  title: string
  detail: string
}> = [
  {
    start: 'texting',
    ask: false,
    title: 'Start with the default',
    detail: 'Garden 1’s layout: your agents beside a conversation.',
  },
  {
    start: 'blank',
    ask: true,
    title: 'Start empty and ask my agent',
    detail: 'An empty page. Your agent builds it with you.',
  },
]

/** Why `name` can't be a new Garden's name, or null. Same rules as the
 *  daemon (1 to 60 characters, unique case aside), checked before the
 *  choice so a clash never waits for a second step. */
export function zenNewGardenNameProblem(name: string, taken: readonly string[]): string | null {
  // eslint-disable-next-line no-control-regex
  const clean = name.replace(/[\u0000-\u001f\u007f]/g, '').trim()
  if (clean.length === 0 || [...clean].length > 60) return 'A Garden name is 1 to 60 characters.'
  const lower = clean.toLocaleLowerCase()
  if (taken.some((t) => t.toLocaleLowerCase() === lower)) return `You already have a Garden called “${clean}”.`
  return null
}

/** Calls `open(short)` when the What's new card's "See it in New Garden"
 *  fires `k2:zen-open-new-garden` in this window (zen-sync GS43). */
export function useZenOpenNewGardenRequest(enabled: boolean, open: (short: string) => void): void {
  const openRef = useRef(open)
  openRef.current = open
  useEffect(() => {
    if (!enabled) return
    const on = (e: Event): void => {
      const short = (e as CustomEvent<{ short?: unknown }>).detail?.short
      if (typeof short === 'string' && short) openRef.current(short)
    }
    window.addEventListener(ZEN_OPEN_NEW_GARDEN_EVENT, on)
    return () => window.removeEventListener(ZEN_OPEN_NEW_GARDEN_EVENT, on)
  }, [enabled])
}

export function ZenNewGarden({
  bridge,
  onDone,
}: {
  bridge: ZenWidgetBridge
  /** Close the menu that holds this row. */
  onDone(): void
}): React.JSX.Element | null {
  if (!bridge.caps.has('gardens:manage')) return null
  return (
    <button
      type="button"
      role="menuitem"
      data-zen-new-garden=""
      onClick={() => {
        // The catalog may have grown since the last time (UWB23).
        void loadZenTemplates()
        openZenNewGarden()
        onDone()
      }}
      className="flex w-full items-center gap-3 text-left cursor-pointer"
      style={{ minHeight: 32, padding: '6px 10px', borderRadius: 'calc(var(--zen-radius) - 4px)', color: 'var(--zen-text)' }}
    >
      <span aria-hidden style={{ width: 8, textAlign: 'center', color: 'var(--zen-text-muted)' }}>
        +
      </span>
      <span>New Garden</span>
    </button>
  )
}

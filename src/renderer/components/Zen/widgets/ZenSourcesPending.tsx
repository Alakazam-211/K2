// Garden v3 data sources — "Review N pending" on a Garden (PRD §9.5, Q12;
// contract-sources-v1 §11.4). A small K2-drawn pill at the page's bottom
// right while sources this Garden's cards name wait for approval on their
// servers; it opens B2's batch dialog (one confirm per server for the
// secret-bound ones). Checked on mount and every 20 s while the window is
// visible; nothing when no card reads sources.

import { useEffect, useMemo, useState } from 'react'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { zenBoundSourceAddresses } from '@/lib/zen/zen-custom-sources'
import { zenSourcesReader } from '@/lib/zen/zen-sources-live'
import { ZEN_SOURCES_POLL_MS } from '@/lib/zen/zen-sources'
import { zenPendingCount, zenPendingSources, zenSourcesBatchOpener } from '@/lib/zen/zen-sources-review'

export function ZenSourcesPending({ gardenName, widgets }: { gardenName: string; widgets: readonly ZenWidgetDecl[] }): React.JSX.Element | null {
  const addresses = useMemo(() => {
    const all: string[] = []
    for (const w of widgets) for (const a of w.custom ? zenBoundSourceAddresses(w.custom) : []) if (!all.includes(a)) all.push(a)
    return all
  }, [widgets])
  const key = addresses.join('\n')
  const [byServer, setByServer] = useState<Record<string, string[]>>({})
  useEffect(() => {
    if (!key) {
      setByServer({})
      return
    }
    let live = true
    const check = (): void => {
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return
      void zenPendingSources(key.split('\n'), zenSourcesReader()).then((r) => {
        if (live) setByServer(r)
      })
    }
    check()
    const t = setInterval(check, ZEN_SOURCES_POLL_MS)
    return () => {
      live = false
      clearInterval(t)
    }
  }, [key])
  const n = zenPendingCount(byServer)
  const open = zenSourcesBatchOpener()
  if (n === 0 || !open) return null
  return (
    <button
      type="button"
      data-zen-sources-pending={n}
      data-zen-soft-button=""
      className="no-drag cursor-pointer"
      onClick={() => open({ gardenName, byServer })}
      style={{
        position: 'absolute',
        right: 14,
        bottom: 14,
        zIndex: 3,
        padding: '5px 12px',
        borderRadius: 999,
        border: '1px solid var(--zen-border)',
        background: 'var(--zen-surface-raised)',
        color: 'var(--zen-text)',
      }}
    >
      Review {n} pending
    </button>
  )
}

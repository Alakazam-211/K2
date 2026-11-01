// A built-in widget that needs a Beta core (the Board, Rosson 2026-11-01).
//
// `k2:board@<n>` runs only while the Board core is on (Settings → Cores →
// Multiplayer, `k2 doc cores on board`). Off, the widget's place shows a
// card with a one-click "Turn on Board" (never a Settings hunt), and the
// daemon refuses its data calls with `core_off` either way: the check that
// matters is the daemon's. The boards are kept while it's off.
//
// The state comes from `GET /cli/docs/cores` on this computer and follows
// `doc_cores_changed`, so a switch flipped anywhere (Settings, the CLI,
// another window, adding a Board Garden) shows at once. A daemon without
// the Board core (an older one) never hides the widget.

import { useEffect, useState } from 'react'
import { fetchDocCores, setDocCore } from '@/components/Settings/sections/multiplayer-cores-api'
import { onDocCoresChanged } from '@/stores/session-events'
import { zenLocalScope } from './zen-api'

/** The core a custom widget needs, by its name (`k2:board@1` → `board`). */
export function zenWidgetCore(widget: string | null | undefined): string | null {
  return typeof widget === 'string' && /^k2:board(@[1-9][0-9]*)?$/.test(widget) ? 'board' : null
}

export type ZenCoreState = 'checking' | 'on' | 'off'

/** Whether `core` is on on this computer (`on` when there's no core). */
export function useZenCoreState(core: string | null): ZenCoreState {
  const [state, setState] = useState<ZenCoreState>(core ? 'checking' : 'on')
  useEffect(() => {
    if (!core) {
      setState('on')
      return
    }
    let live = true
    const scope = zenLocalScope()
    fetchDocCores(scope)
      .then((d) => {
        if (!live) return
        const row = d.cores.find((c) => c.id === core)
        setState(!row || row.on ? 'on' : 'off')
      })
      .catch(() => {
        // The daemon gates the data itself; a failed read never hides it.
        if (live) setState('on')
      })
    const off = onDocCoresChanged(scope, (e) => {
      if (live && core in e.cores) setState(e.cores[core] ? 'on' : 'off')
    })
    return () => {
      live = false
      off()
    }
  }, [core])
  return state
}

/** The placeholder's words. */
export function zenCoreOffText(core: string): string {
  const name = core === 'board' ? 'Board' : core
  return `${name} is a Beta that's off on this computer. Turn it on here or in Settings → Cores → Multiplayer; your boards are kept while it's off.`
}

/** The card's one click: the same switch Settings flips. */
export function turnOnZenCore(core: string): Promise<void> {
  return setDocCore(core, true, zenLocalScope())
}

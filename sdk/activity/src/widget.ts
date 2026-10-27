// k2-activity@1: the Garden widget library (global `K2Activity`), built
// from ./core and ./bar by scripts/zen-lib/build-k2-activity.mjs into
// src/renderer/zen-lib/k2-activity@1.0.0/k2-activity.js (0.46.5 activity
// SDK). FSL-1.1-Apache-2.0, Alakazam Labs.
//
//   manifest.json: "caps": ["agents:read", "thread:read", "activity:read"],
//                  "requires": {"libs": ["k2-activity@1"]}
//   const bar = K2Activity.mount(el, {address: row.address, look: 'chamfer'})
//   bar.setAddress(other); bar.setLook('glass'); bar.destroy()
//   // your own drawing: K2Activity.describe(K2Activity.coerce(state.turn, Date.now()), Date.now())
//
// It listens with `k2.thread.onActivity(address, …)` (thread:read +
// activity:read): the same turn an App guest gets, counts and step kinds,
// never text. No turn = no bar: it never guesses from messages.

import { coerceTurn, describe, reduceTurn, stripClock, stripClockShort, type Turn } from './core'
import { mountBar, type BarLook } from './bar'

type Off = () => void

interface WidgetK2 {
  thread: {
    onActivity(address: string, handlers: { data(v: unknown): void; error?(e: unknown): void }): Off
  }
}

interface MountOptions {
  address?: string | null
  look?: BarLook
  endLingerMs?: number
  /** Called with each new state, after the bar drew it. */
  onChange?: (state: { address: string; turn: Turn | null; live: boolean; source: string; fresh: string }) => void
}

function k2(): WidgetK2 {
  const g = globalThis as { k2?: WidgetK2 }
  if (!g.k2 || !g.k2.thread || typeof g.k2.thread.onActivity !== 'function') {
    throw new Error('K2Activity: k2.thread.onActivity is missing (K2 0.46.5+, caps thread:read and activity:read)')
  }
  return g.k2
}

function mount(el: HTMLElement, opts: MountOptions = {}) {
  const bar = mountBar(el, { look: opts.look ?? 'chamfer', endLingerMs: opts.endLingerMs })
  let off: Off | null = null
  let address: string | null = null
  let turn: Turn | null = null

  const listen = (next: string | null): void => {
    if (off) off()
    off = null
    address = next
    turn = null
    bar.update(null)
    if (!next) return
    off = k2().thread.onActivity(next, {
      data(v) {
        if (address !== next || !v || typeof v !== 'object') return
        const s = v as { turn?: unknown; live?: boolean; source?: string; fresh?: string }
        const incoming = s.turn ? coerceTurn(s.turn, Date.now()) : null
        turn = s.turn ? reduceTurn(turn, incoming) : null
        bar.update(turn)
        opts.onChange?.({ address: next, turn, live: !!s.live, source: String(s.source ?? ''), fresh: String(s.fresh ?? '') })
      },
      error(e) {
        // A refusal (a missing cap, an agent outside the widget's scope)
        // ends the subscription: no bar, and the console says why.
        bar.update(null)
        console.warn('[k2-activity]', e)
      },
    })
  }

  listen(opts.address ?? null)
  return {
    el: bar.el,
    setAddress: (next: string | null) => listen(next),
    setLook: (look: BarLook) => bar.setLook(look),
    destroy: () => {
      if (off) off()
      off = null
      bar.destroy()
    },
  }
}

export const K2Activity = Object.freeze({
  version: '1.0.0',
  mount,
  coerce: coerceTurn,
  reduce: reduceTurn,
  describe,
  clock: stripClock,
  clockShort: stripClockShort,
})
;(globalThis as { K2Activity?: unknown }).K2Activity = K2Activity

// The viewer's own choices on the step on screen, shared by the picture and
// the panel: what you picked, the view you chose and where Esc goes back
// to, whether the views menu and the details are open, and the quiet line
// that confirms a view ("Now looking through Camera 2"). One store per
// step's state (a new step starts from what the step says, as the mock
// does), so two tabs never share one.

import type { SimState } from './sim'

export interface SceneLocal {
  /** Your pick; undefined until you pick (the step's own pick shows). */
  pick: string | null | undefined
  view: string | null
  back: string[]
  /** null: as the step says. */
  menu: boolean | null
  info: boolean
  flash: string | null
}

export const LOCAL0: SceneLocal = { pick: undefined, view: null, back: [], menu: null, info: false, flash: null }

export interface LocalStore {
  get(): SceneLocal
  set(fn: (s: SceneLocal) => SceneLocal): void
  subscribe(cb: () => void): () => void
}

const STORES = new WeakMap<SimState, LocalStore>()

function makeStore(): LocalStore {
  let state = LOCAL0
  const subs = new Set<() => void>()
  return {
    get: () => state,
    set(fn) {
      const next = fn(state)
      if (next === state) return
      state = next
      for (const cb of subs) cb()
    },
    subscribe(cb) {
      subs.add(cb)
      return () => subs.delete(cb)
    },
  }
}

/** The store for this step's state (made on first use). */
export function localStore(S: SimState): LocalStore {
  let s = STORES.get(S)
  if (!s) STORES.set(S, (s = makeStore()))
  return s
}

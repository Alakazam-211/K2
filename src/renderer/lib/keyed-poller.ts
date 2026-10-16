// One poller per key, however many components want it (Usage v2 B32: the
// top bar, Zen's band and the Zen menu each mounted their own 60 s poll for
// the same server). The first `acquire` starts the poller; each release
// drops one hold; the last release stops it.

export interface KeyedPollers {
  /** Hold the poller for `key`, starting it with `start` when none runs.
   *  Returns this hold's release (safe to call twice). */
  acquire: (key: string, start: () => () => void) => () => void
  /** Holds on `key` right now (tests). */
  holds: (key: string) => number
  /** Stop every poller (tests, a full reset). */
  stopAll: () => void
}

export function createKeyedPollers(): KeyedPollers {
  const running = new Map<string, { holds: number; stop: () => void }>()
  return {
    acquire(key, start) {
      let entry = running.get(key)
      if (entry) entry.holds += 1
      else {
        entry = { holds: 1, stop: start() }
        running.set(key, entry)
      }
      const mine = entry
      let released = false
      return () => {
        if (released) return
        released = true
        // A reset may have replaced the entry; this hold is gone with it.
        if (running.get(key) !== mine) return
        mine.holds -= 1
        if (mine.holds > 0) return
        running.delete(key)
        mine.stop()
      }
    },
    holds(key) {
      return running.get(key)?.holds ?? 0
    },
    stopAll() {
      for (const entry of running.values()) entry.stop()
      running.clear()
    },
  }
}

// Phase 2.5 fix (finding #547) — global daemon-reconnect bus.
//
// Sub-tauri's `daemon_events.rs` emits a `daemon:connected` event
// every time its background subscriber thread (re)opens its WS
// connection to the daemon. Renderer-side stores that initialize
// from `/cli/settings/get` (or any other daemon HTTP route) need
// a hook to retry their initial load when:
//
// 1. The daemon was slow to come online at boot — the store's
//    eager init-on-import fired before the daemon's HTTP listener
//    was bound (heavy first-boot migrations can take 5+ seconds).
// 2. The daemon was killed mid-session (launchctl kickstart -k,
//    crash, manual debugging) and just came back up.
//
// Without this signal, every store's `initFromSettings` swallows
// its failure with `try/catch → use defaults`, and the next user
// interaction in that subsystem persists those defaults BACK to
// settings.json — overwriting whatever was there before. The
// `hasLoadedFromDaemon` gate guards the write side; this module
// drives the recovery read side.
//
// One subscriber per browser process: every store registers a
// callback at module-load time, the listen() is shared. Tauri's
// `listen()` returns an unlisten Promise — we never unlisten
// (the listeners live for the whole app lifetime).

import { listen } from '@tauri-apps/api/event'

type Cb = () => void

const callbacks: Set<Cb> = new Set()
let subscribed = false

function ensureSubscribed(): void {
  if (subscribed) return
  subscribed = true
  // `daemon:connected` payload is intentionally empty — the event
  // itself is the signal. Subscribers re-fetch fresh state.
  listen('daemon:connected', () => {
    for (const cb of callbacks) {
      try {
        cb()
      } catch (err) {
        // One bad callback must not break the others. Stores'
        // own `initFromSettings` blocks have their own try/catch
        // for the actual fetch; this layer just guards loop hygiene.
        console.error('[daemon-reconnect] callback failed:', err)
      }
    }
  }).catch((err) => {
    // listen() rejects when run outside Tauri (jest, vitest). The
    // store-side init-on-import paths still work — they just don't
    // get the reconnect-driven retry. Smoke tests run without the
    // Tauri shim and this is the only safe behavior.
    console.warn('[daemon-reconnect] listen() unavailable:', err)
    subscribed = false
  })
}

/** Register `cb` to fire every time the Tauri-side daemon-events
 *  subscriber (re)connects. Idempotent per `cb` reference. The
 *  first call lazily attaches the underlying `listen()`. */
export function onDaemonConnected(cb: Cb): void {
  callbacks.add(cb)
  ensureSubscribed()
}

import './globals.css'
// Dev-only room-frame shim (Home P1.5 spike). Must stay the first JS import.
import './dev/room-frame-shim'
import React from 'react'
import ReactDOM from 'react-dom/client'
import { invoke } from '@tauri-apps/api/core'
import { ConnectionGate } from './components/ConnectionGate'
import { installExternalLinkHandler } from './lib/external-link-handler'
import { installRandomUUIDPolyfill } from './lib/random-uuid'
import { bootWebHostIfNeeded } from './web/boot-host'
import { bootZenPausedStart, noteZenWidgetsHeartbeat } from './lib/zen/zen-widgets-running'

// Hosted web over plain HTTP (LAN IP / remote vite:dev:web) is not a
// secure context — crypto.randomUUID is missing and toast/tabs/transfer
// blow up on drop. Polyfill before any store runs.
installRandomUUIDPolyfill()


// 0.39.x (Issue #6): webview liveness HEARTBEAT.
// Beat once synchronously the instant the bundle executes (so the
// Rust-side watchdog in src-tauri/src/lib.rs knows the renderer JS came
// alive at launch), then on a ~3s timer for the rest of the session.
// The watchdog reloads the webview from Rust (the programmatic
// equivalent of right-click → Reload) if these heartbeats go stale —
// covering BOTH the black-screen-after-update launch failure AND a
// mid-session content-process death (e.g. the renderer crashing after
// the laptop sleeps + wakes). `.catch` swallows the rejection in
// non-Tauri/dev (browser) contexts where `invoke` has no backend.
//
// Zen v2 (prd-zen-user-widgets-v2 UW32, UW56): the watchdog is per window
// (Tauri tells `renderer_heartbeat` which window called), and each beat
// also counts toward a Garden's "custom widgets started fine" marker. At
// boot this window asks whether the watchdog reloaded it, so a Garden whose
// widgets froze the window opens with them paused.
const beat = (): void => {
  void invoke('renderer_heartbeat').catch(() => {})
  noteZenWidgetsHeartbeat()
}
beat()
setInterval(beat, 3000)
void bootZenPausedStart(() => invoke<number | null>('watchdog_take_reload_note'))

// NOTE: do NOT statically import `./App` here. ConnectionGate uses
// `import('./App')` dynamically only AFTER the daemon is verified
// healthy. Statically importing it would defeat the gate: App's
// transitively-imported stores (projects, tabs, settings, focus-
// groups, timer, assistant, …) fire eager daemon fetches at
// module-init time, and those fetches would race the gate's daemon
// readiness check — the exact bug 0.39.2 left unsolved.

const root = document.getElementById('root')!

installExternalLinkHandler()

// Hosted web (VITE_WEB): force a single same-origin ConnectHost BEFORE
// ConnectionGate runs so the gate never polls the Tauri local path.
bootWebHostIfNeeded()

// Zen v2 S0 spike (prd-zen-user-widgets-v2 §15): a build with
// `VITE_K2_ZEN_SPIKE=s0` (a throwaway signed build on z3mbpZ / z13flow), or
// `?zenspike=s0` on the dev page, renders the sealed-frame capability spike
// instead of the app. A normal build never sets the variable, so this branch
// and the spike module drop out.
const zenSpikeS0 =
  window === window.top &&
  (import.meta.env.VITE_K2_ZEN_SPIKE === 's0' ||
    (import.meta.env.DEV && new URLSearchParams(window.location.search).get('zenspike') === 's0'))

// ConnectionGate (0.39.3): polls daemon's /ping until reachable,
// THEN dynamically imports + mounts App. The dynamic import is the
// key — App.tsx (and all its transitive imports) stays out of the
// JS context until the daemon is confirmed healthy, so no store
// fires a fetch against a down daemon. Closes the black-screen
// race + reusable for K2 Connect's remote-daemon scenario.
//
// Dev-only (Home P1.5 spike): `VITE_K2_ROOMFRAME_PROBE=1` or `?roomframe=1`
// renders the room-frame IPC probe instead of the app. `import.meta.env.DEV`
// is a build-time constant, so a production bundle drops this branch and the
// probe module.
const roomFrameProbe =
  import.meta.env.DEV &&
  (window === window.top
    ? Boolean(import.meta.env.VITE_K2_ROOMFRAME_PROBE) ||
      new URLSearchParams(window.location.search).has('roomframe')
    : window.location.hash === '#room=probe')

if (roomFrameProbe) {
  void import('./dev/room-frame-probe').then((m) => m.startRoomFrameProbe(root))
} else if (zenSpikeS0) {
  void import('./dev/zen-spike-s0').then((m) => m.startZenSpikeS0(root))
} else {
  ReactDOM.createRoot(root).render(
    <ConnectionGate />
  )
}

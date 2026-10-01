// Dev-only room-frame IPC probe (Home P1.5 spike,
// `.k2/prds/research-home-p15-room-frame-spike.md`).
//
// Question: can a Home remote room be a same-origin <iframe> of this app
// (`index.html#room=<hostKey>/<workspaceId>`) and still use Tauri IPC?
//
// Run: `VITE_K2_ROOMFRAME_PROBE=1 bun run tauri dev`, or open the dev page
// with `?roomframe=1`. The top frame then renders ONLY this probe (no
// ConnectionGate, no App), mounts a same-origin iframe of itself with
// `#room=probe`, and prints a results table. Set
// `VITE_K2_ROOMFRAME_SINK=http://127.0.0.1:<port>/` to also POST the table
// as JSON to a local listener.
//
// `VITE_K2_ROOMFRAME_PROBE=app` (or `?roomframe=app`) instead mounts two
// real app frames side by side: `#room=local` and `#room=<id>` for the host
// in `VITE_K2_ROOMFRAME_HOST` (see room-frame-shim.ts). Run it only against
// temp daemons (temp HOME): a full app boot writes to its daemon.
//
// index.tsx imports this module only under `import.meta.env.DEV`, so a
// production bundle never contains it.

import { invoke } from '@tauri-apps/api/core'
import { emit, emitTo, listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'

interface Row {
  side: 'parent' | 'frame'
  id: string
  ok: boolean
  detail: string
}

interface ProbeWindow extends Window {
  __TAURI_INTERNALS__?: { metadata?: unknown }
  isTauri?: boolean
  ipc?: unknown
  webkit?: { messageHandlers?: Record<string, unknown> }
  __k2ProbeLeakHits?: number
  __k2ProbeLeak2Hits?: number
}

const PING = 'k2-probe:ping'
const WIN_EVENT = 'k2-probe:win'
const LEAK = 'k2-probe:leak'
const LEAK2 = 'k2-probe:leak2'
const STORAGE_KEY = 'k2.probe.roomframe'

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms))
}

function describe(e: unknown): string {
  if (e instanceof Error) return `${e.name}: ${e.message}`
  if (e && typeof e === 'object' && 'message' in e) return String((e as { message: unknown }).message)
  return String(e)
}

async function step(
  rows: Row[],
  side: Row['side'],
  id: string,
  fn: () => Promise<{ ok: boolean; detail: string }>,
): Promise<void> {
  try {
    const r = await fn()
    rows.push({ side, id, ...r })
  } catch (e) {
    rows.push({ side, id, ok: false, detail: `threw ${describe(e)}` })
  }
}

function waitFor(pred: () => boolean, ms: number): Promise<boolean> {
  return new Promise((resolve) => {
    const start = Date.now()
    const tick = (): void => {
      if (pred()) return resolve(true)
      if (Date.now() - start > ms) return resolve(false)
      setTimeout(tick, 25)
    }
    tick()
  })
}

// ── Frame side ─────────────────────────────────────────────────────────

async function runFrame(): Promise<void> {
  const w = window as ProbeWindow
  const rows: Row[] = []
  const side = 'frame' as const

  // 1. What the native side injected into this subframe on its own.
  rows.push({ side, id: 'raw: typeof __TAURI_INTERNALS__', ok: true, detail: typeof w.__TAURI_INTERNALS__ })
  rows.push({ side, id: 'raw: typeof __TAURI_EVENT_PLUGIN_INTERNALS__', ok: true, detail: typeof w.__TAURI_EVENT_PLUGIN_INTERNALS__ })
  rows.push({ side, id: 'raw: window.isTauri', ok: true, detail: String(w.isTauri) })
  rows.push({ side, id: 'raw: typeof window.ipc', ok: true, detail: typeof w.ipc })
  rows.push({
    side,
    id: 'raw: typeof webkit.messageHandlers.ipc',
    ok: true,
    detail: typeof w.webkit?.messageHandlers?.ipc,
  })

  // 2. @tauri-apps/api invoke with no shim.
  await step(rows, side, 'direct: invoke(daemon_ws_url), no shim', async () => {
    const r = await invoke<{ state: string }>('daemon_ws_url')
    return { ok: true, detail: `resolved state=${r.state}` }
  })

  // 3. Raw IPC custom protocol from the subframe realm, no invoke key.
  await step(rows, side, 'direct: fetch ipc://localhost/renderer_heartbeat, no key', async () => {
    const res = await fetch('ipc://localhost/renderer_heartbeat', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'Tauri-Callback': '1', 'Tauri-Error': '2' },
      body: '{}',
    })
    const text = await res.text()
    return { ok: true, detail: `HTTP ${res.status} "${text.slice(0, 80)}"` }
  })

  // 4. Shim: alias the parent's Tauri globals into this realm.
  const parent = window.parent as ProbeWindow
  await step(rows, side, 'shim: alias parent.__TAURI_INTERNALS__', async () => {
    if (!parent.__TAURI_INTERNALS__) return { ok: false, detail: 'parent has no __TAURI_INTERNALS__' }
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: parent.__TAURI_INTERNALS__, configurable: true })
    Object.defineProperty(window, '__TAURI_EVENT_PLUGIN_INTERNALS__', {
      value: parent.__TAURI_EVENT_PLUGIN_INTERNALS__,
      configurable: true,
    })
    Object.defineProperty(window, 'isTauri', { value: true, configurable: true })
    return { ok: true, detail: 'aliased internals + event internals + isTauri' }
  })

  let port: number | null = null
  await step(rows, side, 'proxy: invoke(daemon_ws_url)', async () => {
    const r = await invoke<{ state: string; port?: number }>('daemon_ws_url')
    port = typeof r.port === 'number' ? r.port : null
    return {
      ok: r.state === 'available',
      detail: `state=${r.state} keys=${Object.keys(r).join(',')} instanceof frame Object=${r instanceof Object}`,
    }
  })

  await step(rows, side, 'proxy: invoke(connect_hosts_read)', async () => {
    const raw = await invoke<string>('connect_hosts_read')
    const parsed = JSON.parse(raw) as unknown
    return { ok: true, detail: `string len=${raw.length}, hosts=${Array.isArray(parsed) ? parsed.length : '?'}` }
  })

  await step(rows, side, 'proxy: window/webview label', async () => {
    const win = getCurrentWindow().label
    const wv = getCurrentWebviewWindow().label
    return { ok: true, detail: `getCurrentWindow=${win} getCurrentWebviewWindow=${wv}` }
  })

  // 5. Events: listen in the frame, emit from the frame (JS → Rust → eval
  // in the webview), handler must run in this realm.
  await step(rows, side, 'proxy: listen + emit round trip', async () => {
    let got: unknown = null
    const un = await listen<{ n: number }>(PING, (e) => {
      got = e.payload
    })
    await emit(PING, { n: 1, from: 'frame' })
    const ok = await waitFor(() => got !== null, 2000)
    un()
    return { ok, detail: ok ? `payload=${JSON.stringify(got)}` : 'no event within 2s' }
  })

  await step(rows, side, 'proxy: unlisten stops delivery', async () => {
    let hits = 0
    const un = await listen(PING, () => {
      hits += 1
    })
    un()
    await emit(PING, { n: 2, from: 'frame-after-unlisten' })
    await sleep(600)
    return { ok: hits === 0, detail: `hits after unlisten=${hits}` }
  })

  await step(rows, side, 'proxy: window-targeted listen + emitTo(label)', async () => {
    let got = false
    const win = getCurrentWindow()
    const un = await win.listen(WIN_EVENT, () => {
      got = true
    })
    await emitTo(win.label, WIN_EVENT, { from: 'frame' })
    const ok = await waitFor(() => got, 2000)
    un()
    return { ok, detail: ok ? `delivered to label=${win.label}` : 'no event within 2s' }
  })

  // 6. Listeners left behind when the frame is removed.
  await step(rows, side, 'proxy: register leak listeners for detach test', async () => {
    const p = window.parent as ProbeWindow
    await listen(LEAK, () => {
      p.__k2ProbeLeakHits = (p.__k2ProbeLeakHits ?? 0) + 1
    })
    const un2 = await listen(LEAK2, () => {
      p.__k2ProbeLeak2Hits = (p.__k2ProbeLeak2Hits ?? 0) + 1
    })
    window.addEventListener('pagehide', () => un2())
    return { ok: true, detail: `${LEAK} never unlistened; ${LEAK2} unlistened on pagehide` }
  })

  // 7. Network from the frame realm to the local daemon (read-only).
  await step(rows, side, 'net: fetch local /boot-status', async () => {
    if (port === null) return { ok: false, detail: 'no port' }
    const res = await fetch(`http://127.0.0.1:${port}/boot-status`)
    const body = (await res.json()) as { phase?: string; protocol?: number }
    return { ok: res.ok, detail: `HTTP ${res.status} phase=${body.phase} protocol=${body.protocol}` }
  })

  // 8. Storage is one origin: the parent checks this key after.
  await step(rows, side, 'storage: write probe key', async () => {
    localStorage.setItem(STORAGE_KEY, 'frame')
    sessionStorage.setItem(STORAGE_KEY, 'frame')
    return { ok: true, detail: `localStorage + sessionStorage ${STORAGE_KEY}=frame` }
  })

  await step(rows, side, 'geometry: frameElement rect', async () => {
    const el = window.frameElement
    if (!el) return { ok: false, detail: 'frameElement null' }
    const r = el.getBoundingClientRect()
    return {
      ok: true,
      detail: `x=${Math.round(r.x)} y=${Math.round(r.y)} w=${Math.round(r.width)} h=${Math.round(r.height)}; frame viewport ${window.innerWidth}x${window.innerHeight}`,
    }
  })

  rows.push({ side, id: 'focus: document.hasFocus() at end', ok: true, detail: String(document.hasFocus()) })

  window.parent.postMessage({ k2RoomFrameProbe: rows }, window.location.origin)
}

// ── Parent side ────────────────────────────────────────────────────────

function renderTable(root: HTMLElement, rows: Row[], status: string): void {
  const esc = (s: string): string =>
    s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  root.innerHTML = `
    <div style="font:12px ui-monospace,monospace;color:#ddd;background:#111;padding:12px;height:100vh;overflow:auto;box-sizing:border-box">
      <div style="font-size:14px;margin-bottom:8px">Room-frame IPC probe — ${esc(status)}</div>
      <table style="border-collapse:collapse;width:100%">
        ${rows
          .map(
            (r) => `<tr>
              <td style="padding:2px 8px;color:#888">${r.side}</td>
              <td style="padding:2px 8px;color:${r.ok ? '#7c7' : '#e77'}">${r.ok ? 'ok' : 'FAIL'}</td>
              <td style="padding:2px 8px">${esc(r.id)}</td>
              <td style="padding:2px 8px;color:#aaa">${esc(r.detail)}</td>
            </tr>`,
          )
          .join('')}
      </table>
      <div id="k2-probe-frame-host"></div>
    </div>`
}

async function sendToSink(rows: Row[]): Promise<void> {
  const sink = import.meta.env.VITE_K2_ROOMFRAME_SINK
  if (!sink) return
  try {
    await fetch(sink, {
      method: 'POST',
      mode: 'no-cors',
      headers: { 'Content-Type': 'text/plain' },
      body: JSON.stringify({ ua: navigator.userAgent, href: location.href, rows }, null, 2),
    })
  } catch (e) {
    console.warn('[roomframe-probe] sink POST failed', e)
  }
}

async function runParent(root: HTMLElement): Promise<void> {
  const w = window as ProbeWindow
  const rows: Row[] = []
  const side = 'parent' as const
  renderTable(root, rows, 'running')

  await step(rows, side, 'baseline: invoke(daemon_ws_url)', async () => {
    const r = await invoke<{ state: string }>('daemon_ws_url')
    return { ok: r.state === 'available', detail: `state=${r.state}` }
  })

  const framePings: unknown[] = []
  const unPing = await listen(PING, (e) => {
    framePings.push(e.payload)
  })

  let blurs = 0
  window.addEventListener('blur', () => {
    blurs += 1
  })

  // Mount the same app URL as a same-origin subframe, offset from (0,0) so
  // the frame reports non-zero geometry.
  const frame = document.createElement('iframe')
  frame.src = `${location.pathname}?roomframe=1#room=probe`
  frame.style.cssText =
    'position:fixed;left:240px;top:160px;width:640px;height:360px;border:1px solid #444;background:#000'
  document.body.appendChild(frame)

  const frameRows = await new Promise<Row[] | null>((resolve) => {
    const timer = setTimeout(() => resolve(null), 20000)
    window.addEventListener('message', (ev) => {
      if (ev.origin !== location.origin) return
      const data = ev.data as { k2RoomFrameProbe?: Row[] }
      if (data && Array.isArray(data.k2RoomFrameProbe)) {
        clearTimeout(timer)
        resolve(data.k2RoomFrameProbe)
      }
    })
  })
  if (!frameRows) {
    rows.push({ side, id: 'frame: results via postMessage', ok: false, detail: 'no results within 20s' })
  } else {
    rows.push({ side, id: 'frame: results via postMessage', ok: true, detail: `${frameRows.length} rows` })
    rows.push(...frameRows)
  }

  rows.push({
    side,
    id: 'events: parent saw the frame emit',
    ok: framePings.length > 0,
    detail: `parent PING hits=${framePings.length} ${JSON.stringify(framePings)}`,
  })
  unPing()

  await step(rows, side, 'storage: frame write visible in parent', async () => {
    const ls = localStorage.getItem(STORAGE_KEY)
    const ss = sessionStorage.getItem(STORAGE_KEY)
    localStorage.removeItem(STORAGE_KEY)
    sessionStorage.removeItem(STORAGE_KEY)
    return { ok: ls === 'frame', detail: `localStorage=${ls} sessionStorage=${ss}` }
  })

  await step(rows, side, 'focus: parent window blur when frame takes focus', async () => {
    const before = blurs
    frame.contentWindow?.focus()
    await sleep(300)
    return {
      ok: true,
      detail: `blur events=${blurs - before}; activeElement=${document.activeElement?.tagName}; parent hasFocus=${document.hasFocus()}`,
    }
  })

  // Remove the frame, then fire the events its listeners still hold.
  await step(rows, side, 'detach: listeners of a removed frame', async () => {
    w.__k2ProbeLeakHits = 0
    w.__k2ProbeLeak2Hits = 0
    frame.remove()
    await sleep(400)
    await emit(LEAK, { from: 'parent-after-detach' })
    await emit(LEAK2, { from: 'parent-after-detach' })
    await sleep(800)
    return {
      ok: true,
      detail: `never-unlistened hits=${w.__k2ProbeLeakHits}; pagehide-unlistened hits=${w.__k2ProbeLeak2Hits}`,
    }
  })

  const fails = rows.filter((r) => !r.ok).length
  renderTable(root, rows, `done, ${rows.length} rows, ${fails} not ok`)
  console.info('[roomframe-probe]', JSON.stringify(rows, null, 2))
  await sendToSink(rows)
}

function runAppFrames(root: HTMLElement): void {
  const raw = import.meta.env.VITE_K2_ROOMFRAME_HOST
  const hostId = raw ? String((JSON.parse(raw) as { id?: string }).id) : null
  root.innerHTML = `<div style="font:12px ui-monospace,monospace;color:#ddd;background:#111;height:100vh;padding:8px;box-sizing:border-box;display:flex;flex-direction:column;gap:6px">
    <div>Room-frame app probe — left: #room=local · right: #room=${hostId ?? '(no VITE_K2_ROOMFRAME_HOST)'}</div>
    <div id="k2-probe-frames" style="flex:1;display:flex;gap:6px;min-height:0"></div>
  </div>`
  const host = root.querySelector('#k2-probe-frames') as HTMLElement
  for (const key of hostId ? ['local', hostId] : ['local']) {
    const f = document.createElement('iframe')
    f.src = `${location.pathname}#room=${key}`
    f.title = `room ${key}`
    f.style.cssText = 'flex:1;border:1px solid #444;background:#000;min-width:0'
    host.appendChild(f)
  }
  // Native menu items reach the webview as `win.emit('menu:*')`
  // (src-tauri/src/menu.rs emit_to_focused). Fire one the same way after
  // both rooms boot: every realm's App listener receives it.
  setTimeout(() => {
    const label = getCurrentWindow().label
    console.info('[roomframe-probe] emitTo', label, 'menu:new-tab')
    void emitTo(label, 'menu:new-tab')
  }, 25000)
}

export async function startRoomFrameProbe(root: HTMLElement): Promise<void> {
  const appMode =
    import.meta.env.VITE_K2_ROOMFRAME_PROBE === 'app' ||
    new URLSearchParams(window.location.search).get('roomframe') === 'app'
  if (window === window.top && appMode) {
    runAppFrames(root)
    return
  }
  if (window !== window.top) {
    root.textContent = 'room-frame probe (frame)'
    await runFrame()
    return
  }
  await runParent(root)
}

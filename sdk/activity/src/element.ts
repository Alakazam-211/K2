// <k2-activity-bar addr="sales" look="glass">: K2's activity bar for an App
// page (0.46.5 activity SDK, review §2.3). FSL-1.1-Apache-2.0, Alakazam Labs.
//
// Built by scripts/zen-lib/build-k2-activity.mjs into
// sdk/activity/browser/k2-activity-bar.js (one classic script; it defines the
// element and `window.K2ActivityBar`). It uses the page's own App session
// through the App helper, same origin: `GET /cli/thread/activity?addr=`
// for the snapshot and `WS /cli/thread/stream?rooms=all&ear=1` for frames
// (the guest needs thread:read + activity:read in that room). Everything
// else is `watchActivity` from ./core: socket first, a read on every open,
// activity_reset, the 60 s self-check, back-off on 429.
//
// Attributes: `addr` (the agent's handle, required), `look` (chamfer,
// glass, plain), `base` (another origin's helper, rarely needed).
// Theming: --k2a-bg, --k2a-fg, --k2a-muted, --k2a-work, --k2a-need,
// --k2a-quiet, --k2a-fail, --k2a-radius, --k2a-font on the element.

import { watchActivity, type ActivityTransport, type Snapshot, type Turn } from './core'
import { mountBar, type Bar, type BarLook } from './bar'

const BACKOFF_MS = [500, 1000, 2000, 5000, 10000]

function looks(v: string | null): BarLook {
  return v === 'glass' || v === 'plain' ? v : 'chamfer'
}

/** The App page transport for one agent (by handle). */
export function appPageTransport(addr: string, base = ''): ActivityTransport {
  let conversation: string | null = null
  let room: string | null = null
  const origin = base || (globalThis as { location?: { origin: string } }).location?.origin || ''
  return {
    async read(): Promise<Snapshot> {
      const res = await fetch(`${origin}/cli/thread/activity?addr=${encodeURIComponent(addr)}`, { credentials: 'include' })
      const text = await res.text()
      const body = text ? (JSON.parse(text) as Record<string, unknown>) : {}
      if (!res.ok) {
        const err = (body.error ?? body) as Record<string, unknown>
        const header = Number(res.headers.get('retry-after'))
        throw {
          status: res.status,
          code: err.code ?? 'failed',
          retryAfterMs: typeof err.retryAfterMs === 'number' ? err.retryAfterMs : Number.isFinite(header) && header > 0 ? header * 1000 : null,
        }
      }
      const conv = body.conversation_id ?? body.conversation
      if (typeof conv === 'string') conversation = conv
      return { [addr]: body.turn ?? null }
    },
    connect(on) {
      let ws: WebSocket | null = null
      let stopped = false
      let attempt = 0
      let timer: ReturnType<typeof setTimeout> | null = null
      const open = (): void => {
        const url = `${origin.replace(/^http/, 'ws')}/cli/thread/stream?rooms=all&ear=1`
        const sock = new WebSocket(url)
        ws = sock
        sock.onopen = () => {
          attempt = 0
        }
        sock.onmessage = (ev) => {
          if (typeof ev.data !== 'string') return
          let f: Record<string, unknown>
          try {
            f = JSON.parse(ev.data) as Record<string, unknown>
          } catch {
            return
          }
          if (f.kind === 'inbox_subscribed') {
            // The socket is (re)open and K2 says which rooms it streams:
            // find ours by its pinned Chat once the read has named it.
            const resolved = (f.resolved ?? {}) as Record<string, string>
            for (const [r, c] of Object.entries(resolved)) if (c === conversation) room = r
            on.open()
          } else if (f.kind === 'thread_activity') {
            const conv = f.conversation_id ?? f.conversation
            if (conversation !== null && conv !== conversation) return
            if (typeof f.room === 'string') room = f.room
            on.frame(addr, f.activity)
          } else if (f.kind === 'activity_reset') {
            const turns = (f.turns ?? {}) as Record<string, unknown>
            if (room !== null && room in turns) on.reset({ [addr]: turns[room] ?? null })
            else on.reset()
          }
        }
        sock.onclose = () => {
          ws = null
          if (stopped) return
          on.drop()
          const wait = BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)] ?? 10000
          attempt += 1
          timer = setTimeout(() => {
            timer = null
            if (!stopped) open()
          }, wait)
        }
      }
      open()
      return () => {
        stopped = true
        if (timer) clearTimeout(timer)
        if (ws && ws.readyState <= 1) ws.close(1000, 'unsubscribed')
      }
    },
  }
}

/** The custom element. */
export class K2ActivityBarElement extends HTMLElement {
  static get observedAttributes(): string[] {
    return ['addr', 'look', 'base']
  }

  private bar: Bar | null = null
  private off: (() => void) | null = null
  private turn: Turn | null = null

  connectedCallback(): void {
    const shadow = this.shadowRoot ?? this.attachShadow({ mode: 'open' })
    if (!this.bar) {
      const host = this.ownerDocument.createElement('div')
      shadow.appendChild(host)
      this.bar = mountBar(host, { look: looks(this.getAttribute('look')) })
    }
    this.listen()
  }

  disconnectedCallback(): void {
    this.off?.()
    this.off = null
  }

  attributeChangedCallback(name: string): void {
    if (name === 'look') this.bar?.setLook(looks(this.getAttribute('look')))
    else if (this.isConnected) this.listen()
  }

  /** The turn drawn now (null: none). */
  get activity(): Turn | null {
    return this.turn
  }

  private listen(): void {
    this.off?.()
    this.off = null
    this.turn = null
    this.bar?.update(null)
    const addr = this.getAttribute('addr')
    if (!addr) return
    this.off = watchActivity(
      appPageTransport(addr, this.getAttribute('base') ?? ''),
      (states) => {
        this.turn = states.get(addr)?.turn ?? null
        this.bar?.update(this.turn)
        this.dispatchEvent(new CustomEvent('k2-activity', { detail: states.get(addr) ?? null }))
      },
      { endLingerMs: 2_000 },
    )
  }
}

const reg = (globalThis as { customElements?: CustomElementRegistry }).customElements
if (reg && !reg.get('k2-activity-bar')) reg.define('k2-activity-bar', K2ActivityBarElement)
;(globalThis as { K2ActivityBar?: unknown }).K2ActivityBar = K2ActivityBarElement

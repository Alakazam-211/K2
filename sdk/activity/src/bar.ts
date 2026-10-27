// @k2/activity bar: K2's activity bar as a few DOM nodes (0.46.5 activity
// SDK, review §2.3). FSL-1.1-Apache-2.0, Alakazam Labs.
//
// Draws `describe(turn)` from ./core: the same words the desktop drawer
// says. textContent only (never markup from data); one <style> per root
// (the document, or a shadow root). `monitoring`, needs-you, a quiet turn
// and an end never pulse: no typing dots. No Stop button: a widget or an
// App can't type into the agent's terminal.

import { describe, type Counter, type Description, type Turn } from './core'

export type BarLook = 'chamfer' | 'glass' | 'plain'

export interface BarOptions {
  look?: BarLook
  /** Keep "Done" / "Stopped" / "Failed" this long after an end frame (2 s). */
  endLingerMs?: number
  now?: () => number
}

export interface Bar {
  /** Draw this turn (null: hide the bar). */
  update(turn: Turn | null): void
  setLook(look: BarLook): void
  /** The node the bar draws into (for tests and custom placement). */
  readonly el: HTMLElement
  destroy(): void
}

const ICON: Record<Counter['key'], string> = {
  subagents: '◇',
  background: '◷',
  read: '▤',
  search: '⌕',
  cmd: '›',
  edit: '✎',
}

const CHIP_WORD: Record<Counter['key'], string> = {
  subagents: 'subagents',
  background: 'background tasks',
  read: 'files read',
  search: 'searches',
  cmd: 'commands',
  edit: 'files edited',
}

/** The bar's CSS: `--k2a-*` properties, which fall back to a Garden's
 *  `--zen-*` theme and then to neutral light/dark colours. */
export const BAR_CSS = [
  '.k2a{--k2a-bg:var(--zen-surface-raised,rgba(127,127,127,.12));--k2a-fg:var(--zen-text,inherit);--k2a-muted:var(--zen-text-muted,rgba(127,127,127,.95));--k2a-work:var(--zen-accent,#3b82f6);--k2a-need:#d97706;--k2a-quiet:rgba(127,127,127,.8);--k2a-fail:#dc2626;--k2a-radius:var(--zen-radius,8px);--k2a-font:inherit}',
  '.k2a{display:flex;align-items:center;gap:.5em;min-width:0;padding:.3em .6em;font:12px/1.4 var(--k2a-font);color:var(--k2a-fg);background:var(--k2a-bg);border-radius:var(--k2a-radius);cursor:pointer;user-select:none;flex-wrap:wrap}',
  '.k2a[hidden]{display:none}',
  ".k2a[data-look=chamfer]{border-radius:0;clip-path:polygon(6px 0,100% 0,100% calc(100% - 6px),calc(100% - 6px) 100%,0 100%,0 6px)}",
  '.k2a[data-look=glass]{border-radius:999px;backdrop-filter:blur(8px);-webkit-backdrop-filter:blur(8px)}',
  '.k2a[data-look=plain]{background:none;padding:0}',
  '.k2a-dot{flex:none;width:.6em;height:.6em;border-radius:50%;background:var(--k2a-work)}',
  '.k2a[data-tone=monitoring] .k2a-dot{background:none;box-shadow:inset 0 0 0 2px var(--k2a-work)}',
  '.k2a[data-tone=needs-you] .k2a-dot{background:var(--k2a-need)}',
  '.k2a[data-tone=unverifiable] .k2a-dot,.k2a[data-tone=stopped] .k2a-dot,.k2a[data-tone=done] .k2a-dot{background:var(--k2a-quiet)}',
  '.k2a[data-tone=failed] .k2a-dot{background:var(--k2a-fail)}',
  '.k2a[data-animate=true] .k2a-dot{animation:k2a-pulse 1.4s ease-in-out infinite}',
  '@keyframes k2a-pulse{0%,100%{opacity:1}50%{opacity:.35}}',
  '@media (prefers-reduced-motion:reduce){.k2a[data-animate=true] .k2a-dot{animation:none}}',
  '.k2a-head{flex:1 1 auto;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}',
  '.k2a-head code{font-family:var(--zen-font-mono,ui-monospace,monospace);font-size:.95em}',
  '.k2a-chips{display:flex;gap:.5em;color:var(--k2a-muted)}',
  '.k2a-chip{white-space:nowrap}',
  '.k2a-clock{color:var(--k2a-muted);font-variant-numeric:tabular-nums}',
  '.k2a-detail{flex:1 0 100%;color:var(--k2a-muted);white-space:normal}',
  '.k2a-detail[hidden]{display:none}',
  '.k2a-detail div{overflow-wrap:anywhere}',
].join('\n')

/** Add the bar's <style> once to `root` (a document or a shadow root). */
export function ensureBarStyle(root: Document | ShadowRoot): void {
  const isDoc = root.nodeType === 9
  const doc = isDoc ? (root as Document) : (root as ShadowRoot).ownerDocument
  const holder: ParentNode = isDoc ? ((root as Document).head ?? (root as Document).documentElement) : root
  if (holder.querySelector('style[data-k2-lib="k2-activity"]')) return
  const s = doc.createElement('style')
  s.setAttribute('data-k2-lib', 'k2-activity')
  s.textContent = BAR_CSS
  holder.appendChild(s)
}

/** Text with `code` spans: backticks become <code>, everything stays text. */
function fillCode(el: HTMLElement, text: string): void {
  const doc = el.ownerDocument
  el.replaceChildren()
  text.split('`').forEach((part, i) => {
    if (!part) return
    if (i % 2 === 1) {
      const c = doc.createElement('code')
      c.textContent = part
      el.appendChild(c)
    } else el.appendChild(doc.createTextNode(part))
  })
}

function chipText(c: Counter): string {
  if (c.key === 'subagents') return c.count > 0 ? `${ICON.subagents} ${c.count}${c.done ? ` ✓${c.done}` : ''}` : `${ICON.subagents} ✓${c.done}`
  return `${ICON[c.key]} ${c.count}`
}

/** Mount a bar into `host` (appended as its last child). */
export function mountBar(host: HTMLElement, opts: BarOptions = {}): Bar {
  const doc = host.ownerDocument
  const now = opts.now ?? (() => Date.now())
  const linger = Math.max(0, opts.endLingerMs ?? 2_000)
  const root = host.getRootNode()
  ensureBarStyle(root.nodeType === 11 ? (root as ShadowRoot) : doc)
  const el = doc.createElement('div')
  el.className = 'k2a'
  el.hidden = true
  el.setAttribute('role', 'status')
  el.setAttribute('aria-live', 'polite')
  el.setAttribute('data-look', opts.look ?? 'chamfer')
  const dot = doc.createElement('span')
  dot.className = 'k2a-dot'
  dot.setAttribute('aria-hidden', 'true')
  const head = doc.createElement('span')
  head.className = 'k2a-head'
  const chips = doc.createElement('span')
  chips.className = 'k2a-chips'
  const clock = doc.createElement('span')
  clock.className = 'k2a-clock'
  const detail = doc.createElement('div')
  detail.className = 'k2a-detail'
  detail.hidden = true
  el.append(dot, head, chips, clock, detail)
  host.appendChild(el)

  let turn: Turn | null = null
  let endedAt: number | null = null
  let tick: ReturnType<typeof setInterval> | null = null
  let destroyed = false

  const stopTick = (): void => {
    if (tick !== null) clearInterval(tick)
    tick = null
  }

  const paint = (): void => {
    if (destroyed) return
    const t = now()
    let d: Description | null = describe(turn, t)
    if (d && d.ended && (endedAt === null || t - endedAt >= linger)) d = null
    if (!d) {
      el.hidden = true
      el.removeAttribute('data-tone')
      head.replaceChildren()
      chips.replaceChildren()
      clock.textContent = ''
      detail.replaceChildren()
      stopTick()
      return
    }
    el.hidden = false
    el.setAttribute('data-tone', d.tone)
    el.setAttribute('data-animate', String(d.animate))
    el.setAttribute('aria-label', d.aria)
    fillCode(head, d.headline)
    chips.replaceChildren(
      ...d.counters.map((c) => {
        const s = doc.createElement('span')
        s.className = 'k2a-chip'
        s.setAttribute('data-key', c.key)
        s.title = `${c.count} ${CHIP_WORD[c.key]}${c.done ? `, ${c.done} done` : ''}`
        s.textContent = chipText(c)
        return s
      }),
    )
    clock.textContent = d.clock
    detail.replaceChildren(
      ...d.detail.map((line) => {
        const row = doc.createElement('div')
        fillCode(row, line)
        return row
      }),
    )
    if (tick === null) tick = setInterval(paint, 1000)
  }

  const toggle = (): void => {
    detail.hidden = !detail.hidden
  }
  el.addEventListener('click', toggle)

  return {
    el,
    update(next) {
      if (destroyed) return
      // The linger counts from the first time this turn's end was drawn.
      const sameEnd = !!(turn && turn.end && next && next.end && turn.turnId === next.turnId)
      turn = next
      endedAt = next && next.end ? (sameEnd ? endedAt : now()) : null
      paint()
    },
    setLook(look) {
      el.setAttribute('data-look', look)
    },
    destroy() {
      if (destroyed) return
      destroyed = true
      stopTick()
      el.removeEventListener('click', toggle)
      el.remove()
    },
  }
}

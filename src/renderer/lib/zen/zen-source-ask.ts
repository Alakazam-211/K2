// Garden v3 data sources — "Ask an agent about this" (PRD §9.4,
// contract-sources-v1 §11.4). A card reading sources without thread:post can't post to the
// Thread; K2's own Ask box can. It opens the existing Ask my agent flow
// (`ZenAskMyAgent`: the chooser, then that agent's conversation with the
// message box filled, never sent) with a draft K2 writes itself:
//
//   the source address and label, its state, the reason sentence, the
//   failing item keys, "this check runs on <host>" for another server,
//   and `k2 sources show <name>`.
//
// The draft NEVER carries a field value (not even for `values` sources; the
// agent reads them with `k2 sources show`, masked as its server decides),
// a secret, a URL query or response text. Which agent (Q15): the agent that
// made the source when it lives on this computer, else the one last chosen
// for this Garden, else the chooser.

import { create } from 'zustand'
import type { ZenSourceView } from './zen-sources'
import { parseSourceAddress } from './zen-sources'

/** What the draft is about. `view` is the last view K2 has (may be null:
 *  a source the card never read). */
export interface ZenSourceAskRequest {
  gardenId: string
  gardenName: string
  address: string
  itemKey: string | null
  view: ZenSourceView | null
  /** `agent:<projectId>` when the source's maker is known (local sources). */
  madeBy: string | null
}

const STATE_WORDS: Record<string, string> = {
  ok: 'green (ok)',
  warn: 'amber (warn)',
  crit: 'red (crit)',
  unknown: 'unknown',
  stale: 'stale: no good check lately',
  pending: 'waiting for its first check',
  paused: 'paused',
}

function oneLine(s: string, max = 120): string {
  const t = s.replace(/[\u0000-\u001f\u007f]+/g, ' ').replace(/\s+/g, ' ').trim()
  return t.length > max ? `${t.slice(0, max - 1)}…` : t
}

/** The draft K2 puts in the box. Pure; never includes a field value. */
export function zenSourceAskDraft(req: ZenSourceAskRequest): string {
  const p = parseSourceAddress(req.address)
  const name = p?.name ?? req.address
  const v = req.view
  const lines: string[] = []
  const label = v?.label ? `"${oneLine(v.label, 80)}" ` : ''
  lines.push(`About the data source ${label}(${req.address}) on my Zen Garden "${oneLine(req.gardenName, 60)}":`)
  if (v) {
    let state = STATE_WORDS[v.display] ?? v.display
    if (v.reach === 'unreachable') state = `unknown right now: ${p?.host ?? 'its server'} isn’t answering (last seen ${STATE_WORDS[v.health] ?? v.health})`
    if (v.reach === 'removed') state = `removed on ${p?.host ?? 'this computer'}`
    lines.push(`It's ${state}.${v.reasonText ? ` ${oneLine(v.reasonText)}` : v.reason ? ` Reason: ${v.reason}.` : ''}`)
    if (v.simulated) lines.push(`(A TEST state is on: ${v.simulated.state}.)`)
    const failing = v.items.filter((it) => it.health === 'crit' || it.health === 'warn' || it.health === 'unknown')
    if (req.itemKey) {
      const it = v.items.find((i) => i.key === req.itemKey)
      const why = it ? `${it.health}${it.reason ? `, ${it.reason}` : ''}` : 'not in the last check'
      lines.push(`I'm asking about ${oneLine(req.itemKey, 80)} (${why}).`)
    } else if (failing.length > 0) {
      const shown = failing.slice(0, 12).map((it) => `${oneLine(it.key, 60)} (${it.health}${it.reason ? `, ${it.reason}` : ''})`)
      const more = failing.length > shown.length ? `, and ${failing.length - shown.length} more` : ''
      lines.push(`Not green: ${shown.join(', ')}${more}.`)
    }
  } else if (req.itemKey) {
    lines.push(`I'm asking about ${oneLine(req.itemKey, 80)}.`)
  }
  if (p?.host) lines.push(`This check runs on ${p.host}.`)
  lines.push(`Read it with: k2 sources show ${name}${p?.host ? ` (on ${p.host})` : ''}`)
  lines.push('')
  lines.push('Can you look into it? ')
  return lines.join('\n')
}

// ── which agent ───────────────────────────────────────────────────────

const LAST_KEY = 'k2.zen.sourceAsk.lastAgent.v1'

/** The agent last chosen for this Garden (per-device convenience). */
export function zenSourceAskLastAgent(gardenId: string): string | null {
  try {
    const raw = globalThis.localStorage?.getItem(LAST_KEY)
    const m = raw ? (JSON.parse(raw) as Record<string, unknown>) : {}
    const v = m[gardenId]
    return typeof v === 'string' ? v : null
  } catch {
    return null
  }
}

export function rememberZenSourceAskAgent(gardenId: string, address: string): void {
  try {
    const raw = globalThis.localStorage?.getItem(LAST_KEY)
    const m = raw ? (JSON.parse(raw) as Record<string, unknown>) : {}
    m[gardenId] = address
    globalThis.localStorage?.setItem(LAST_KEY, JSON.stringify(m))
  } catch (err) {
    console.warn('[zen] Ask: could not remember the chosen agent:', err)
  }
}

/** The maker's project id from `madeBy` (`agent:<projectId>`). */
export function zenSourceMakerProject(madeBy: string | null): string | null {
  return madeBy && madeBy.startsWith('agent:') ? madeBy.slice('agent:'.length) || null : null
}

// ── the open request (one at a time, per window) ───────────────────────

interface AskState {
  request: ZenSourceAskRequest | null
  open(req: ZenSourceAskRequest): void
  close(): void
}

export const useZenSourceAskStore = create<AskState>((set) => ({
  request: null,
  open: (req) => set({ request: req }),
  close: () => set({ request: null }),
}))

/** Open K2's Ask box (replaces one already open). */
export function openZenSourceAsk(req: ZenSourceAskRequest): boolean {
  useZenSourceAskStore.getState().open(req)
  return true
}

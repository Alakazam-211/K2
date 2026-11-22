// What K2 says about a timeline's changes, in plain words: from the actor's
// name, the verb, the ops and the lanes model either side of the change.
// Places are timecode; each touched clip's start rides on the frame
// (`derived.units.<address>.tc`, as given) and the model's own count is only
// a fallback. Codes (MPC-E301 …) are only ever in a tooltip.

import { actorOf } from './actors'
import { modelOf, visibleItems, type Item, type TimelineModel } from './fold'
import { pictureAt, type MediaFacts } from './picture'
import { CONT, MKR, PROPS, UNIT, VIEW, type Book, type Claim, type OutsideSaveEvent, type Refusal, type TimelineFrame } from './schema'
import { PRODUCT_NAME } from '@/lib/brand'

export const plural = (n: number, w: string): string => `${n} ${w}${n === 1 ? '' : 's'}`

export interface Phrase {
  who: string
  sep: string
  text: string
  place: string
  target: { item?: string | null; marker?: string; f: number | null; gone?: Item } | null
  short: string
  line: string
}

/** A touched clip's timecode from the frame, as given: [start, end?]. */
export function frameTc(f: Pick<TimelineFrame, 'derived'> | null | undefined, key: string): [string, string | null] | null {
  const d = f?.derived?.units?.[key]
  if (d && typeof d.tc === 'string') return [d.tc, null]
  const g = d && d.tc && typeof d.tc === 'object' ? d.tc.geom : null
  const m = typeof g === 'string' ? /^([^-]+)-([^-]+)$/.exec(g) : null
  return m ? [m[1], m[2]] : null
}

export function framePhrase(f: TimelineFrame, B0: Book, B1: Book): Phrase {
  const a = actorOf(f.actor)
  const file = a.kind === 'file'
  const M0 = modelOf(B0)
  const M1 = modelOf(B1)
  const L = f.ops.filter((o) => o.kind === 'list')
  const MP = f.ops.filter((o) => o.kind === 'map')
  const out = (text: string, place: string | null, target: Phrase['target'], short?: string): Phrase => ({
    who: a.name,
    sep: file ? ': ' : ' ',
    text,
    place: place || '',
    target: target || null,
    short: short || text,
    line: `${a.name}${file ? ': ' : ' '}${text}${place && !text.includes(place) ? ' · ' + place : ''}`,
  })
  const lab = (M: TimelineModel, k: string): string => (M.byT[k] ? M.byT[k].label : '')
  if (file) {
    const ik = new Set(f.ops.filter((o) => o.root === PROPS[UNIT]).map((o) => o.key))
    const ins = L.filter((o) => o.root === UNIT && o.op === 'insert').map((o) => o.key)
    const del = L.filter((o) => o.root === UNIT && o.op === 'delete').map((o) => o.key)
    const chg = [...ik].filter((k) => !ins.includes(k) && !del.includes(k))
    const mi = L.filter((o) => o.root === MKR && o.op === 'insert')
    const md = L.filter((o) => o.root === MKR && o.op === 'delete')
    const mc = [...new Set(MP.filter((o) => o.root === PROPS[MKR]).map((o) => o.key))].filter((k) => !mi.some((o) => o.key === k))
    const parts: string[] = []
    if (chg.length) parts.push(plural(chg.length, 'clip') + ' changed')
    if (ins.length) parts.push(plural(ins.length, 'clip') + ' added')
    if (del.length) parts.push(plural(del.length, 'clip') + ' removed')
    if (mi.length) parts.push(plural(mi.length, 'marker') + ' added')
    if (md.length) parts.push(plural(md.length, 'marker') + ' removed')
    if (mc.length) parts.push(plural(mc.length, 'marker') + ' changed')
    const first = M1.items[chg[0] || ins[0]]
    return out(parts.join(', ') || 'no change to the timeline', '', first ? { item: first.key, f: first.f0 } : null, plural(chg.length + ins.length + del.length, 'clip'))
  }
  const at = (it: Item): [string, string] => {
    const t = frameTc(f, it.key)
    return t ? [t[0], t[1] || (M1.tc(it.e) as string)] : [M1.tc(it.s) as string, M1.tc(it.e) as string]
  }
  const iIns = L.find((o) => o.root === UNIT && o.op === 'insert')
  if (iIns) {
    const it = M1.items[iIns.key]
    const p = at(it)[0]
    return out(`added ${it.name} at ${p} on ${lab(M1, it.track)}`, p, { item: it.key, f: it.f0 }, 'added')
  }
  const iDel = L.find((o) => o.root === UNIT && o.op === 'delete')
  if (iDel) {
    const it = M0.items[iDel.key]
    return out(`removed ${it.name} from ${lab(M0, it.track)}, leaving a gap`, M0.tc(it.s), { item: null, f: it.f0, gone: it }, 'removed, gap left')
  }
  const mIns = L.find((o) => o.root === MKR && o.op === 'insert')
  if (mIns) {
    const m = M1.markers.find((x) => x.key === mIns.key)!
    return out(`dropped a marker “${m.name}” at ${M1.tc(m.rec)}`, M1.tc(m.rec), { marker: m.key, f: m.f }, 'marker')
  }
  const mDel = L.find((o) => o.root === MKR && o.op === 'delete')
  if (mDel) {
    const m = M0.markers.find((x) => x.key === mDel.key)!
    return out(`removed the marker “${m.name}”`, m.rec != null ? M0.tc(m.rec) : '', { f: m.f }, 'marker removed')
  }
  const g = MP.find((o) => o.root === PROPS[UNIT] && o.field === VIEW.geometry)
  if (g) {
    const x = M0.items[g.key]
    const y = M1.items[g.key]
    const moved = MP.some((o) => o.key === g.key && o.field === VIEW.unit_container_field)
    const T = { item: y.key, f: y.f0 }
    const [st, en] = at(y)
    if (x.s === y.s && x.d !== y.d && !moved) {
      const df = y.f1 - x.f1
      return out(`${df < 0 ? 'trimmed' : 'extended'} the end of ${y.name} by ${M1.dur(df)}`, en, T, `${df < 0 ? 'trimmed' : 'extended'} ${df < 0 ? '−' : '+'}${M1.dur(df)}`)
    }
    if (x.e === y.e && x.s !== y.s && !moved) {
      const df = y.f0 - x.f0
      return out(`${df > 0 ? 'trimmed' : 'extended'} the start of ${y.name} by ${M1.dur(df)}`, st, T, `${df > 0 ? 'trimmed' : 'extended'} the start ${M1.dur(df)}`)
    }
    if (x.d === y.d || moved) {
      const df = y.f0 - x.f0
      return out(`moved ${y.name} to ${st}${moved ? ' on ' + lab(M1, y.track) : ''}`, st, T, `moved ${df >= 0 ? '+' : '−'}${M1.dur(df)}`)
    }
    return out(`changed where ${y.name} sits and how long it runs`, st, T, 'moved and trimmed')
  }
  const nm = MP.find((o) => o.root === PROPS[UNIT] && o.field === 'name')
  if (nm) {
    const y = M1.items[nm.key]
    return out(`renamed ${nm.before} to ${nm.after}`, at(y)[0], { item: y.key, f: y.f0 }, 'renamed')
  }
  const off = MP.find((o) => o.root === PROPS[UNIT] && o.field === VIEW.hidden)
  if (off) {
    const y = M1.items[off.key]
    return out(`turned ${off.after === '1' ? 'off' : 'on'} ${y.name}`, at(y)[0], { item: y.key, f: y.f0 }, off.after === '1' ? 'turned off' : 'turned on')
  }
  const tn = MP.find((o) => o.root === PROPS[CONT] && o.field === 'name')
  if (tn) return out(`renamed the track ${tn.before} to ${tn.after}`, '', null, 'renamed')
  const mk = MP.find((o) => o.root === PROPS[MKR])
  if (mk) {
    const m = M1.markers.find((x) => x.key === mk.key)
    return out(`changed the marker “${m ? m.name : mk.key}”`, m && m.rec != null ? M1.tc(m.rec) : '', m ? { marker: m.key, f: m.f } : null, 'marker')
  }
  return out('changed the timeline', '', null)
}

/** A refused verb's word ("couldn’t trim …"). */
export const VERB_WORD: Record<string, string> = { 'clip trim': 'trim', 'clip move': 'move', 'clip rename': 'rename', 'clip delete': 'remove', 'clip add': 'add', 'clip enable': 'turn on or off', 'marker add': 'mark' }

/** A refused verb in plain words: who, what, which clip, why. The code only in the tooltip. */
export function refusalPhrase(r: Refusal, M: TimelineModel, claims?: Claim[]) {
  const a = actorOf(r.actor)
  const h = r.holder ? actorOf(r.holder) : null
  const it = M.items[r.unit]
  const name = it ? it.name : 'a clip'
  const verb = VERB_WORD[r.verb] || 'change'
  const why = h ? `${h.name} is holding it` : `${PRODUCT_NAME} didn’t allow it`
  const c = (claims || []).find((x) => x.id === r.claim)
  const text = `couldn’t ${verb} ${name}: ${why}`
  return {
    who: a.name,
    verb,
    clip: name,
    holder: h ? h.name : null,
    text,
    line: `${a.name} ${text}`,
    sub: `${h && c && c.note ? `${h.name}’s hold: “${c.note}”. ` : ''}Nothing changed.`,
    tip: `${r.code} · ${r.verb} ${r.unit}${r.claim ? ' · ' + r.claim : ''}`,
    target: it ? { item: it.key, f: it.f0 } : null,
  }
}

export const copyLabel = (n: unknown): string => String(n || '').replace(/\.[a-z0-9]+$/i, '')

/** An outside save's notice in two lines (kit c23d2349). */
export function saveLine(o: OutsideSaveEvent): { l1: string; l2: string } {
  if (o.outcome === 'kept_both') return { l1: `Saved outside ${PRODUCT_NAME}: that save is now live.`, l2: `What was live before is kept as a copy: ${copyLabel(o.copy && o.copy.name)}` }
  if (o.outcome === 'ingested') return { l1: `Saved outside ${PRODUCT_NAME} · taken in`, l2: o._summary || 'The changes show as an outside change' }
  if (o.outcome === 'declined') return { l1: `Saved outside ${PRODUCT_NAME}, and the file can’t be live any more`, l2: `${o.message || `${PRODUCT_NAME} can’t watch this save live.`} Edits are paused.` }
  if (o.outcome === 'missing') return { l1: 'The file was deleted or moved', l2: 'The live version is kept. Save it again to keep working on it.' }
  return { l1: `Saved outside ${PRODUCT_NAME}`, l2: '' }
}

/** Track renames in a frame, in words (the clip wording doesn't cover tracks). */
export const trackRenames = (f: TimelineFrame): string =>
  f.ops
    .filter((o) => o.root === PROPS[CONT] && o.kind === 'map' && o.field === 'name' && o.before != null)
    .map((o) => `the ${o.before} track renamed ${o.after}`)
    .join(', ')

/** What a hold covers: clips by count, a track by its name. */
export function heldWhat(M: TimelineModel, keys: string[]): string {
  const t = keys.filter((k) => /^@trk:/.test(k))
  if (t.length) return t.map((k) => (M.byT[k] ? M.byT[k].title : k)).join(', ')
  return plural(keys.length, 'clip')
}

/** A hold the kit ended because an outside save changed what it held (claim_ended how: gone_outside_save), named as it was. */
export function holdGoneText(M: TimelineModel, e: { actor: string; keys?: string[]; claim?: Claim | null; was?: string | null }): string {
  const a = actorOf(e.actor)
  const k = (e.keys || e.claim?.keys || [])[0] || ''
  const was = e.claim && e.claim.note_track ? e.claim.note_track : e.was || heldWhat(M, [k])
  return `${a.name}’s hold on ${was} ended: the ${/^@trk:/.test(k) ? 'track' : 'clip'} was renamed outside ${PRODUCT_NAME}`
}

/** Where someone is, in words: the clip under their playhead and its lane ("Interview B · V1"), else a gap. */
export function placeOf(M: TimelineModel, f: number, track: string | null | undefined, facts: MediaFacts): string {
  const t = track ? M.byT[track] : undefined
  const it = t ? visibleItems(t, f, f + 1e-6).find((x) => x.kind !== 'transition' && x.f0 <= f && f < x.f1) : pictureAt(M, f, null, facts).it || null
  return it ? `${it.name}${t ? ' · ' + t.label : ''}` : t ? `a gap on ${t.label}` : 'a gap'
}

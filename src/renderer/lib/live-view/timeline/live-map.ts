// What a real timeline doc's wire gives, in the view's own shapes (T7),
// pure: the kit client's PresencePeer as a sim Peer (its playhead from its
// time position), a ClaimView as a Claim, the kit's refusal at open as the
// card's words, the facts a real file has before K2 Render (every video
// source "preparing", never footage), and the sequence a say row's time
// is in. The real source (keyed-source.ts) folds through these and the
// sim's own fold, so the lanes, Watching and Changes read one state.

import type { ClaimView, PresencePeer } from '@k2/mpc-client'
import { lanesOf, type TimelineModel } from './fold'
import type { MediaFacts, PreviewState } from './picture'
import { DEFAULT_PV } from './picture'
import { tpPtr } from './playheads'
import type { Claim } from './schema'
import type { Peer } from './sim'

/** A roster peer as the view's Peer: its playhead from its time position (ticks), its view by address. */
export function peerOf(p: PresencePeer): Peer {
  const tp = p.timePosition ? { ...p.timePosition } : null
  return {
    handle: p.handle,
    name: p.name,
    kind: p.kind === 'agent' ? 'agent' : 'person',
    colour: p.colour || null,
    away: p.away,
    focus: (p.focus as Peer['focus']) ?? null,
    selection: (p.selection as Peer['selection']) ?? null,
    viewport: null,
    pointer: tpPtr(tp),
    timePosition: tp,
    viewAddress: p.viewAddress,
    isSelf: p.isSelf,
  }
}

/** A live claim as the view's Claim (`startedMs` is null from an older host: then it's as old as the doc). */
export function claimOf(c: ClaimView): Claim {
  return { id: c.claim, actor: { id: c.actor.id, name: c.actor.name, kind: c.actor.kind, colour: c.actor.colour ?? null }, root: c.root, keys: [...c.keys], note: c.note ?? '', hold: c.hold, started_ms: c.startedMs ?? 0, expires_ms: c.expiresMs }
}

/** Why a file can't be watched live, by the kit's refusal code at open: the timeline pack's `live_*` (rel/v0.1.0 live/model.rs, all five) and the kit's `unreadable`. */
export function declineWords(code: string, message?: string | null): { code: string; title: string; text: string; detail: string } {
  const W: Record<string, [string, string]> = {
    live_many_timelines: ['This file holds more than one timeline', 'K2 watches one timeline per file live. You can still open the saved file.'],
    live_time_off_tick: ['Some times fall between frames', 'A few clips start or end at times K2 can’t count exactly, so it won’t watch this file live. You can still open the saved file.'],
    live_too_large: ['Too large to watch live, for now', 'This timeline has more items than K2 watches live today. You can still open the saved file.'],
    live_time_range: ['A time in the file is out of range', 'K2 can’t count one of this timeline’s times, so it won’t watch it live. You can still open the saved file.'],
    live_structure: ['K2 can’t follow this timeline’s structure live', 'Something in the file’s tracks or clips isn’t one K2 watches live yet. You can still open the saved file.'],
    unreadable: ['K2 couldn’t read this file', 'It doesn’t read as a timeline K2 knows. You can still open the saved file.'],
  }
  const w = W[code]
  return w ? { code, title: w[0], text: w[1], detail: message || code } : { code, title: 'This file can’t be watched live', text: message ? `${message} You can still open the saved file.` : 'You can still open the saved file.', detail: message || code }
}

/** A real file's sources before K2 Render: each video source known as video, with no proxy, poster or filmstrip (so nothing is ever drawn). */
export function realFacts(M: TimelineModel): MediaFacts {
  const facts: MediaFacts = {}
  for (const t of lanesOf(M))
    if (t.kind === 'video')
      for (const it of t.items) {
        const md = it.media ? M.media[it.media] : undefined
        const file = md?.file
        if (file) facts[file] = { kind: 'video' }
      }
  return facts
}

/** What the monitor can honestly show on a real file: video still being prepared (nothing made yet), or no preview on this host. */
export function realPreview(M: TimelineModel, host: 'preparing' | 'not_on_this_host'): PreviewState {
  if (host === 'not_on_this_host') {
    const media = new Set<string>()
    for (const t of lanesOf(M)) if (t.kind === 'video') for (const it of t.items) if (it.media) media.add(it.media)
    return { ...DEFAULT_PV, player: 'cut', mode: 'pause', nopreview: [...media] }
  }
  return { ...DEFAULT_PV, player: 'cut', mode: 'pause', prep: { ready: [], cant: [] } }
}

/** The pack's derived key for a file's first timeline, when the doc names no sequence of its own. */
export const FIRST_SEQUENCE = '@d-seq:1'

/** The address a say row's time is in: the doc's own sequence key when its keyed state names one (\`timeline\` map, \`uid\`), else the pack's derived key for its first timeline. */
export function sequenceOf(M: TimelineModel): string {
  const uid = (M.B.maps.timeline?.uid ?? {})['']
  return typeof uid === 'string' && uid ? `@uid:${uid}` : FIRST_SEQUENCE
}

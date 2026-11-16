// The real source for a live timeline (T7): a doc socket whose welcome lists
// `keyed_watch`, with the kit client's KeyedWatch for the timeline's keyed
// state and its PresenceChannel for who's here (playheads by time position,
// views by address, holds). It folds what the wire says through the same
// fold the sample uses (sim.ts applyGroup), so the lanes, Watching, Changes
// and the notices read one state, and hands the shell a TimelineSnapshot.
//
// Honest by construction: it is `kind: 'live'` (no Sample data banner), its
// facts know each video source only as video (no proxy, poster or
// filmstrip until K2 Render), so the monitor says "Preparing video" (or,
// where the host can't decode, "This clip can't be previewed"), never draws
// sample footage, and the lanes draw no pictures or waveforms.
//
// The connection is the caller's (connect.ts): it hands each welcome in
// (`attach`, `reconnected`) and says when the socket is down.

import { KeyedWatch, PresenceChannel, type ClaimEnded, type ErrorReport, type OpsChangeFrame, type OutsideSaveFrame, type ServerFrame, type TimePositionSet, type Transport, type VerbRefusedFrame, type Welcome } from '@k2/mpc-client'
import type { LiveMessage, LiveShellSnapshot, LiveSource, SayResult } from '../shared/source'
import type { SayTime } from '../shared/say-time'
import { readSayTime } from '../shared/say-time'
import { putActor } from './actors'
import { bookFromKeyedState } from './adapter'
import { modelOf } from './fold'
import { claimOf, declineWords, peerOf, realFacts, realPreview, sequenceOf } from './live-map'
import { KEYED_ROOTS, type Book, type OutsideSaveEvent, type Refusal, type TimelineFrame } from './schema'
import { applyGroup, blankState, finish, type SimEvent, type SimState } from './sim'
import { timelineSnapshot, type TimelineSnapshot } from './snapshot'

/** The welcome as K2's docs socket sends it for a keyed doc: the kit's, with its features. */
export type KeyedWelcome = Welcome & { features?: string[]; rights?: string }

export interface KeyedSourceOptions {
  /** Sends a message (`POST /cli/docs/say`), with the view's address and time. */
  post: (body: { text: string; to: string; at?: string; time?: SayTime }) => Promise<unknown>
  /** The host clock now (ms); presence and changes read against it. */
  now?: () => number
}

const EMPTY_BOOK: Book = { rev: 0, lists: {}, hidden: {}, maps: {}, derived: { containers: {} } }

export class KeyedWatchTimelineSource implements LiveSource {
  readonly kind = 'live' as const
  watch: KeyedWatch | null = null
  presence: PresenceChannel | null = null
  private welcome: KeyedWelcome | null = null
  private S: SimState = blankState(EMPTY_BOOK, '')
  private i = 0
  private conn: 'reconnecting' | 'failed' | null = null
  private host: 'preparing' | 'not_on_this_host' = 'preparing'
  private snap: TimelineSnapshot | null = null
  private readonly subs = new Set<(s: LiveShellSnapshot) => void>()
  private offs: (() => void)[] = []
  private extra: LiveMessage[] = []
  private closed = false
  private seq = 0
  private onRetry: (() => void) | null = null
  private mine: { view: string | null; time: TimePositionSet | null } = { view: null, time: null }

  constructor(
    private readonly t: Transport,
    private readonly o: KeyedSourceOptions,
  ) {
    this.offs.push(t.onFrame((f) => this.onFrame(f)))
    // Until the welcome: the tab shows the file name and a spinner.
    this.S.loading = { phase: 'connect', upto: -1, roots: [], unitPages: [] }
    this.S.follow = false
  }

  /** The kit can't watch this doc live (`watch_refused`, or a socket refused at open): the can't-be-live card, in the pack's words. */
  refusedAtOpen(e: ErrorReport): void {
    this.group([{ kind: 'tier', tier: 'declined', decline: declineWords(refusalCode(e), e.message) }, { kind: 'loading', v: null }])
  }

  /** The first welcome on this socket. */
  attach(welcome: KeyedWelcome): void {
    this.welcome = welcome
    this.S = blankState(EMPTY_BOOK, welcome.self, { rights: welcome.rights ?? null })
    this.S.loading = { phase: 'welcome', upto: -1, roots: [], unitPages: [] }
    this.S.follow = false
    this.watch = new KeyedWatch(this.t, welcome)
    this.presence = new PresenceChannel(this.t, welcome)
    // Where you were before the welcome (the view mounted while connecting) goes out now.
    if (this.mine.view !== null) this.presence.setViewAddress(this.mine.view)
    if (this.mine.time !== null) this.presence.setTimePosition(this.mine.time)
    this.wire()
    this.changed()
  }

  /** A reconnect's welcome: the watch and presence pick up from it (the watch asks for a fresh snapshot). */
  reconnected(welcome: KeyedWelcome): void {
    this.welcome = welcome
    this.watch?.reconnected(welcome)
    this.presence?.reconnected(welcome)
    this.setConnection(null)
  }

  /** The socket is down: `reconnecting` while the caller retries, `failed` when it stopped. */
  setConnection(c: 'reconnecting' | 'failed' | null): void {
    if (this.conn === c) return
    this.conn = c
    this.group([{ kind: 'conn', state: c }])
  }

  /** What the host's preview renderer says for this doc (K2 Render): still preparing, or it can't decode here. */
  setPreviewHost(h: 'preparing' | 'not_on_this_host'): void {
    this.host = h
    this.changed()
  }

  /** The connection driver's retry (Retry now). */
  onRetryAsked(cb: () => void): void {
    this.onRetry = cb
  }

  snapshot(): LiveShellSnapshot {
    return (this.snap ??= this.build())
  }

  timeline(): TimelineSnapshot['timeline'] {
    return (this.snapshot() as TimelineSnapshot).timeline
  }

  subscribe(cb: (s: LiveShellSnapshot) => void): () => void {
    this.subs.add(cb)
    return () => this.subs.delete(cb)
  }

  retry(): void {
    this.onRetry?.()
  }

  async say(msg: { to: string; text: string; address: string | null; time?: SayTime | null }): Promise<SayResult> {
    try {
      await this.o.post({ text: msg.text, to: msg.to, ...(msg.address ? { at: msg.address } : {}), ...(msg.time ? { time: msg.time } : {}) })
      this.extra = [...this.extra, { id: `say-local-${++this.seq}`, from: this.S.self, to: msg.to, text: msg.text, at_ms: this.now(), address: msg.address ? { label: msg.address, address: msg.address } : null, ...(msg.time ? { time: msg.time } : {}) }]
      this.changed()
      return { ok: true }
    } catch (e) {
      const reason = e instanceof Error && e.message ? e.message : undefined
      return { ok: false, ...(reason ? { reason } : {}) }
    }
  }

  /** What you're looking at, for everyone else (kept until the presence channel opens, then sent). */
  setViewAddress(address: string | null): void {
    this.mine.view = address
    this.presence?.setViewAddress(address)
  }

  /** Your playhead, for everyone else (and for whoever follows you); kept until the presence channel opens. */
  setTimePosition(t: TimePositionSet | null): void {
    this.mine.time = t
    this.presence?.setTimePosition(t)
  }

  close(): void {
    if (this.closed) return
    this.closed = true
    for (const off of this.offs) off()
    this.offs = []
    this.watch?.close()
    this.presence?.close()
    this.subs.clear()
  }

  private now(): number {
    return (this.o.now ?? Date.now)()
  }

  private wire(): void {
    const w = this.watch as KeyedWatch
    const p = this.presence as PresenceChannel
    this.offs.push(
      w.onSnapshot(() => this.group([{ kind: 'book', B: this.book() }, { kind: 'loading', v: null }], false)),
      w.onChange((c) => this.onChange(c)),
      w.onRefused((e) => this.refusedAtOpen(e)),
      w.onVerbRefused((r) => this.onVerbRefused(r)),
      w.onOutsideSave((o) => this.onOutsideSave(o)),
      p.onChange(() => this.changed()),
      p.onClaims(() => this.onClaims()),
      p.onClaimEnded((e) => this.onClaimEnded(e)),
    )
  }

  /** The watched state as the view's book. */
  private book(): Book {
    const w = this.watch
    if (!w || !w.state) return this.S.B
    return bookFromKeyedState(w.state, w.rev ?? this.S.B.rev, (name, c) => w.containerDerived(name, c))
  }

  private onChange(c: OpsChangeFrame): void {
    putActor({ ...c.actor, colour: c.actor.colour ?? null })
    // The sim folds the frame for its log (the change's words, the agent's playhead); the kit's state stays the truth.
    this.group([{ kind: 'frame', frame: c as unknown as TimelineFrame }])
    this.S.B = this.book()
    this.changed()
  }

  private onVerbRefused(r: VerbRefusedFrame): void {
    putActor({ ...r.actor, colour: r.actor.colour ?? null })
    if (r.holder) putActor({ ...r.holder, colour: r.holder.colour ?? null })
    const ref: Refusal = { at_ms: r.at_ms, actor: { ...r.actor, colour: r.actor.colour ?? null }, client: r.client, turn: r.turn, verb: r.verb, unit: r.unit ?? '', code: r.code, holder: r.holder ? { ...r.holder, colour: r.holder.colour ?? null } : null, claim: r.claim }
    this.group([{ kind: 'refused', r: ref }])
  }

  private onOutsideSave(o: OutsideSaveFrame): void {
    const base = { _at_ms: o.at_ms }
    const save: OutsideSaveEvent =
      o.outcome === 'ingested'
        ? { outcome: 'ingested', rev: o.rev, ...base }
        : o.outcome === 'kept_both'
          ? { outcome: 'kept_both', rev: o.rev, kept: String(o.kept), copy: o.copy ?? { name: '', path: '' }, ...base }
          : o.outcome === 'declined'
            ? { outcome: 'declined', code: o.code, message: o.message, ...base }
            : { outcome: 'missing', ...base }
    this.group([{ kind: 'outside_save', save, B: this.book() } as SimEvent])
    if (o.outcome === 'declined') this.S.paused = { code: o.code, message: o.message }
    if (o.outcome === 'missing') this.S.missing = true
    this.changed()
  }

  /** New holds come in as claims started (a hold that ends comes through onClaimEnded). */
  private onClaims(): void {
    const have = new Set(this.S.claims.map((c) => c.id))
    const fresh = (this.presence?.claims() ?? []).filter((c) => !have.has(c.claim) && !c.cells)
    if (fresh.length) this.group(fresh.map((c) => ({ kind: 'claim_started', claim: claimOf(c) }) as SimEvent))
  }

  private onClaimEnded(e: ClaimEnded): void {
    this.group([{ kind: 'claim_ended', id: e.claim, how: e.how, at_ms: this.now(), actor: e.who, keys: e.keys }])
  }

  private onFrame(f: ServerFrame | { type: string }): void {
    if (this.closed || f.type !== 'say') return
    const s = f as unknown as { id?: unknown; from?: { id?: unknown }; to?: { id?: unknown } | null; text?: unknown; at?: unknown; time?: unknown; at_ms?: unknown }
    if (typeof s.id !== 'string' || typeof s.from?.id !== 'string' || typeof s.text !== 'string') return
    const time = readSayTime(s.time)
    const at = typeof s.at === 'string' && s.at ? s.at : null
    this.extra = [...this.extra, { id: s.id, from: s.from.id, to: typeof s.to?.id === 'string' ? s.to.id : this.S.self, text: s.text, at_ms: typeof s.at_ms === 'number' ? s.at_ms : this.now(), address: at ? { label: at, address: at } : null, ...(time ? { time } : {}) }]
    this.changed()
  }

  /** Fold one group of events as the sim does (a new step), then rebuild. */
  private group(events: SimEvent[], step = true): void {
    if (step) this.i += 1
    this.S.at = this.now()
    applyGroup(this.S, { at: this.S.at, label: null, events, what: '' }, this.i, { roots: KEYED_ROOTS })
    this.changed()
  }

  private changed(): void {
    if (this.closed) return
    this.snap = this.build()
    for (const cb of this.subs) cb(this.snap)
  }

  private build(): TimelineSnapshot {
    const S = this.S
    S.at = this.now()
    // Presence is the roster's, as it stands now.
    if (this.presence) {
      const peers: SimState['peers'] = {}
      for (const pp of this.presence.peers()) {
        // One row a person: this connection's for you, else the one that moved its playhead last.
        const had = peers[pp.handle]
        const later = (pp.timePosition?.at_ms ?? -1) > (had?.timePosition?.at_ms ?? -1)
        if (!had || pp.isSelf || (!had.isSelf && later)) peers[pp.handle] = peerOf(pp)
        if (!pp.isSelf) putActor({ id: pp.handle, name: pp.name, kind: pp.kind, colour: pp.colour || null })
      }
      if (S.self && !peers[S.self]) peers[S.self] = { ...peerOf({ handle: S.self, name: 'You', colour: '', kind: 'person', rawKind: 'person', isSelf: true, sameUser: true, selection: null, pointer: null, viewport: null, focus: null, range: null, cells: null, viewAddress: null, timePosition: null, away: false } as never) }
      S.peers = peers
    }
    finish(S)
    const M = modelOf(S.B)
    S.pv = realPreview(M, this.host)
    const snap = timelineSnapshot(S, { facts: realFacts(M), sample: false, scenario: 'live' })
    const f = this.welcome?.features ?? []
    snap.features = { viewAddress: f.includes('view_address'), timePosition: f.includes('time_position'), say: true }
    snap.timeline.sequence = sequenceOf(M)
    if (this.extra.length) snap.messages = [...snap.messages, ...this.extra].sort((a, b) => a.at_ms - b.at_ms)
    return snap
  }
}

/** The kit's refusal at open, as the pack's code (`live_too_large`…): the report's
 * `details.code` (k2-mpc-base errors/map.rs `declined`, `.with("code", d.code)`), else one
 * named in its id, rule or message, else its id. */
export function refusalCode(e: { id: string; message: string; rule?: string; details?: Record<string, unknown> }): string {
  const code = e.details?.code
  if (typeof code === 'string' && code) return code
  if (/^live_|^unreadable$/.test(e.id)) return e.id
  if (e.rule && /^live_|^unreadable$/.test(e.rule)) return e.rule
  const m = /\b(live_[a-z_]+|unreadable)\b/.exec(e.message)
  return m ? m[1] : e.id
}

// The real source for a live 3D scene: a doc socket whose welcome lists
// `keyed_watch`, with the kit client's KeyedWatch for the scene's keyed
// state and its PresenceChannel for who's here (their selections, the view
// each looks through, their holds). What the wire says is folded through
// the same fold the sample uses (sim.ts: change frames, refusals, outside
// saves, holds, presence), so the picture, its overlays, Activity, People
// and holds, the outliner and Follow read one state, and the shell gets a
// SceneSnapshot.
//
// Honest by construction: it is `kind: 'live'` (no Sample data banner);
// only real people are shown; holds show only when the host takes claims
// (the welcome's `claims` feature), never made up. A mesh edit or swap (a
// unit's derived `mesh_hash` moves) asks the picture for new stills.
//
// The connection is the caller's (shared/live-connect.ts): it hands each
// welcome in (`attach`, `reconnected`) and says when the socket is down.

import { KeyedWatch, PresenceChannel, type ClaimEnded, type ClaimView, type ErrorReport, type OpsChangeFrame, type OutsideSaveFrame, type PresencePeer, type Transport, type VerbRefusedFrame, type Welcome } from '@k2/mpc-client'
import type { LiveMessage, LiveShellSnapshot, LiveSource, SayResult } from '../shared/source'
import { docFromWatch, meshesChanged, normFrame, withDerived } from './keyed-doc'
import { sceneSnapshot, type SceneControl, type SceneSnapshot } from './sample-source'
import { applyGroup, blankState, finish, type Claim, type Peer, type SimEvent, type SimState } from './sim'
import type { Scenario } from './turn'
import type { Actor, Doc, Frame, Refusal } from './types'
import { SCENE_VIEW } from './view-address'
import { ACTORS } from './wording'

/** The welcome as K2's docs socket sends it for a keyed doc: the kit's, with its features. */
export type KeyedWelcome = Welcome & { features?: string[]; rights?: string }

export interface KeyedSceneOptions {
  /** Sends a message (`POST /cli/docs/say`) with the view's address. */
  post: (body: { text: string; to: string; at?: string }) => Promise<unknown>
  /** The file's name, as the header shows it. */
  fileName: string
  /** The host clock now (ms). */
  now?: () => number
}

const EMPTY: Doc = { rev: 0, lists: {}, kinds: {}, props: {}, scene: {}, units: {}, space: null, tier: 'live', removed: {}, added: {} }
/** The live tab has one "step": now. */
const LIVE: Scenario = { id: 'live', group: '', title: '', steps: [{ label: '', events: [] } as unknown as Scenario['steps'][number]], base: () => blankState(EMPTY, '') }

/** A person or agent as the scene's actors read them (their colour is the roster's). */
function putActor(a: { id: string; name: string; kind: string; colour: string }): Actor {
  const x: Actor = { id: a.id, name: a.name || a.id, kind: a.kind === 'agent' ? 'agent' : a.kind === 'file' ? 'file' : 'person', colour: a.colour || '#888' }
  ACTORS[a.id] = x
  return x
}

/** An actor a frame names: kept as the roster has them when it does (a frame may spell the name its own way). */
function knowActor(a: { id: string; name: string; kind: string; colour: string }): Actor {
  return ACTORS[a.id] ?? putActor(a)
}

/** A refusal's code: the pack's own when the kit put it in the error's details, else the kit's. */
export function refusalCode(e: ErrorReport): string {
  const d = (e as { details?: { code?: unknown } }).details
  return typeof d?.code === 'string' ? d.code : e.id
}

export class KeyedWatchSceneSource implements LiveSource {
  readonly kind = 'live' as const
  watch: KeyedWatch | null = null
  presence: PresenceChannel | null = null
  private S: SimState = blankState(EMPTY, '')
  private i = 0
  private snap: SceneSnapshot | null = null
  private readonly subs = new Set<(s: LiveShellSnapshot) => void>()
  private offs: (() => void)[] = []
  private extra: LiveMessage[] = []
  private closed = false
  private seq = 0
  private onRetry: (() => void) | null = null
  private myView: string | null = null
  private claimsOn = false
  /** Moves when a unit's mesh changes: the picture asks for new stills. */
  meshEpoch = 0
  /** The pack's refusal at open, when the watch can't be live. */
  refusedCode: string | null = null
  readonly control: SceneControl & { viewChanged: (v: string | null) => void } = {
    retryPicture: () => undefined,
    viewChanged: (v) => this.setViewAddress(v),
  }

  constructor(
    private readonly t: Transport,
    private readonly o: KeyedSceneOptions,
  ) {
    this.S.loading = { phase: 'connect' }
    this.S.follow = false
  }

  /** The kit can't watch this doc live (`watch_refused`, or a socket refused at open): the saved version, read-only, said in the scene's words. */
  refusedAtOpen(e: ErrorReport): void {
    this.refusedCode = refusalCode(e)
    this.group([
      { kind: 'tier', tier: 'blob' },
      { kind: 'loading', v: null },
    ])
  }

  /** The first welcome on this socket. */
  attach(welcome: KeyedWelcome): void {
    this.S = blankState(EMPTY, welcome.self)
    this.S.rights = welcome.rights ?? this.S.rights
    this.S.loading = { phase: 'welcome' }
    this.S.follow = false
    this.watch = new KeyedWatch(this.t, welcome)
    this.presence = new PresenceChannel(this.t, welcome)
    // Holds only when the host takes claims: never made up.
    this.claimsOn = (welcome.features ?? []).includes('claims') || this.presence.claimsSupported
    if (this.myView !== null) this.presence.setViewAddress(this.myView)
    this.wire()
    this.changed()
  }

  /** A reconnect's welcome: the watch and presence pick up from it (the watch asks for a fresh snapshot). */
  reconnected(welcome: KeyedWelcome): void {
    this.watch?.reconnected(welcome)
    this.presence?.reconnected(welcome)
    this.setConnection(null)
  }

  /** The socket is down: `reconnecting` while the caller retries, `failed` when it stopped. */
  setConnection(c: 'reconnecting' | 'failed' | null): void {
    this.group([{ kind: 'conn', v: c === 'reconnecting' ? 'lost' : c === 'failed' ? 'gaveup' : 'ok' }])
  }

  onRetryAsked(cb: () => void): void {
    this.onRetry = cb
  }

  snapshot(): LiveShellSnapshot {
    return (this.snap ??= this.build())
  }

  subscribe(cb: (s: LiveShellSnapshot) => void): () => void {
    this.subs.add(cb)
    return () => this.subs.delete(cb)
  }

  retry(): void {
    this.onRetry?.()
  }

  async say(msg: { to: string; text: string; address: string | null }): Promise<SayResult> {
    try {
      await this.o.post({ text: msg.text, to: msg.to, ...(msg.address ? { at: msg.address } : {}) })
      this.extra = [...this.extra, { id: `say-local-${++this.seq}`, from: this.S.self, to: msg.to, text: msg.text, at_ms: this.now(), address: msg.address ? { label: msg.address, address: msg.address } : null }]
      this.changed()
      return { ok: true }
    } catch (e) {
      const reason = e instanceof Error && e.message ? e.message : undefined
      return { ok: false, ...(reason ? { reason } : {}) }
    }
  }

  /** The view you look through, for everyone else (and whoever follows you); kept until presence opens. */
  setViewAddress(address: string | null): void {
    if (address === this.myView) return
    this.myView = address
    this.presence?.setViewAddress(address)
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

  /** The state as the scene reads it (tests). */
  state(): SimState {
    return this.S
  }

  private now(): number {
    return (this.o.now ?? Date.now)()
  }

  private wire(): void {
    const w = this.watch as KeyedWatch
    const p = this.presence as PresenceChannel
    this.offs.push(
      w.onSnapshot(() => this.onSnapshot()),
      w.onChange((c) => this.onChange(c)),
      w.onResync(() => this.group([{ kind: 'loading', v: { phase: 'resync' } }])),
      w.onRefused((e) => this.refusedAtOpen(e)),
      w.onVerbRefused((r) => this.group([{ kind: 'refused', r: this.refusal(r) }])),
      w.onOutsideSave((o) => this.onOutsideSave(o)),
      p.onChange((peers) => this.onPeers(peers)),
      p.onClaims((cs) => this.onClaims(cs)),
      p.onClaimEnded((e) => this.onClaimEnded(e)),
    )
  }

  private onSnapshot(): void {
    const D = this.watch ? docFromWatch(this.watch) : null
    if (!D) return
    const before = this.S.D
    this.S.D = D
    this.S.revDocs = { [D.rev]: D }
    if (before !== EMPTY && meshesChanged(before, D).length) this.meshEpoch++
    this.group([{ kind: 'loading', v: null }])
  }

  private onChange(c: OpsChangeFrame): void {
    const f = normFrame(c as unknown as Frame)
    knowActor(f.actor)
    const before = this.S.D
    this.group([{ kind: 'frame', frame: f }], false)
    // The frame names only what changed; the derived values and the scene drawn are the watch's.
    if (this.watch) withDerived(this.S.D, this.watch)
    if (meshesChanged(before, this.S.D).length) this.meshEpoch++
    this.changed()
  }

  private refusal(r: VerbRefusedFrame): Refusal {
    const x = r as unknown as Refusal
    knowActor(x.actor)
    const holder = (r as unknown as { holder?: Actor | string | null }).holder
    if (holder && typeof holder === 'object') knowActor(holder)
    // The scene keeps a holder by id.
    return { ...x, holder: holder && typeof holder === 'object' ? holder.id : (holder ?? null) } as Refusal
  }

  private onOutsideSave(o: OutsideSaveFrame): void {
    this.group([{ kind: 'settled', v: o }])
  }

  private onPeers(peers: readonly PresencePeer[]): void {
    const next: Record<string, Peer> = {}
    for (const p of peers) {
      if (next[p.handle]) continue
      putActor({ id: p.handle, name: p.name, kind: p.kind, colour: p.colour })
      next[p.handle] = {
        handle: p.handle,
        name: p.name,
        kind: p.kind === 'agent' ? 'agent' : 'person',
        colour: p.colour,
        away: p.away,
        focus: p.focus ? { root: p.focus.root, key: (p.focus as { key?: string }).key ?? '' } : null,
        selection: p.selection ? { root: p.selection.root, keys: [...p.selection.keys] } : null,
        view: p.isSelf ? (this.myView ?? SCENE_VIEW) : (p.viewAddress ?? SCENE_VIEW),
      }
    }
    if (this.S.self && !next[this.S.self]) {
      const me = ACTORS[this.S.self]
      next[this.S.self] = { handle: this.S.self, name: me?.name ?? 'You', kind: 'person', colour: me?.colour ?? '#888', away: false, focus: null, selection: null, view: this.myView ?? SCENE_VIEW }
    }
    this.S.peers = next
    this.changed()
  }

  private onClaims(cs: readonly ClaimView[]): void {
    if (!this.claimsOn) return
    const have = new Set(this.S.claims.map((c) => c.id))
    const fresh = cs.filter((c) => !have.has(c.claim)).map(
      (c): SimEvent => ({ kind: 'claim_started', claim: { id: c.claim, actor: putActor({ id: c.actor.id, name: c.actor.name, kind: c.actor.kind, colour: c.actor.colour }), root: c.root, keys: [...c.keys], label: c.note ?? '', hold: c.hold, started_ms: c.startedMs, expires_ms: c.expiresMs } as Claim }),
    )
    if (fresh.length) this.group(fresh)
  }

  private onClaimEnded(e: ClaimEnded): void {
    if (!this.claimsOn || !this.S.claims.some((c) => c.id === e.claim)) return
    this.group([{ kind: 'claim_ended', id: e.claim, how: e.how, at_ms: this.now(), at_rev: this.S.D.rev, actor: e.who }])
  }

  /** One step of what happened, folded as the sample folds a step. */
  private group(events: SimEvent[], notify = true): void {
    this.i += 1
    applyGroup(this.S, { at: this.now(), events } as unknown as Parameters<typeof applyGroup>[1], this.i)
    if (notify) this.changed()
  }

  private changed(): void {
    if (this.closed) return
    this.snap = null
    const s = this.snapshot()
    for (const cb of this.subs) cb(s)
  }

  private build(): SceneSnapshot {
    // A new state object each time: the views memoise on it (the sample hands a new one each step).
    const S = finish({ ...this.S, peers: { ...this.S.peers }, claims: [...this.S.claims], log: [...this.S.log] })
    const snap = sceneSnapshot(S, LIVE, 0, this.control)
    const messages = [...snap.messages, ...this.extra].sort((a, b) => a.at_ms - b.at_ms)
    return { ...snap, size: null, messages, scene: { ...snap.scene, fileName: this.o.fileName, scenario: 'live', label: '', meshEpoch: this.meshEpoch } }
  }
}

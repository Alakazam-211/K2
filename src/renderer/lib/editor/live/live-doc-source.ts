// LiveDocSource (build-editors.md §4, E4a): the shell's LiveSource for a live
// text doc, the first kit-backed one. It reads the kit's editor
// (components/LiveEditor: state, roster, rights, change, pending, note,
// closed) and the socket's own frames through the transport tap (the
// welcome's `features` and `access_owner`, `outside_save`, `verb_refused`),
// and keeps what the editor's header, panel, notices and status line show:
//
//   - who's here and where (the section their caret is in), you first;
//   - finished edits: a run of one person's changes that goes quiet for
//     FINISH_MS is one edit. It becomes a Changes row, a Live edits line and
//     one announcement ("Atlas edited Setup"), never keystrokes;
//   - the connection, in the shell's words: reconnecting, trying (Retry),
//     can't reconnect (after CANT_AFTER_MS), offline (the computer is), back;
//     offline edits are counted once, as finished edits of yours made while
//     disconnected, and that one number is used everywhere;
//   - the last outside save, refusal and removed-text note;
//   - the text before the last outside save ("before external edit"), for
//     Compare and Undo their save.
//
// Until the kit's next round, ChangeInfo carries only `hunks`, `by` and
// `local` (no version, kind, turn, verb or at_ms): rows say "Atlas edited
// Setup", and Undo works only on your own newest edit, through the server's
// per-person undo (⌘Z).
//
// Presence on the text (E4b): each other person's presence colour (slot),
// their holds from the kit's claims (a hold names what they're doing in the
// sentence and the Here list), and Say something: `say()` posts
// `docs/say` and the socket's `say` frames are Messages.

import type { RosterEntry } from '@k2/mpc-client'
import type { ConnectionState as KitState, LiveEditorEvents, RemovedTextNote } from '@/components/LiveEditor'
import type { LivePerson } from '@/lib/live-view/shared/people'
import type { ConnectionState, OutsideSave } from '@/lib/live-view/shared/notices'
import type { LiveChange, LiveMessage, LiveShellSnapshot, LiveSource, SayResult } from '@/lib/live-view/shared/source'
import { holdLabel, holdsReducer, pruneHolds, type Hold } from './holds'
import { assignSlots, type Slot } from './presence-slots'
import { placeAddr, resolveAddr } from './addr'
import { addressLabel, sayFailure } from './say-words'
import { caretLine, editWords, lineAtByte, placeAt, sectionAt } from './sections'
import type { TappedWelcome, TransportTap } from './transport-tap'
import { readSayTime } from '@/lib/live-view/shared/say-time'

/** The kit's participant for a save made outside K2 (`EXTERNAL_PID`, session/participant.rs). */
export const EXTERNAL_PID = 'kit:external'

/** One person's changes that went quiet this long are one finished edit. */
export const FINISH_MS = 1500
/** Reconnecting this long reads as "can't reconnect". */
export const CANT_AFTER_MS = 30_000
/** Retry shows "Trying now" this long. */
export const TRYING_MS = 4000
/** "Back online" stays this long (or until dismissed). */
export const BACK_MS = 12_000
/** A finished edit or a refusal stays the header's sentence this long. */
export const SENTENCE_MS = 10_000

/** The part of LiveEditorHandle the source reads (tests script it). */
export interface LiveDocEditor {
  readonly text: string
  readonly state: KitState
  readonly rights: string
  readonly roster: readonly RosterEntry[]
  readonly pending: number
  on<K extends keyof LiveEditorEvents>(event: K, cb: (v: LiveEditorEvents[K]) => void): () => void
  undo(): void
  redo(): void
  reconnectNow(): void
}

export interface LiveDocTimers {
  set(f: () => void, ms: number): unknown
  clear(h: unknown): void
  now(): number
}

const realTimers: LiveDocTimers = {
  set: (f, ms) => setTimeout(f, ms),
  clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
  now: () => Date.now(),
}

export interface LiveDocClose {
  reason: string
  code?: string
  path?: string
  message?: string
}

/** One finished edit by someone else (Live edits). */
export interface LiveEdit {
  id: string
  who: string
  /** After the name: "edited Setup". */
  text: string
  at_ms: number
  lines: { first: number; last: number }
}

export interface LiveDocSnapshot extends LiveShellSnapshot {
  rights: string | null
  /** Who can grant edit access (`access_owner.name`); null: "the file's owner". */
  accessOwner: string | null
  /** Your finished edits made while disconnected (one count, used everywhere). */
  offlineEdits: number
  /** Set while "Back online" shows: how many offline edits synced. */
  synced: number | null
  /** Finished edits by others, oldest first. */
  liveEdits: LiveEdit[]
  /** The newest finished edit by someone else, once (the live region). */
  announce: { id: string; text: string } | null
  /** An outside save removed your text. */
  note: RemovedTextNote | null
  /** The editor closed for good (core off, idle, removed…). */
  closed: LiveDocClose | null
  /** The text as it was before the last outside save, as seen on this connection. */
  beforeExternal: string | null
  /** The stored version from before the last outside save (`before_version`): Compare and
   *  Undo their save read it (`docs/show`) when this connection didn't see the text itself
   *  (a tab that only got the save replayed after its welcome). Null for missing and declined. */
  beforeVersion: number | null
  /** Why Undo and Redo are off now, or null. */
  undoOff: string | null
  /** Others here (not away, not you): the Live chip and panel. */
  othersHere: number
  /** Your changes not yet acknowledged by the server. */
  pending: number
  /** Each other person's presence colour (1..4), kept while they're here. */
  slots: ReadonlyMap<string, Slot>
  /** Others' holds and claims on lines (never yours), oldest first. */
  holds: Hold[]
  /** Each person's caret line (1-based), or null: the flags and the off-screen band. */
  lines: ReadonlyMap<string, number | null>
  /** Where each person is, as a kit address (`@h:<slug>` or `L12`): what a message to them points at. */
  addrs: ReadonlyMap<string, string | null>
}

interface Run {
  who: string
  first: number
  last: number
  started: number
  at: number
  timer: unknown
  offline: boolean
}

type Frame = Record<string, unknown>

const kindOf = (k: string): LivePerson['kind'] => (k === 'agent' ? 'agent' : 'person')

export class LiveDocSource implements LiveSource {
  readonly kind = 'live' as const
  private readonly subs = new Set<(s: LiveShellSnapshot) => void>()
  private readonly timers: LiveDocTimers
  private readonly markdown: boolean
  private editor: LiveDocEditor | null = null
  private offs: (() => void)[] = []
  private tapOffs: (() => void)[] = []
  private snap: LiveDocSnapshot

  private kit: KitState = 'connecting'
  private everLive = false
  private self = ''
  private roster: readonly RosterEntry[] = []
  private text = ''
  private features: string[] = []
  private online = true
  private retrying = false
  private retried = false
  private cant = false
  private back = false
  private wasDown = false
  private caughtUp = 0
  private connTimer: unknown = null
  private backTimer: unknown = null
  private runs = new Map<string, Run>()
  private changes: LiveChange[] = []
  private liveEdits: LiveEdit[] = []
  private offlineEdits = 0
  private offlineRow: LiveChange | null = null
  private ignoreLocalUntil = 0
  private sentence: { who: string; text: string; until: number } | null = null
  private seq = 0
  private pending = 0
  private slots: ReadonlyMap<string, Slot> = new Map()
  private holds: Hold[] = []
  private messages: LiveMessage[] = []
  /** The last outside save handled (its at_ms): a reconnect without resume replays it. */
  private lastSaveAt: number | null = null
  /** An external change came on this connection since the last outside save: its text-before is this save's. */
  private externalSinceSave = false
  private readonly post: ((body: Record<string, unknown>) => Promise<unknown>) | null

  constructor(opts: { markdown: boolean; timers?: LiveDocTimers; online?: boolean; post?: (body: Record<string, unknown>) => Promise<unknown> }) {
    this.markdown = opts.markdown
    this.post = opts.post ?? null
    this.timers = opts.timers ?? realTimers
    this.online = opts.online ?? (typeof navigator === 'undefined' ? true : navigator.onLine !== false)
    this.snap = this.build({})
  }

  // ---- wiring ----

  /** The socket's own frames (welcome extras, outside_save, verb_refused). */
  attachTap(tap: TransportTap): void {
    for (const off of this.tapOffs) off()
    this.tapOffs = [tap.onWelcome((w) => this.onWelcome(w)), tap.onFrame((f) => this.onFrame(f))]
  }

  /** The kit's editor (a Reopen attaches a new one; Changes stay). */
  attach(ed: LiveDocEditor): void {
    for (const off of this.offs) off()
    this.editor = ed
    this.kit = ed.state
    this.text = ed.text
    this.roster = ed.roster
    this.offs = [
      ed.on('state', (s) => this.onState(s)),
      ed.on('roster', (r) => {
        this.roster = r
        this.emit({})
      }),
      ed.on('rights', (r) => this.emit({ rights: r })),
      ed.on('change', (c) => this.onChange(c)),
      ed.on('note', (n) => this.emit({ note: n })),
      ed.on('claim', (c) => {
        this.holds = holdsReducer(this.holds, c, { self: this.self, people: this.roster.map((e) => ({ id: e.handle, name: e.name, kind: e.kind, self: !!e.self })), slot: (id) => this.slotOf(id) })
        this.emit({})
      }),
      ed.on('pending', (n) => {
        this.pending = n
        this.emit({})
      }),
      ed.on('closed', (c) => {
        const m = (c as { message?: unknown }).message
        this.emit({ closed: { reason: c.reason, ...(c.code ? { code: c.code } : {}), ...(c.path ? { path: c.path } : {}), ...(typeof m === 'string' && m.trim() ? { message: m } : {}) } })
      }),
    ]
    this.emit({ closed: null })
  }

  /** The computer went online or offline (window events). */
  setOnline(online: boolean): void {
    this.online = online
    this.emit({})
  }

  // ---- LiveSource ----

  snapshot(): LiveDocSnapshot {
    return this.snap
  }

  subscribe(cb: (s: LiveShellSnapshot) => void): () => void {
    this.subs.add(cb)
    return () => void this.subs.delete(cb)
  }

  retry(): void {
    if (!this.editor || this.kit !== 'reconnecting') return
    this.retrying = true
    this.editor.reconnectNow()
    this.emit({})
    this.timers.set(() => {
      this.retrying = false
      if (this.kit === 'reconnecting' && this.cant) this.retried = true
      this.emit({})
    }, TRYING_MS)
  }

  /** "Say something": `docs/say {text, to, at}`; the sent line joins Messages at once. */
  async say(msg: { to: string; text: string; address: string | null }): Promise<SayResult> {
    if (!this.post) return { ok: false }
    try {
      const r = (await this.post({ text: msg.text, to: msg.to, ...(msg.address ? { at: msg.address } : {}) })) as { say?: { id?: unknown; at_ms?: unknown } } | null
      const id = typeof r?.say?.id === 'string' ? r.say.id : `say-${++this.seq}`
      const at = typeof r?.say?.at_ms === 'number' ? r.say.at_ms : this.timers.now()
      this.addMessage({ id, from: this.self, to: msg.to, text: msg.text, at_ms: at, address: msg.address ? { label: this.addrLabel(msg.address), address: msg.address } : null })
      return { ok: true }
    } catch (e) {
      return { ok: false, ...sayFailure(e, msg.to, (id) => this.roster.find((x) => x.handle === id)?.name ?? null) }
    }
  }

  private addMessage(m: LiveMessage): void {
    if (this.messages.some((x) => x.id === m.id)) return
    this.messages = [...this.messages, m]
    this.emit({})
  }

  close(): void {
    for (const off of [...this.offs, ...this.tapOffs]) off()
    this.offs = []
    this.tapOffs = []
    for (const r of this.runs.values()) this.timers.clear(r.timer)
    this.timers.clear(this.connTimer)
    this.timers.clear(this.backTimer)
    this.subs.clear()
  }

  // ---- actions the panel and notices take ----

  /** Undo a row of yours (the server's per-person undo: only your newest). */
  undoChange(id: string): void {
    const c = this.changes.find((x) => x.id === id)
    if (!c || !this.editor || this.snap.undoOff) return
    this.ignoreLocalUntil = this.timers.now() + FINISH_MS
    this.editor.undo()
    c.undone = true
    this.emit({})
  }

  redoChange(id: string): void {
    const c = this.changes.find((x) => x.id === id)
    if (!c || !this.editor || this.snap.undoOff) return
    this.ignoreLocalUntil = this.timers.now() + FINISH_MS
    this.editor.redo()
    c.undone = false
    this.emit({})
  }

  dismissBack(): void {
    this.back = false
    this.emit({})
  }

  /** Saved again after a save outside K2 removed the file or paused live edits. */
  clearOutsideSave(): void {
    this.emit({ outsideSave: null })
  }

  /** The removed-text note was put away. */
  dismissNote(): void {
    this.emit({ note: null })
  }

  // ---- events ----

  private onWelcome(w: TappedWelcome): void {
    this.self = w.self
    this.features = Array.isArray(w.features) ? w.features : []
    this.emit({ rights: w.rights, accessOwner: w.access_owner?.name?.trim() || null })
  }

  private onFrame(f: Frame): void {
    if (f.type === 'outside_save') this.onOutsideSave(f)
    else if (f.type === 'verb_refused') this.onRefused(f)
    else if (f.type === 'say') this.onSay(f)
  }

  private onSay(f: Frame): void {
    const from = (f.from ?? {}) as { id?: unknown }
    const to = (f.to ?? null) as { id?: unknown } | null
    if (typeof f.id !== 'string' || typeof from.id !== 'string' || typeof f.text !== 'string') return
    const at = typeof f.at === 'string' && f.at ? f.at : null
    const time = readSayTime(f.time)
    this.addMessage({ id: f.id, from: from.id, to: typeof to?.id === 'string' ? to.id : '', text: f.text, at_ms: typeof f.at_ms === 'number' ? f.at_ms : this.timers.now(), address: at ? { label: this.addrLabel(at), address: at } : null, ...(time ? { time } : {}) })
  }

  /** A message's address in words for its chip: the heading's own text, "line 12", else as it came. */
  private addrLabel(address: string): string {
    return resolveAddr(this.text, address)?.label ?? addressLabel(address)
  }

  private slotOf(id: string): Slot {
    const s = this.slots.get(id)
    if (s) return s
    this.slots = assignSlots(this.slots, [...this.roster.map((e) => ({ id: e.handle, self: !!e.self || e.handle === this.self })), { id }])
    return this.slots.get(id) ?? 1
  }

  /** What a hold says its holder is doing, after the name ("rewriting Setup"). */
  private holdDoing(id: string): string | null {
    const h = this.holds.find((x) => x.who === id)
    return h ? holdLabel(h, sectionAt(this.text, h.first, this.markdown)).slice(h.name.length + 4) : null
  }

  private onState(s: KitState): void {
    const prev = this.kit
    this.kit = s
    if (s === 'reconnecting' && prev !== 'reconnecting') {
      this.wasDown = true
      this.cant = false
      this.retried = false
      this.timers.clear(this.connTimer)
      this.connTimer = this.timers.set(() => {
        this.cant = true
        this.emit({})
      }, CANT_AFTER_MS)
    }
    if (s === 'live') {
      this.timers.clear(this.connTimer)
      this.retrying = false
      this.cant = false
      this.everLive = true
      if (this.wasDown) {
        this.wasDown = false
        this.back = true
        this.caughtUp = 0
        this.timers.clear(this.backTimer)
        this.backTimer = this.timers.set(() => this.dismissBack(), BACK_MS)
        if (this.offlineRow) this.offlineRow.text = `· ${this.offlineWords()} · synced`
      }
    }
    this.emit({ synced: this.back ? this.offlineEdits : null })
    if (s === 'live' && this.back) this.offlineEdits = 0
  }

  private offlineWords(): string {
    return `${this.offlineEdits} offline edit${this.offlineEdits === 1 ? '' : 's'}`
  }

  private disconnected(): boolean {
    return this.kit === 'reconnecting'
  }

  private onChange(c: LiveEditorEvents['change']): void {
    const before = this.text
    this.text = this.editor?.text ?? this.text
    const now = this.timers.now()
    // An outside save (the kit's external participant): keep the text from before it.
    if (!c.local && c.by === EXTERNAL_PID) {
      this.externalSinceSave = true
      this.emit({ beforeExternal: before })
      return
    }
    // A snapshot or a rejection moving the text: nobody's edit.
    if (!c.local && c.by === null) {
      this.emit({})
      return
    }
    if (c.local && now < this.ignoreLocalUntil) {
      this.emit({})
      return
    }
    const who = c.local ? this.self : (c.by as string)
    if (!who) return
    const h = c.hunks[0]
    let run = this.runs.get(who)
    // The line once per burst: the first change, then at most every 300 ms.
    const line = h && (!run || now - run.at > 300 || run.first === 0) ? lineAtByte(this.text, h.start) : null
    if (!run) {
      const offline = c.local && this.disconnected()
      run = { who, first: line ?? 1, last: line ?? 1, started: now, at: now, timer: null, offline }
      this.runs.set(who, run)
      if (!c.local && this.back) this.caughtUp++
      if (offline) this.countOffline(now)
    } else if (line !== null) {
      run.first = Math.min(run.first, line)
      run.last = Math.max(run.last, line)
    }
    run.at = now
    this.timers.clear(run.timer)
    const r = run
    run.timer = this.timers.set(() => this.finish(r), FINISH_MS)
    this.emit({})
  }

  private countOffline(now: number): void {
    this.offlineEdits++
    if (!this.offlineRow || this.offlineRow.text.endsWith('synced')) {
      this.offlineRow = { id: `offline-${++this.seq}`, who: this.self, verb: 'edited', text: '', at_ms: now, target: null }
      this.changes.push(this.offlineRow)
    }
    this.offlineRow.text = `· ${this.offlineWords()} · waiting to sync`
  }

  private finish(r: Run): void {
    this.runs.delete(r.who)
    const own = r.who === this.self
    // Offline edits are one row, counted (countOffline).
    if (r.offline) return this.emit({})
    const text = editWords(this.text, r, this.markdown)
    const id = `edit-${++this.seq}`
    this.changes.push({ id, who: r.who, verb: 'edited', text, at_ms: r.at, target: `lines:${r.first}-${r.last}`, undoable: own })
    if (own) return this.emit({})
    const edit: LiveEdit = { id, who: r.who, text, at_ms: r.at, lines: { first: r.first, last: r.last } }
    this.liveEdits.push(edit)
    this.sentence = { who: r.who, text, until: r.at + SENTENCE_MS }
    this.emit({ announce: { id, text: `${this.nameOf(r.who)} ${text}` } })
  }

  private onOutsideSave(f: Frame): void {
    const at = typeof f.at_ms === 'number' ? f.at_ms : this.timers.now()
    // The same save again (replayed to a reconnect without resume): already handled.
    if (typeof f.at_ms === 'number' && f.at_ms === this.lastSaveAt) return
    const outcome = f.outcome
    let save: OutsideSave
    if (outcome === 'ingested' && typeof f.rev === 'number') save = { outcome, rev: f.rev }
    else if (outcome === 'missing') save = { outcome }
    else if (outcome === 'declined') save = { outcome, code: String(f.code ?? ''), message: String(f.message ?? '') }
    else return
    const words = outcome === 'ingested' ? '· merged' : outcome === 'missing' ? '· the file was deleted or moved' : '· live edits paused'
    this.lastSaveAt = at
    this.changes.push({ id: `save-${++this.seq}`, who: 'file', verb: 'saved', text: words, at_ms: at, target: null, tone: 'outside' })
    const bv = outcome === 'ingested' && typeof f.before_version === 'number' ? f.before_version : null
    // The text seen here is this save's only when its external change came on this connection.
    const seen = this.externalSinceSave
    this.externalSinceSave = false
    this.emit({ outsideSave: { ...save, at_ms: at }, beforeVersion: bv, ...(seen ? {} : { beforeExternal: null }) })
  }

  private onRefused(f: Frame): void {
    const actor = (f.actor ?? {}) as { id?: string; name?: string }
    const holder = (f.holder ?? null) as { id?: string; name?: string } | null
    const at = typeof f.at_ms === 'number' ? f.at_ms : this.timers.now()
    const verb = String(f.verb ?? 'edit')
    // A text unit has no address: the holder's place names it ("Configuration").
    const held = holder?.id ? this.placeOf(holder.id) : null
    const target = typeof f.unit === 'string' && f.unit ? f.unit : (held ?? '')
    const holderName = holder?.name ?? null
    const tail = `${target ? ` ${target}` : ''}${holderName ? `: ${holderName} is holding it` : ''}`
    const actorId = actor.id && this.roster.some((e) => e.handle === actor.id) ? actor.id : (actor.name ?? 'Someone')
    const text = `couldn’t ${verb}${tail}`
    this.changes.push({ id: `refused-${++this.seq}`, who: actorId, verb, text, at_ms: at, target: null, tone: 'refused', ...(holder?.id && holderName ? { ask: { id: holder.id, name: holderName } } : {}) })
    this.sentence = { who: actorId, text, until: this.timers.now() + SENTENCE_MS }
    this.emit({ refusal: { actor: actor.name ?? 'Someone', verb, target, holder: holderName, at_ms: at } })
  }

  // ---- the snapshot ----

  private nameOf(id: string): string {
    return this.roster.find((e) => e.handle === id)?.name ?? id
  }

  private placeOf(id: string): string | null {
    const e = this.roster.find((x) => x.handle === id)
    return e ? placeAt(this.text, caretLine(this.text, e.cursor), this.markdown) : null
  }

  private connection(): ConnectionState | null {
    if (this.kit === 'reconnecting') {
      if (!this.online) return 'offline'
      if (this.retrying) return 'trying'
      return this.cant ? 'cant' : 'reconnecting'
    }
    return this.kit === 'live' && this.back ? 'back' : null
  }

  private person(e: RosterEntry, down: boolean): LivePerson {
    const self = !!e.self || e.handle === this.self
    const place = placeAt(this.text, caretLine(this.text, e.cursor), this.markdown)
    const run = this.runs.get(e.handle)
    const runPlace = run ? placeAt(this.text, run.first, this.markdown) : null
    const agent = e.kind === 'agent'
    const held = !down && !e.away ? this.holdDoing(e.handle) : null
    const doing = held
      ? held
      : down
      ? place
        ? `last seen in ${place}`
        : 'last seen'
      : e.away
        ? place
          ? `was in ${place}`
          : 'not in the file now'
        : run
          ? `${self ? 'writing in' : 'editing'} ${runPlace ?? place ?? 'the file'}`
          : place
            ? `${agent || self ? 'in' : 'reading'} ${place}`
            : agent
              ? 'in the file, not editing'
              : 'viewing'
    const presence = self ? undefined : this.slots.get(e.handle)
    return { id: e.handle, name: e.name, kind: kindOf(e.kind), colour: e.colour, away: e.away, self, place, doing, editing: !!run && !self, viewAddress: e.view_address ?? null, ...(presence ? { presence } : {}) }
  }

  private emit(patch: Partial<LiveDocSnapshot>): void {
    // The editor's text now (a welcome or a snapshot sets it without a change).
    if (this.editor) this.text = this.editor.text
    this.snap = this.build(patch)
    for (const cb of [...this.subs]) cb(this.snap)
  }

  private build(patch: Partial<LiveDocSnapshot>): LiveDocSnapshot {
    const prev = this.snap as LiveDocSnapshot | undefined
    const now = this.timers.now()
    const connection = this.connection()
    const down = connection !== null && connection !== 'back'
    const present = this.roster.filter((e) => e.handle !== EXTERNAL_PID)
    const lines = new Map(present.map((e) => [e.handle, e.cursor ? caretLine(this.text, e.cursor) : null]))
    this.slots = assignSlots(this.slots, present.map((e) => ({ id: e.handle, self: !!e.self || e.handle === this.self })))
    this.holds = pruneHolds(this.holds, present.map((e) => e.handle))
    const people = present.map((e) => this.person(e, down))
    const othersHere = people.filter((p) => !p.self && !p.away).length
    // The header's sentence: someone editing now, else the last finished edit or refusal.
    const open = [...this.runs.values()].filter((r) => r.who !== this.self && !r.offline).sort((a, b) => b.at - a.at)[0]
    const recent = this.sentence && this.sentence.until > now ? this.sentence : null
    // A hold names what its holder is doing ("is rewriting Setup").
    const holding = this.holds.find((h) => !open || h.who === open.who)
    const openWords = open ? (open.who === holding?.who ? this.holdDoing(open.who) : null) : null
    const activity = down
      ? null
      : open
        ? { who: open.who, text: `is ${openWords ?? `editing ${placeAt(this.text, open.first, this.markdown) ?? 'the file'}`}` }
        : recent
          ? { who: recent.who, text: recent.text }
          : holding
            ? { who: holding.who, text: `is ${this.holdDoing(holding.who) ?? 'working'}` }
            : null
    const outsideSave = patch.outsideSave !== undefined ? patch.outsideSave : (prev?.outsideSave ?? null)
    const paused = outsideSave?.outcome === 'declined' ? 'Live edits are paused until the file is text again' : outsideSave?.outcome === 'missing' ? 'Live edits are off until the file is back' : null
    // Undo: your newest own edit (the server's per-person undo), or Redo on the one you undid.
    const ownRows = this.changes.filter((c) => c.who === this.self && c.verb === 'edited' && c.target !== null)
    const newest = ownRows[ownRows.length - 1]
    const changes = this.changes.map((c) => (c.undoable || c.undone ? { ...c, undoable: c === newest } : c))
    const agent = people.find((p) => p.kind === 'agent' && !p.away && !p.self)?.id ?? null
    return {
      tier: this.everLive ? 'live' : 'loading',
      connection,
      caughtUp: this.caughtUp,
      retried: this.retried,
      self: this.self,
      people,
      agent,
      agentDone: false,
      activity,
      changes,
      messages: this.messages,
      outsideSave,
      refusal: patch.refusal !== undefined ? patch.refusal : (prev?.refusal ?? null),
      features: { viewAddress: this.features.includes('view_address'), timePosition: false, say: this.post !== null },
      clock: now,
      rights: patch.rights !== undefined ? patch.rights : (prev?.rights ?? null),
      accessOwner: patch.accessOwner !== undefined ? patch.accessOwner : (prev?.accessOwner ?? null),
      offlineEdits: this.offlineEdits,
      synced: patch.synced !== undefined ? patch.synced : this.back ? (prev?.synced ?? null) : null,
      liveEdits: [...this.liveEdits],
      announce: patch.announce !== undefined ? patch.announce : (prev?.announce ?? null),
      note: patch.note !== undefined ? patch.note : (prev?.note ?? null),
      closed: patch.closed !== undefined ? patch.closed : (prev?.closed ?? null),
      beforeExternal: patch.beforeExternal !== undefined ? patch.beforeExternal : (prev?.beforeExternal ?? null),
      beforeVersion: patch.beforeVersion !== undefined ? patch.beforeVersion : (prev?.beforeVersion ?? null),
      undoOff: down ? 'Back online to use this' : paused,
      othersHere,
      pending: this.pending,
      slots: this.slots,
      holds: this.holds,
      lines,
      addrs: new Map([...lines].map(([id, l]) => [id, l === null ? null : placeAddr(this.text, l, this.markdown)])),
    }
  }
}

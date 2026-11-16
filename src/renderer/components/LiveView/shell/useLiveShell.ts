// The live shell's logic over one LiveSource (build-editors.md §4): the
// snapshot, the tab's width and the panel, whom you follow, the say field,
// the notices and their dismissals, Changes without the save its notice is
// saying, the off-screen edges. LiveShellView lays it out for the scene and
// the timeline; the editor (components/Editor) uses it with its own layout.
// The view's state only: the data stays in the source.

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react'
import { FOLLOW_IDLE, followReducer, followView, type FollowAction, type FollowState } from '@/lib/live-view/shared/follow'
import { noticeView, pickNotice, type LiveNotice, type NoticeActionId } from '@/lib/live-view/shared/notices'
import { personById } from '@/lib/live-view/shared/people'
import { SAY_IDLE, sayReducer } from '@/lib/live-view/shared/say'
import { isDisconnected, liveTabState } from '@/lib/live-view/shared/state'
import type { LiveShellSnapshot, LiveSource } from '@/lib/live-view/shared/source'
import type { SampleSnapshot } from '@/lib/live-view/shared/samples'
import type { Edge } from './EdgeStack'
import type { SayAt, ShellConfig } from './shell-config'
import { useLiveEntrySwitch } from '../LiveEntrySwitch'

export function useSnapshot(source: LiveSource): LiveShellSnapshot {
  const [snap, setSnap] = useState(() => source.snapshot())
  useEffect(() => {
    setSnap(source.snapshot())
    return source.subscribe(setSnap)
  }, [source])
  return snap
}

/** The tab's width: `given` (tests, samples), else measured. */
export function useTabWidth(given: number | undefined): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement>(null)
  const [w, setW] = useState(given ?? 1440)
  useEffect(() => {
    if (given !== undefined || typeof ResizeObserver === 'undefined' || !ref.current) return
    const ro = new ResizeObserver((es) => setW(Math.round(es[0].contentRect.width)))
    ro.observe(ref.current)
    return () => ro.disconnect()
  }, [given])
  return [ref, given ?? w]
}

const noticeKey = (n: LiveNotice): string => (n.kind === 'connection' ? `c:${n.state}` : n.kind === 'outside_save' ? `s:${n.save.outcome}` : `r:${n.actor}:${n.verb}:${n.target}`)

export function useLiveShell(source: LiveSource, config: ShellConfig, width?: number) {
  const snap = useSnapshot(source)
  const [root, tabWidth] = useTabWidth(width)
  const narrow = tabWidth < config.narrowBelow
  const [panelOpen, setPanelOpen] = useState(!narrow)
  useEffect(() => setPanelOpen(!narrow), [narrow])

  const people = snap.people
  const [follow, dispatchFollow] = useReducer((s: FollowState, a: FollowAction) => followReducer(s, a, people), FOLLOW_IDLE)
  const fv = followView(follow, { people, agent: snap.agent, agentDone: snap.agentDone })
  const followingId = follow.kind === 'following' ? follow.who : null

  // The person you follow leaving: "<name> left the file" for 4 s.
  const names = useRef(new Map<string, string>())
  useEffect(() => {
    for (const p of people) names.current.set(p.id, p.name)
    if ((follow.kind === 'following' || follow.kind === 'away') && !personById(people, follow.who)) dispatchFollow({ type: 'left', who: follow.who, name: names.current.get(follow.who) ?? 'They', now: Date.now() })
  }, [people, follow])
  useEffect(() => {
    if (follow.kind !== 'left') return
    const t = setTimeout(() => dispatchFollow({ type: 'tick', now: Date.now() }), Math.max(0, follow.until - Date.now()))
    return () => clearTimeout(t)
  }, [follow])

  // Say: to whom you follow, else whom you chose to message (the editors' "Message Atlas"), else the agent.
  const [messageTo, setMessageTo] = useState<string | null>(null)
  const sayTo = personById(people, followingId ?? messageTo ?? snap.agent)
  const [say, dispatchSay] = useReducer(sayReducer, SAY_IDLE)
  // What the view says the say row points at (the timeline's playhead), else the config's.
  const [sayAt, setSayAt] = useState<SayAt | null>(null)
  const seen = useRef(snap.messages.length)
  useEffect(() => {
    const fresh = snap.messages.slice(seen.current)
    seen.current = snap.messages.length
    const reply = [...fresh].reverse().find((m) => m.to === snap.self)
    // The line names who said it, whoever the row speaks to next.
    if (reply) dispatchSay({ type: 'reply', text: reply.text, name: personById(snap.people, reply.from)?.name })
  }, [snap.messages, snap.self]) // eslint-disable-line react-hooks/exhaustive-deps
  const send = useCallback(
    (again: boolean) => {
      if (!sayTo) return
      const text = say.draft.trim()
      dispatchSay({ type: again ? 'send-again' : 'send' })
      const address = sayAt ?? config.sayAddress(snap, sayTo.id)
      void source.say({ to: sayTo.id, text, address: address?.address ?? null, time: address?.time ?? null }).then((r) => dispatchSay(r.ok ? { type: 'sent', name: sayTo.name } : { type: 'failed', reason: r.reason }))
    },
    [sayTo, say.draft, source, snap, config, sayAt],
  )

  // One notice at a time, connection first.
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set())
  useEffect(() => setDismissed(new Set()), [snap.outsideSave, snap.refusal, snap.connection])
  const connection = snap.connection ? { kind: 'connection' as const, state: snap.connection, changes: snap.caughtUp, retried: snap.retried, following: followingId ? personById(people, followingId)?.name : null } : null
  const notices: LiveNotice[] = []
  if (snap.outsideSave) notices.push({ kind: 'outside_save', save: snap.outsideSave })
  const r = snap.refusal
  const refused = r ? { kind: 'refused' as const, actor: r.actor, verb: r.verb, target: r.target, holder: r.holder, ...(r.why !== undefined ? { why: r.why } : {}), ...(r.ask !== undefined ? { ask: r.ask } : {}), ...(r.address !== undefined ? { address: r.address } : {}) } : null
  // A view that draws the refusal on its picture gets it there; the bar doesn't say it too.
  if (refused && !config.refusalOnPicture) notices.push(refused)
  const notice = pickNotice({ connection, notices, dismissed: (n) => dismissed.has(noticeKey(n)) })
  // Open the saved file: the tab's File | Live switch goes to File, which shows the file as saved.
  const entry = useLiveEntrySwitch()
  const onNotice = (id: NoticeActionId, n: LiveNotice | null): void => {
    if (id === 'retry') source.retry()
    else if (id === 'open-saved-file') entry?.onMode('file')
    else if (id === 'dismiss' && n) setDismissed((d) => new Set(d).add(noticeKey(n)))
    else if (id === 'ask' && n?.kind === 'refused' && n.ask) {
      // Ask: the say row, to whom the refusal names, about what was refused.
      setMessageTo(n.ask.id)
      if (n.address) setSayAt({ label: n.address.label, title: n.address.label, address: n.address.address })
    }
    config.onNoticeAction?.(id, n)
  }
  const heroRefusal = refused && config.refusalOnPicture && !isDisconnected(snap.connection) && !dismissed.has(noticeKey(refused)) ? { notice: refused, view: noticeView(refused, { fileName: '' }), onAction: (id: NoticeActionId) => onNotice(id, refused) } : null

  const offline = isDisconnected(snap.connection)
  const tabState = liveTabState({ tier: snap.tier, connection: snap.connection, outsideSave: snap.outsideSave, behind: snap.behind })
  const live = snap.tier === 'live'
  const clock = snap.clock ?? Date.now()
  // Changes doesn't repeat the outside save its notice is saying.
  const showingSave = notice?.kind === 'outside_save' ? snap.outsideSave : null
  const changes = useMemo(() => snap.changes.filter((c) => !(showingSave && c.tone === 'outside' && c.at_ms === showingSave.at_ms)), [snap.changes, showingSave])

  const [jumped, setJumped] = useState<Edge | null>(null)
  const onJump = (side: Edge): void => {
    setJumped(side)
    dispatchFollow({ type: 'look-away' })
  }
  const offscreen = ((snap as SampleSnapshot).hints?.offscreen ?? []).map((o) => ({ side: o.side, people: o.who.filter((id) => id !== followingId).flatMap((id) => personById(people, id) ?? []) }))

  const activityWho = snap.activity?.who ? (personById(people, snap.activity.who) ?? null) : null
  const sentence = snap.activity ? { who: activityWho, text: snap.activity.text } : null

  // Your own row says whom you follow.
  const followedName = followingId ? personById(people, followingId)?.name : null
  const panelPeople = useMemo(() => people.map((p) => (p.self ? { ...p, doing: followedName ? `following ${followedName}` : (p.doing ?? 'viewing') } : p)), [people, followedName])

  const showSay = live && !!sayTo && (config.sayAlways || followingId !== null || messageTo !== null)
  const sayAddress = sayTo ? (sayAt ?? config.sayAddress(snap, sayTo.id)) : null

  return {
    snap,
    root,
    narrow,
    panelOpen,
    setPanelOpen,
    people,
    panelPeople,
    follow: fv,
    followState: follow,
    followingId,
    dispatchFollow,
    setSayAt,
    sayTo,
    messageTo,
    setMessageTo,
    say,
    dispatchSay,
    send,
    showSay,
    sayAddress,
    notice,
    onNotice,
    heroRefusal,
    offline,
    tabState,
    live,
    clock,
    changes,
    jumped,
    onJump,
    offscreen,
    sentence,
  }
}

export type LiveShell = ReturnType<typeof useLiveShell>

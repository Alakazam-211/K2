// A live tab on the shared shell: reads one LiveSource and hands the shell's
// presentational pieces what it says. Keeps the view's own state (whom you
// follow, the say field, dismissed notices, the panel) and nothing else:
// the data stays in the source. The scene and the timeline differ only by
// their ShellConfig (scene/scene-shell.ts, timeline/timeline-shell.ts),
// which later phases grow with the picture and the lanes.

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react'
import { FOLLOW_IDLE, followReducer, followView, type FollowAction, type FollowState } from '@/lib/live-view/shared/follow'
import { pickNotice, type LiveNotice, type NoticeActionId } from '@/lib/live-view/shared/notices'
import { personById } from '@/lib/live-view/shared/people'
import { SAY_IDLE, sayReducer } from '@/lib/live-view/shared/say'
import { isDisconnected, liveTabState } from '@/lib/live-view/shared/state'
import type { LiveShellSnapshot, LiveSource } from '@/lib/live-view/shared/source'
import type { SampleSnapshot } from '@/lib/live-view/shared/samples'
import { SampleShellSource } from '@/lib/live-view/shared/sample-shell'
import { AvatarStack } from './AvatarStack'
import { ChangesList } from './ChangesList'
import type { Edge } from './EdgeStack'
import { BackButton, FollowControl } from './FollowControl'
import { LiveHeader } from './LiveHeader'
import { LiveTabFrame } from './LiveTabFrame'
import { MessagesList } from './MessagesList'
import { NoticeBar } from './NoticeBar'
import { SayBar } from './SayBar'
import type { ShellConfig } from './shell-config'
import { ShellSampleBanner } from './ShellSampleBanner'
import { WatchingList } from './WatchingList'

function useSnapshot(source: LiveSource): LiveShellSnapshot {
  const [snap, setSnap] = useState(() => source.snapshot())
  useEffect(() => {
    setSnap(source.snapshot())
    return source.subscribe(setSnap)
  }, [source])
  return snap
}

function useTabWidth(given: number | undefined): [React.RefObject<HTMLDivElement | null>, number] {
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

export function LiveShellView({ source, config, width, fileName }: { source: LiveSource; config: ShellConfig; width?: number; fileName?: string }): React.JSX.Element {
  const snap = useSnapshot(source)
  const sample = source instanceof SampleShellSource ? source : null
  const name = fileName ?? sample?.sample.fileName ?? ''
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

  // Say: to whom you follow, else the agent.
  const sayTo = personById(people, followingId ?? snap.agent)
  const [say, dispatchSay] = useReducer(sayReducer, SAY_IDLE)
  const seen = useRef(snap.messages.length)
  useEffect(() => {
    const fresh = snap.messages.slice(seen.current)
    seen.current = snap.messages.length
    const reply = [...fresh].reverse().find((m) => m.to === snap.self)
    if (reply) dispatchSay({ type: 'reply', text: reply.text })
  }, [snap.messages, snap.self])
  const send = useCallback(
    (again: boolean) => {
      if (!sayTo) return
      const text = say.draft.trim()
      dispatchSay({ type: again ? 'send-again' : 'send' })
      const address = config.sayAddress(snap, sayTo.id)
      void source.say({ to: sayTo.id, text, address: address?.address ?? null }).then((r) => dispatchSay({ type: r.ok ? 'sent' : 'failed' }))
    },
    [sayTo, say.draft, source, snap, config],
  )

  // One notice at a time, connection first.
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(new Set())
  useEffect(() => setDismissed(new Set()), [snap.outsideSave, snap.refusal, snap.connection])
  const connection = snap.connection ? { kind: 'connection' as const, state: snap.connection, changes: snap.caughtUp, retried: snap.retried, following: followingId ? personById(people, followingId)?.name : null } : null
  const notices: LiveNotice[] = []
  if (snap.outsideSave) notices.push({ kind: 'outside_save', save: snap.outsideSave })
  if (snap.refusal) notices.push({ kind: 'refused', actor: snap.refusal.actor, verb: snap.refusal.verb, target: snap.refusal.target, holder: snap.refusal.holder })
  const notice = pickNotice({ connection, notices, dismissed: (n) => dismissed.has(noticeKey(n)) })
  const onNotice = (id: NoticeActionId, n: LiveNotice): void => {
    if (id === 'retry') source.retry()
    else if (id === 'dismiss') setDismissed((d) => new Set(d).add(noticeKey(n)))
    config.onNoticeAction?.(id, n)
  }

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
  const back = fv.control === 'back' && !config.backInHeader ? <BackButton label={fv.label} disabled={offline} onBack={() => dispatchFollow({ type: 'back' })} /> : null

  // Your own row says whom you follow.
  const followedName = followingId ? personById(people, followingId)?.name : null
  const panelPeople = useMemo(() => people.map((p) => (p.self ? { ...p, doing: followedName ? `following ${followedName}` : (p.doing ?? 'viewing') } : p)), [people, followedName])
  const panel = (
    <>
      {config.panel.map((s) =>
        s.id === 'watching' ? (
          <WatchingList key={s.id} title={s.title} people={panelPeople} followingId={followingId} stale={offline} onFollow={(id) => dispatchFollow({ type: 'follow', who: id })} />
        ) : s.id === 'messages' ? (
          <MessagesList key={s.id} title={s.title} messages={snap.messages} people={people} now={clock} />
        ) : (
          <ChangesList key={s.id} title={s.title} changes={changes} people={people} now={clock} onGoThere={() => dispatchFollow({ type: 'look-away' })} />
        ),
      )}
    </>
  )

  const showSay = live && !!sayTo && (config.sayAlways || followingId !== null)
  const sayAddress = sayTo ? config.sayAddress(snap, sayTo.id) : null
  const Hero = config.Hero
  return (
    <div ref={root} className="flex h-full min-h-0 flex-col">
      <LiveTabFrame
        sample={source.kind === 'sample'}
        narrow={narrow}
        panelOpen={panelOpen}
        onClosePanel={() => setPanelOpen(false)}
        header={
          <LiveHeader
            fileName={name}
            fileGlyph={config.fileGlyph}
            kindLabel={narrow ? undefined : config.kindLabel}
            state={tabState}
            sentence={live ? sentence : null}
            sentenceRow={narrow && config.sentenceRowWhenNarrow}
            panelOpen={panelOpen}
            onTogglePanel={() => setPanelOpen((o) => !o)}
            right={
              live ? (
                <>
                  <AvatarStack people={people} followingId={followingId} grey={offline} onFollow={(id) => dispatchFollow({ type: 'follow', who: id })} />
                  <FollowControl view={fv} backHere={config.backInHeader} disabled={offline} onFollow={(id) => dispatchFollow({ type: 'follow', who: id })} onStop={() => dispatchFollow({ type: 'stop' })} onBack={() => dispatchFollow({ type: 'back' })} />
                </>
              ) : null
            }
          />
        }
        banner={sample ? <ShellSampleBanner player={sample.player} line={config.bannerLine} /> : null}
        notice={live ? <NoticeBar notice={notice} ctx={{ fileName: name, kept: config.kept }} onAction={onNotice} /> : null}
        hero={<Hero snap={snap} state={tabState} offscreen={offscreen} onJump={onJump} jumped={jumped} back={back} narrow={narrow} />}
        foot={
          showSay && sayTo ? (
            <SayBar
              toName={sayTo.name}
              address={sayAddress ? { label: sayAddress.label, title: sayAddress.title } : null}
              state={say}
              offline={offline}
              paused={tabState === 'paused'}
              onDraft={(text) => dispatchSay({ type: 'draft', text })}
              onSend={() => send(false)}
              onSendAgain={() => send(true)}
            />
          ) : null
        }
        panel={panel}
      />
    </div>
  )
}

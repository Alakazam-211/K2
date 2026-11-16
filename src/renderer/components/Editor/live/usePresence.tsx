// Presence on a live text (build-editors.md E4b), the editor's side of it:
//   - name flags and hairlines, and the holds (tint, bar, label with Message,
//     the line under your caret), as CodeMirror extensions made once
//     (lib/editor/live/presence-layer.ts, hold-field.ts);
//   - the off-screen band at the editor's top or bottom edge, whose chips jump;
//   - Follow: an avatar, a Here row or "Follow Atlas" follows that person's
//     caret (or the heading they're viewing); "Following Atlas · Stop", and
//     any scroll or typing of yours, stops it; when they finish, "Atlas
//     finished. Stopped following." with Back to my place and Follow again;
//   - Say something to Atlas under the text while you follow (or after
//     Message on a hold), through `docs/say`;
//   - your own view address: the heading at the top of your view.
// The shell's hook keeps whom you follow and the say row; this keeps the rest.

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from 'react'
import { EditorView } from '@codemirror/view'
import type { Extension } from '@codemirror/state'
import { AvatarStack } from '@/components/LiveView/shell/AvatarStack'
import { FollowControl } from '@/components/LiveView/shell/FollowControl'
import { EdgeArrow, FollowGlyph } from '@/components/LiveView/shell/glyphs'
import { GIVE_UP } from '@/components/LiveView/shell/LiveHeader'
import { SayBar } from '@/components/LiveView/shell/SayBar'
import type { LiveShell } from '@/components/LiveView/shell/useLiveShell'
import { Button } from '@/components/ui'
import { TEXT_FOLLOW_IDLE, finishedWords, textFollowReducer } from '@/lib/editor/live/follow-text'
import { holdsExtension, setHolds } from '@/lib/editor/live/hold-field'
import type { Hold } from '@/lib/editor/live/holds'
import type { LiveDocSnapshot } from '@/lib/editor/live/live-doc-source'
import { NO_OFFSCREEN, type Offscreen } from '@/lib/editor/live/offscreen'
import { presenceChanged, presenceExtension, type PresenceHost, type PresencePerson } from '@/lib/editor/live/presence-layer'
import { caretSlotCss, slotColour } from '@/lib/editor/live/presence-slots'
import { headingAddress } from '@/lib/editor/live/say-words'
import { headingLine, sectionAt } from '@/lib/editor/live/sections'
import { personById } from '@/lib/live-view/shared/people'
import type { LiveDoc } from './useLiveDoc'
import { EdgeBand } from './EdgeBand'

/** Your view address goes this long after you stop scrolling. */
const VIEW_ADDRESS_MS = 600

export function usePresence({ doc, shell, snap, markdown, root }: { doc: LiveDoc; shell: LiveShell; snap: LiveDocSnapshot; markdown: boolean; root: React.RefObject<HTMLElement | null> }) {
  const view = (): EditorView | null => doc.editor()?.view ?? null

  // ---- what the CodeMirror side reads (one host, made once) ----
  const people = useMemo<PresencePerson[]>(
    () => snap.people.map((p) => ({ id: p.id, name: p.name, agent: p.kind === 'agent', slot: snap.slots.get(p.id) ?? 1, self: !!p.self, away: p.away, line: snap.lines.get(p.id) ?? null })),
    [snap.people, snap.slots, snap.lines],
  )
  const peopleRef = useRef(people)
  peopleRef.current = people
  const followingRef = useRef<string | null>(null)
  followingRef.current = shell.followingId
  const [off, setOff] = useState<Offscreen & { centre: number }>({ ...NO_OFFSCREEN, centre: 0 })
  const moveRef = useRef<() => void>(() => undefined)
  const topLineRef = useRef<(line: number) => void>(() => undefined)
  const messageRef = useRef<(h: Hold) => void>(() => undefined)
  const [host] = useState<PresenceHost>(() => ({
    people: () => peopleRef.current,
    following: () => followingRef.current,
    onOffscreen: (o) => setOff(o),
    onTopLine: (l) => topLineRef.current(l),
    onUserMove: () => moveRef.current(),
  }))
  const extension = useMemo<Extension>(
    () => [
      presenceExtension(host),
      holdsExtension({
        onMessage: (h) => messageRef.current(h),
        section: (st, line) => (markdown ? sectionAt(st.doc.sliceString(0, st.doc.line(Math.min(line, st.doc.lines)).to), line, true) : null),
      }),
    ],
    [host, markdown],
  )

  // The people changed: redraw the flags and sort the band again.
  useEffect(() => view()?.dispatch({ effects: presenceChanged.of(null) }), [people]) // eslint-disable-line react-hooks/exhaustive-deps
  // The holds, into the text (and again after a reload or a new mode's extensions).
  const holdsRef = useRef(snap.holds)
  holdsRef.current = snap.holds
  const syncHolds = useCallback(() => doc.editor()?.view.dispatch({ effects: setHolds.of(holdsRef.current) }), [doc])
  useEffect(() => syncHolds(), [snap.holds, syncHolds])
  useEffect(() => doc.editor()?.on('reload', () => syncHolds()), [doc, syncHolds, snap.tier])

  // ---- Follow ----
  const [tf, dispatchTf] = useReducer(textFollowReducer, TEXT_FOLLOW_IDLE)
  const tfRef = useRef(tf)
  tfRef.current = tf
  const top = (): number => view()?.scrollDOM.scrollTop ?? 0
  const follow = useCallback(
    (who: string) => {
      dispatchTf({ type: 'follow', who, home: view()?.scrollDOM.scrollTop ?? 0 })
      shell.dispatchFollow({ type: 'follow', who })
    },
    [shell.dispatchFollow], // eslint-disable-line react-hooks/exhaustive-deps
  )
  const stop = useCallback(() => {
    dispatchTf({ type: 'stop' })
    shell.dispatchFollow({ type: 'stop' })
  }, [shell.dispatchFollow]) // eslint-disable-line react-hooks/exhaustive-deps
  moveRef.current = () => {
    if (tfRef.current.kind !== 'following') return
    dispatchTf({ type: 'moved' })
    shell.dispatchFollow({ type: 'stop' })
  }
  // The shell saw them leave: its note says so; ours goes quiet.
  useEffect(() => {
    if (tf.kind === 'following' && shell.followingId !== tf.who) dispatchTf({ type: 'gone', who: tf.who })
  }, [tf, shell.followingId])

  // The view goes with them: the kit keeps their caret in view; with no caret, the heading they view.
  const followed = personById(snap.people, shell.followingId)
  const followedLine = followed ? (snap.lines.get(followed.id) ?? null) : null
  const followedHeading = followed?.viewAddress?.startsWith('heading:') ? decodeURIComponent(followed.viewAddress.slice(8)) : null
  useEffect(() => {
    const ed = doc.editor()
    if (!ed) return
    if (!shell.followingId) return ed.follow(null)
    if (followedLine !== null) {
      ed.follow(shell.followingId)
      const n = Math.min(followedLine, ed.view.state.doc.lines)
      ed.view.dispatch({ effects: EditorView.scrollIntoView(ed.view.state.doc.line(n).from, { y: 'center' }) })
      return
    }
    ed.follow(null)
    const line = followedHeading ? headingLine(ed.text, followedHeading) : null
    if (line !== null) ed.view.dispatch({ effects: EditorView.scrollIntoView(ed.view.state.doc.line(line).from, { y: 'start', yMargin: 24 }) })
  }, [doc, shell.followingId, followedLine === null, followedHeading]) // eslint-disable-line react-hooks/exhaustive-deps

  // They finished: their hold ended, or (an agent holding nothing) their edit finished.
  const prevHolds = useRef(snap.holds)
  const prevEdits = useRef(snap.liveEdits.length)
  useEffect(() => {
    const was = prevHolds.current
    const edits = snap.liveEdits.slice(prevEdits.current)
    prevHolds.current = snap.holds
    prevEdits.current = snap.liveEdits.length
    if (tf.kind !== 'following') return
    const who = personById(snap.people, tf.who)
    if (!who) return
    const holding = snap.holds.some((h) => h.who === tf.who)
    const released = was.some((h) => h.who === tf.who) && !holding
    const edited = who.kind === 'agent' && !holding && edits.some((e) => e.who === tf.who)
    if (released || edited) {
      dispatchTf({ type: 'finished', who: tf.who, name: who.name })
      shell.dispatchFollow({ type: 'stop' })
    }
  }, [snap.holds, snap.liveEdits]) // eslint-disable-line react-hooks/exhaustive-deps

  const fin = finishedWords(tf)
  const finishedWho = tf.kind === 'finished' ? (personById(snap.people, tf.who) ?? null) : null
  const back = (): void => {
    if (tf.kind !== 'finished') return
    const sc = view()?.scrollDOM
    if (sc) sc.scrollTop = tf.home
    dispatchTf({ type: 'back' })
  }
  const again = (): void => {
    if (tf.kind !== 'finished') return
    dispatchTf({ type: 'again', home: top() })
    shell.dispatchFollow({ type: 'follow', who: tf.who })
  }

  // ---- Say ----
  const focusSay = (): void => void setTimeout(() => root.current?.querySelector<HTMLInputElement>('[data-testid="lv-say-field"]')?.focus(), 0)
  messageRef.current = (h) => {
    shell.setMessageTo(h.who)
    focusSay()
  }

  // ---- your view address: the heading at the top of your view ----
  const textRef = useRef(() => doc.editor()?.text ?? '')
  const sent = useRef<string | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const canSend = snap.features.viewAddress && markdown
  topLineRef.current = (line) => {
    if (!canSend) return
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(() => {
      const section = sectionAt(textRef.current(), line, true)
      const address = section && section !== 'the intro' ? headingAddress(section) : null
      if (address === sent.current) return
      sent.current = address
      doc.sendViewAddress(address)
    }, VIEW_ADDRESS_MS)
  }
  useEffect(() => () => void (timer.current && clearTimeout(timer.current)), [])

  // ---- the band ----
  const jump = (id: string): void => {
    const ed = doc.editor()
    const line = snap.lines.get(id) ?? null
    if (!ed || line === null) return
    moveRef.current()
    const n = Math.min(line, ed.view.state.doc.lines)
    ed.view.dispatch({ effects: EditorView.scrollIntoView(ed.view.state.doc.line(n).from, { y: 'start', yMargin: Math.round(ed.view.scrollDOM.clientHeight / 3) }) })
  }
  const live = snap.tier === 'live' && !shell.offline && !snap.closed
  const band = (side: 'top' | 'bottom'): React.ReactNode => {
    if (!live) return null
    const ids = side === 'top' ? off.above : off.below
    return <EdgeBand side={side} people={ids.flatMap((id) => personById(snap.people, id) ?? [])} centre={off.centre} onJump={jump} />
  }

  // ---- the header: the stack (click to follow), Follow, and "finished" ----
  const withOthers = snap.people.some((p) => !p.self && (!p.away || shell.offline))
  const right = (
    <>
      {withOthers ? <AvatarStack people={snap.people} editor grey={shell.offline} followingId={shell.followingId} onFollow={shell.offline ? undefined : follow} /> : null}
      {fin ? null : <FollowControl view={shell.follow} disabled={shell.offline} onFollow={follow} onStop={stop} onBack={() => (shell.follow.target ? follow(shell.follow.target) : undefined)} />}
    </>
  )
  const sentence = fin && finishedWho ? { who: finishedWho, text: fin.text, short: fin.short } : null
  // As the header narrows the buttons give up their words first ("Back"), then all but their marks;
  // the sentence keeps its short form ("Atlas finished.") to the end. The full words stay as names.
  const finButton = (label: string, short: string, glyph: React.ReactNode, onClick: () => void, tight: number): React.ReactNode => (
    <Button size={tight >= GIVE_UP.shortWords ? 'sm' : 'md'} onClick={onClick} aria-label={label} title={label} className={`ed-finished-btn inline-flex items-center gap-1.5 text-[var(--color-text-primary)]${tight >= GIVE_UP.shortWords ? ' ed-finished-mark' : ''}`}>
      {tight >= GIVE_UP.words ? glyph : null}
      {tight >= GIVE_UP.shortWords ? null : tight >= GIVE_UP.words ? short : label}
    </Button>
  )
  const sentenceEnd = fin
    ? (tight: number) => (
        <span className="inline-flex flex-none items-center gap-1.5" data-testid="ed-follow-finished" data-tight={tight || undefined}>
          {finButton(fin.back, fin.backShort, <EdgeArrow side="left" />, back, tight)}
          {finButton(fin.again, fin.againShort, <FollowGlyph size={14} />, again, tight)}
        </span>
      )
    : null

  const sayAddress = shell.sayAddress
  const say =
    shell.showSay && shell.sayTo ? (
      <SayBar
        toName={shell.sayTo.name}
        address={sayAddress ? { label: sayAddress.label, title: sayAddress.title } : null}
        state={shell.say}
        offline={shell.offline}
        paused={snap.outsideSave?.outcome === 'declined'}
        onDraft={(text) => shell.dispatchSay({ type: 'draft', text })}
        onSend={() => shell.send(false)}
        onSendAgain={() => shell.send(true)}
      />
    ) : null

  const ringSlot = shell.followingId ? snap.slots.get(shell.followingId) : undefined
  return {
    extension,
    css: caretSlotCss(snap.slots),
    syncHolds,
    top: band('top'),
    bottom: band('bottom'),
    say,
    ring: ringSlot ? slotColour(ringSlot) : null,
    right,
    sentence,
    sentenceEnd,
    fitKey: `${shell.follow.control}|${fin ? 'fin' : ''}|${withOthers}`,
    follow,
  }
}

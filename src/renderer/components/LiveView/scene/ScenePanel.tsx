// The scene tab's side panel (the shell's `Panel` slot for scene
// snapshots): Activity, People and holds, Outliner, Properties, in the
// PANEL order. Keeps the view's own state for the step on screen (your
// pick, the row the keys are on, open collections, the search, which
// sections are open); a new step starts from what the step says, as the
// mock does. The outliner takes whole rows of the room that's left, and
// Properties sits right under it: the spare room goes to the panel's foot,
// so the gap above Properties never changes.

import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import { LOCAL0, localStore, type SceneLocal } from '@/lib/live-view/scene/local'
import { viewOnScreen } from '@/lib/live-view/scene/picture-model'
import { goView, objectView } from '@/lib/live-view/scene/views'
import { EMPTY_ACTIVITY, PANEL, activityItems, isLive, myPick, openByDefault, outlinerModel, peopleRows, propsModel, recentMap, target, type SectionId } from '@/lib/live-view/scene/panel'
import { isSceneSnapshot } from '@/lib/live-view/scene/sample-source'
import { ancestorsOf, sceneTree } from '@/lib/live-view/scene/tree'
import type { LiveShellSnapshot } from '@/lib/live-view/shared/source'
import type { ShellPanelProps } from '../shell/shell-config'
import { ActivityLines, PeopleLines } from './SceneActivity'
import { ROW_H, SceneOutliner } from './SceneOutliner'
import { PropertiesBody } from './SceneProperties'
import { SceneSection } from './SceneSection'
import { SCENE_PANEL_CSS } from './sceneCss'

/** Rows the outliner keeps at least (more when narrow: the sheet scrolls). */
const MIN_ROWS = { wide: 5, narrow: 9 }
/** Without a measured panel (tests, first paint), this many rows. */
const UNMEASURED_ROWS = 40

const NO_SUB = (): (() => void) => () => undefined
const NO_LOCAL = (): SceneLocal => LOCAL0

export const scenePanelFor =(snap: LiveShellSnapshot): boolean => isSceneSnapshot(snap)

/** Whole outliner rows in the panel's room, and the panel's height for its foot. */
function useRoom(root: React.RefObject<HTMLDivElement | null>, narrow: boolean, key: string): { rows: number; minHeight: number | undefined } {
  const [room, setRoom] = useState<{ rows: number; minHeight: number | undefined }>({ rows: UNMEASURED_ROWS, minHeight: undefined })
  // A few measures per layout (the panel's content, the window's height): a
  // row count that changes the panel's width (a scrollbar coming and going,
  // text re-wrapping) must not measure forever.
  const tries = useRef({ key: '', n: 0 })
  const measure = (force = false): void => {
    const el = root.current
    const scroller = el?.closest<HTMLElement>('[data-scroll-edges-list]')
    if (!el || !scroller || !scroller.clientHeight) return
    const k = `${key}|${scroller.clientHeight}`
    if (tries.current.key !== k || force) tries.current = { key: k, n: 0 }
    if (tries.current.n >= 3) return
    tries.current.n++
    const list = el.querySelector<HTMLElement>('.lv-olist-box')
    const foot = el.querySelector<HTMLElement>('.lv-foot')
    const used = el.scrollHeight - (foot?.offsetHeight ?? 0) - (list?.offsetHeight ?? 0)
    const min = narrow ? MIN_ROWS.narrow : MIN_ROWS.wide
    const rows = Math.max(min, Math.floor((scroller.clientHeight - used) / ROW_H))
    setRoom((r) => (r.rows === rows && r.minHeight === scroller.clientHeight ? r : { rows, minHeight: scroller.clientHeight }))
  }
  const latest = useRef(measure)
  latest.current = measure
  useLayoutEffect(() => measure())
  useEffect(() => {
    const scroller = root.current?.closest<HTMLElement>('[data-scroll-edges-list]')
    if (!scroller || typeof ResizeObserver === 'undefined') return
    let h = scroller.clientHeight
    const ro = new ResizeObserver(() => {
      if (scroller.clientHeight === h) return
      h = scroller.clientHeight
      latest.current(true)
    })
    ro.observe(scroller)
    return () => ro.disconnect()
  }, [root])
  return room
}

export function ScenePanel({ snap, followingId, onFollow, offline, narrow }: ShellPanelProps): React.JSX.Element | null {
  const scene = isSceneSnapshot(snap) ? snap.scene : null
  const S = scene?.S
  // A live host that takes no claims: no holds anywhere, and the section doesn't promise them.
  const snapHolds = scene?.holds
  const [open, setOpen] = useState<Record<string, boolean>>({})
  useEffect(() => setOpen({}), [narrow])
  const isOpen = (id: SectionId): boolean => open[id] ?? openByDefault(id, narrow)
  const toggle = (id: SectionId): void => setOpen((o) => ({ ...o, [id]: !isOpen(id) }))

  // The step's own view state; yours until the step changes. Your pick is
  // shared with the picture (it frames what you pick, and "." frames it).
  const store = S ? localStore(S) : null
  const local = useSyncExternalStore(store?.subscribe ?? NO_SUB, store?.get ?? NO_LOCAL, store?.get ?? NO_LOCAL)
  const pick = local.pick
  const setPick = (a: string): void => {
    if (!S || !store) return
    store.set((s) => ({ ...s, pick: a, ...goView(s, viewOnScreen(S, s), objectView(S.D, a)) }))
  }
  const [cur, setCur] = useState<string | null>(null)
  const [q, setQ] = useState(S?.ui.q ?? '')
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set(S?.ui.collapsed ?? []))
  // The state above belongs to the step it was set on; a new step starts fresh in the same render.
  const [owner, setOwner] = useState(S)
  if (owner !== S) {
    setOwner(S)
    setCur(null)
    setQ(S?.ui.q ?? '')
    setCollapsed(new Set(S?.ui.collapsed ?? []))
  }

  // Reveal your pick when it changes: open its collections, and the outliner scrolls to it.
  const mine = S ? myPick(S, pick) : null
  useEffect(() => {
    if (!S || !mine) return
    const anc = ancestorsOf(sceneTree(S.D), mine)
    setCollapsed((c) => (anc.some((a) => c.has(a)) ? new Set([...c].filter((a) => !anc.includes(a))) : c))
  }, [S, mine])

  const root = useRef<HTMLDivElement>(null)
  const room = useRoom(root, narrow, `${scene?.scenario}:${scene?.step}:${narrow}:${JSON.stringify(open)}:${q}:${collapsed.size}:${pick ?? ''}`)
  const recent = useMemo(() => (S ? recentMap(S) : {}), [S])
  const model = useMemo(() => (S ? outlinerModel(S, { collapsed, q }) : null), [S, collapsed, q])
  if (!S || !scene || !model) return null
  const live = isLive(S)
  const people = peopleRows(S, { followingId, pick })
  const activity = S.loading ? null : activityItems(S, scene.fileName)
  const props = propsModel(S, pick, recent)
  const peopleBody = S.loading ? (
    <div className="lv-skel">
      <i />
      <i />
    </div>
  ) : !live ? (
    <div className="lv-nopick">Others show here when the file is live.</div>
  ) : (
    <PeopleLines rows={people} stale={offline} onFollow={onFollow} />
  )
  const bodies: Record<SectionId, { count?: string | null; flush?: boolean; node: React.ReactNode }> = {
    activity: { node: activity ? <ActivityLines items={activity} empty={EMPTY_ACTIVITY} /> : <div className="lv-skel"><i /><i /></div> },
    people: { count: live && !S.loading ? String(people.length) : null, node: peopleBody },
    outliner: {
      count: model.count,
      flush: true,
      node: (
        <SceneOutliner
          S={S}
          model={model}
          recent={recent}
          pick={mine}
          cur={cur}
          rows={room.rows}
          reveal={mine}
          live={live}
          q={q}
          onQuery={setQ}
          onToggle={(c, o) => setCollapsed((s) => { const n = new Set(s); if (o) n.delete(c); else n.add(c); return n })}
          onPick={(a) => setPick(a)}
          onCur={setCur}
        />
      ),
    },
    props: { node: <PropertiesBody m={props} /> },
  }
  return (
    <div ref={root} className="lv-scene-panel" data-testid="lv-scene-panel" data-target={target(S, pick) ?? undefined} style={{ minHeight: room.minHeight }}>
      <style>{SCENE_PANEL_CSS}</style>
      {PANEL.map((s) => (
        <SceneSection key={s.id} id={s.id} title={s.id === 'people' && snapHolds === false ? 'People' : s.title} count={bodies[s.id].count} flush={bodies[s.id].flush} grow={s.grow} open={isOpen(s.id)} onToggle={() => toggle(s.id)}>
          {bodies[s.id].node}
        </SceneSection>
      ))}
      <div className="lv-foot" aria-hidden="true" />
    </div>
  )
}

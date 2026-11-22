// The live workbook view: a workbook as a live doc sends it, drawn as a grid
// with who's here, what they hold, and what changed. It follows the agent by
// default; scrolling, a click, a tab or the minimap pauses that, and a pill
// offers to follow again. Reads any LiveWorkbookSource; with the sample one,
// a banner says it is sample data.

import { useCallback, useMemo, useState } from 'react'
import { Button, Spinner, Surface } from '@/components/ui'
import { useReducedMotion } from '@/hooks/useReducedMotion'
import { agentFocus, agentId, buildingBook, caretOn, loadingOf } from '@/lib/live-view/reducer'
import type { SampleScenario, SampleWorkbookSource } from '@/lib/live-view/sample-source'
import type { LiveWorkbookSource } from '@/lib/live-view/source'
import { EMPTY_LOCAL, formulaTarget, isFollowing, stageSheet, type LocalView } from '@/lib/live-view/view-model'
import { gridOf, sheetsOf, titleOf } from '@/lib/live-view/workbook'
import { actorFrom, type Actor, type PhraseTarget } from '@/lib/live-view/wording'
import { ActivityBar } from './ActivityBar'
import { LIVE_VIEW_CSS } from './liveViewCss'
import { Minimap } from './Minimap'
import { Avatar, DocGlyph, InfoGlyph } from './parts'
import { SampleBanner } from './SampleBanner'
import { useLiveWorkbook } from './useLiveWorkbook'
import { FormulaBar, SheetTabs, WorkbookHeader } from './WorkbookChrome'
import { WorkbookGrid } from './WorkbookGrid'
import { PRODUCT_NAME } from '@/lib/brand'

export interface LiveWorkbookViewProps {
  source: LiveWorkbookSource
  fileName: string
  /** The sample's player, when the sample drives the view. */
  sample?: { source: SampleWorkbookSource; scenario: SampleScenario; onScenario: (s: SampleScenario) => void }
}

export function LiveWorkbookView({ source, fileName, sample }: LiveWorkbookViewProps): React.JSX.Element {
  const [state, dispatch] = useLiveWorkbook(source)
  const reduced = useReducedMotion()
  const [local, setLocal] = useState<LocalView>(EMPTY_LOCAL)
  const [formulas, setFormulas] = useState(false)
  const [seen, setSeen] = useState<{ r0: number; r1: number; c0: number; c1: number } | null>(null)

  const book = state.book ?? buildingBook(state)
  const loaded = !!state.book && !state.building
  const live = state.tier === 'live' && loaded
  const following = isFollowing(state)
  const stage = state.book ? stageSheet(state, local) : book ? (sheetsOf(book).find((s) => !s.hidden)?.key ?? null) : null
  const aId = agentId(state)
  const agentPeer = aId ? state.peers[aId] : undefined
  const agent: Actor | null = agentPeer ? actorFrom({ id: agentPeer.handle, name: agentPeer.name, kind: agentPeer.kind, colour: agentPeer.colour }) : null
  const target = state.book && stage ? formulaTarget(state, local, stage) : null
  const self = state.welcome?.self ? state.peers[state.welcome.self] : undefined
  const selfCaret = stage && local.caret?.sheet === stage ? local.caret : state.book && stage && self?.focus?.key === stage ? caretOn(state.book, self, stage) : null
  const t0 = useMemo(() => state.log.find((e) => e.at != null)?.at ?? null, [state.log])

  // While following: bring the agent's caret into view, else the latest change's place.
  const scrollTarget = useMemo(() => {
    if (!following || !state.book || !stage) return null
    const rc = caretOn(state.book, agentPeer, stage)
    if (rc) return { ...rc, key: `${stage}:${rc.r}:${rc.c}:${state.logSeq}` }
    const last = [...state.log].reverse().find((e) => e.kind === 'frame')
    const t = last?.kind === 'frame' ? last.phrase.target : null
    return t && t.sheet === stage ? { r: t.r, c: t.c, key: `${stage}:${t.r}:${t.c}:${state.logSeq}` } : null
  }, [following, state.book, state.logSeq, state.log, stage, agentPeer])

  const pause = useCallback(() => {
    if (!following) return
    dispatch({ type: 'local_follow', on: false })
    setLocal((l) => ({ ...l, pinned: stage }))
  }, [following, dispatch, stage])

  const follow = (on: boolean): void => {
    dispatch({ type: 'local_follow', on })
    setLocal(on ? EMPTY_LOCAL : { pinned: stage, caret: null })
  }

  const pickSheet = (key: string): void => {
    if (following && key === agentFocus(state)) return
    dispatch({ type: 'local_follow', on: false })
    setLocal({ pinned: key, caret: null })
  }

  const goTo = (sheet: string, t: PhraseTarget | null): void => {
    if (!state.book || !sheetsOf(state.book).some((s) => s.key === sheet)) return
    dispatch({ type: 'local_follow', on: false })
    setLocal({ pinned: sheet, caret: t ? { sheet, r: t.r, c: t.c } : null })
  }

  const g = book && stage ? gridOf(book, stage) : null
  const progress = loadingOf(state)
  const af = agentFocus(state)

  return (
    <div className="lv-root flex h-full min-h-0 flex-col bg-[var(--color-bg)] text-[var(--color-text-primary)]" data-reduced={reduced ? '' : undefined} data-testid="live-workbook-view">
      <style>{LIVE_VIEW_CSS}</style>
      {source.kind === 'sample' && sample ? <SampleBanner sample={sample.source} scenario={sample.scenario} onScenario={sample.onScenario} /> : null}
      {source.kind === 'sample' && !sample ? (
        <div className="border-b border-[var(--color-border)] bg-[var(--color-status-warn-soft)] px-3 py-1.5 type-hint" role="note" data-testid="lv-sample-banner">
          Sample data, not this file.
        </div>
      ) : null}
      <WorkbookHeader state={state} book={state.book} fileName={fileName} following={following} agent={live ? agent : null} canFollow={live && !!agent} onFollow={follow} />
      <FormulaBar state={state} book={state.book} sheet={stage} target={target} agent={agent} formulas={formulas} onFormulas={setFormulas} />
      {state.tier === 'blob' && book ? (
        <div className="flex items-start gap-2 border-b border-[var(--color-border)] bg-[var(--color-bg-inset)] px-3 py-2" role="note" data-testid="lv-blob-banner">
          <span className="mt-0.5 text-[var(--color-status-warn-text)]">
            <InfoGlyph size={15} />
          </span>
          <div className="type-hint text-[var(--color-text-secondary)]">
            <b className="text-[var(--color-text-primary)]">Too large to watch live.</b> This workbook is past the live limit (25,000 cells and fields), so you’re seeing its last saved version, read-only. Changes show when the file is saved again; who’s here and live edits don’t show.
          </div>
        </div>
      ) : null}
      <div className="relative flex min-h-0 flex-1 flex-col">
        {state.tier === 'none' ? (
          <div className="flex flex-1 items-center justify-center p-6" data-testid="lv-none">
            <Surface role2="elevated" chamfer material="tile" elevation={2} className="max-w-[420px] p-5 text-center">
              <div className="mx-auto mb-2 w-fit text-[var(--color-text-muted)]">
                <DocGlyph size={28} />
              </div>
              <h3 className="type-row font-semibold">This workbook is too large for {PRODUCT_NAME}</h3>
              <p className="mt-1 type-hint text-[var(--color-text-secondary)]">{PRODUCT_NAME} opens workbooks up to 32 MiB. This one is bigger, so there’s no live view, no saved versions and no presence. Open it in a spreadsheet app instead, or ask the agent for a smaller extract.</p>
            </Surface>
          </div>
        ) : book && stage ? (
          <WorkbookGrid
            state={state}
            book={book}
            sheet={stage}
            live={live}
            formulas={formulas}
            reduced={reduced}
            current={target}
            selfCaret={live ? selfCaret : null}
            scrollTarget={scrollTarget}
            onPickCell={(r, c) => {
              if (!live) return
              pause()
              dispatch({ type: 'local_follow', on: false })
              setLocal({ pinned: stage, caret: { sheet: stage, r, c } })
            }}
            onUserMove={pause}
            onSeen={setSeen}
          />
        ) : (
          <div className="flex-1" />
        )}
        {!loaded && state.tier !== 'none' ? (
          <div className="absolute inset-0 z-[9] flex items-center justify-center p-6" data-testid="lv-loading">
            <Surface role2="elevated" chamfer material="raised" elevation={3} className="w-[340px] p-4">
              <h3 className="flex items-center gap-2 type-row font-semibold">
                <Spinner />
                Opening the live workbook
              </h3>
              <p className="mt-1 type-hint text-[var(--color-text-secondary)]">The sheets come first, then the workbook one part at a time. The grid draws as soon as its cells are in.</p>
              <div className="mt-3 h-1.5 overflow-hidden bg-[var(--color-bg-inset)]" role="progressbar" aria-valuemin={0} aria-valuemax={progress.total} aria-valuenow={progress.done} aria-label="Loading">
                <div className="h-full bg-[var(--color-accent)]" style={{ width: `${Math.round((progress.done / Math.max(1, progress.total)) * 100)}%` }} />
              </div>
              <ul className="mt-2 flex flex-col gap-0.5 type-hint text-[var(--color-text-muted)]">
                {progress.roots.map((r) => (
                  <li key={r} className={progress.rootsIn.includes(r) ? 'text-[var(--color-text-secondary)]' : undefined}>
                    {progress.rootsIn.includes(r) ? '✓ ' : '· '}
                    {r}
                  </li>
                ))}
              </ul>
            </Surface>
          </div>
        ) : null}
        {live && g ? <Minimap state={state} book={book!} g={g} seen={seen} onGo={(r, c) => { pause(); setLocal({ pinned: stage, caret: { sheet: g.key, r, c } }) }} /> : null}
        {live && agent && !following ? (
          <div className="absolute bottom-3 left-1/2 z-[8] -translate-x-1/2" data-testid="lv-resume">
            <Surface role2="elevated" chamfer material="raised" elevation={3} className="flex items-center gap-2 px-3 py-1.5">
              <Avatar actor={agent} size={18} />
              <span className="type-hint text-[var(--color-text-secondary)]">{af && af !== stage && state.book ? `${agent.name} is on ${titleOf(state.book, af)}` : 'Paused'}</span>
              <Button size="sm" variant="accent" onClick={() => follow(true)}>
                Follow {agent.name} again
              </Button>
            </Surface>
          </div>
        ) : null}
      </div>
      <SheetTabs state={state} book={book} stage={stage} reduced={reduced} onPick={pickSheet} />
      <ActivityBar state={state} fileName={fileName} t0={t0} onGo={goTo} />
    </div>
  )
}

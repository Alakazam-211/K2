// The activity line (the latest change plus a count; the claim's label while
// one is held; "done" when the turn ends) and the history, grouped by turn.

import { useState } from 'react'
import { Button } from '@/components/ui'
import { IconLock } from '@/components/icons/IconLock'
import { ChevronDownIcon, RecentsIcon } from '@/components/icons/K2Icons'
import { idleOf, type LiveWorkbookState } from '@/lib/live-view/reducer'
import { activityLine, phraseLine, turnGroups, turnShort, type LogEntry, type PhraseTarget } from '@/lib/live-view/wording'
import { Avatar, InfoGlyph, secondsFrom } from './parts'
import { PRODUCT_NAME } from '@/lib/brand'

export function ActivityBar({
  state,
  fileName,
  t0,
  onGo,
}: {
  state: LiveWorkbookState
  fileName: string
  /** The clock's start, for "s into the turn". */
  t0: number | null
  /** A history line was picked: go to its sheet and place. */
  onGo: (sheet: string, target: PhraseTarget | null) => void
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  if (state.building || (!state.book && state.tier === 'live')) {
    return (
      <div className="flex min-h-[44px] items-center gap-2 border-t border-[var(--color-border)] px-3 py-1.5 type-hint text-[var(--color-text-secondary)]" aria-live="polite" data-testid="lv-activity">
        Loading the live workbook… Changes and who’s here show once the snapshot is in.
      </div>
    )
  }
  if (state.tier !== 'live') {
    return (
      <div className="flex min-h-[44px] items-center gap-2 border-t border-[var(--color-border)] px-3 py-1.5" aria-live="polite" data-testid="lv-activity">
        <span className="text-[var(--color-status-warn-text)]">
          <InfoGlyph size={16} />
        </span>
        <div className="min-w-0">
          <div className="type-row text-[var(--color-text-primary)]">{state.tier === 'blob' ? 'Not live: you see changes when the file is saved' : 'Not opened: the file is over 32 MiB'}</div>
          <div className="type-hint text-[var(--color-text-muted)]">{state.tier === 'blob' ? 'Saved versions only. No one else’s cursor, no change outlines, no follow.' : `${PRODUCT_NAME} keeps no live copy of it.`}</div>
        </div>
      </div>
    )
  }
  const idle = idleOf(state)
  const line = activityLine(state.log, state.claims.map((c) => ({ actor: { id: c.actor.id, name: c.actor.name, kind: c.actor.kind, colour: c.actor.colour }, label: c.label })), idle)
  const groups = turnGroups(state.log, idle)
  const latest = line.latest ? phraseLine(line.latest, fileName) : null
  const more = line.more ? `, +${line.more} more` : ''
  let l1: React.ReactNode
  let l2: string
  if (!line.actor) {
    l1 = 'No changes yet'
    l2 = `Changes show here as they land, worded by ${PRODUCT_NAME}.`
  } else if (line.claimLabel) {
    l1 = (
      <>
        <span style={{ color: line.actor.colour }}>
          <IconLock className="inline h-3.5 w-3.5 align-[-2px]" />
        </span>{' '}
        <b>{line.actor.name}:</b> {line.claimLabel}
      </>
    )
    l2 = latest ? `Latest: ${latest}${more}` : 'No changes yet in this turn'
  } else if (line.done && line.latest) {
    l1 = (
      <>
        <b>{line.actor.name} is done</b> · {line.latest.lead}
        {line.latest.text}
        {more}
      </>
    )
    l2 = line.endedBy === 'idle' ? `No word from ${line.actor.name} for ${Math.round((idle.idleMs ?? 0) / 1000)} s: the turn has ended` : `Turn ended at ${secondsFrom(t0, line.endedAt)}`
  } else {
    l1 = (
      <>
        {latest}
        {more}
      </>
    )
    l2 = line.actor.kind === 'file' ? `Saved outside ${PRODUCT_NAME}: no turn, no agent` : line.latestAt != null ? `${secondsFrom(t0, line.latestAt)} into the turn` : ''
  }
  return (
    <div className="border-t border-[var(--color-border)]">
      <div className="flex min-h-[44px] items-center gap-2.5 px-3 py-1.5" data-testid="lv-activity">
        {line.actor ? <Avatar actor={line.actor} size={26} ring={line.working} /> : null}
        <div className="min-w-0 flex-1" aria-live="polite">
          <div key={state.logSeq} className="lv-swap truncate type-row text-[var(--color-text-primary)]" data-testid="lv-activity-line">
            {l1}
          </div>
          <div className="truncate type-hint text-[var(--color-text-muted)]">{l2}</div>
        </div>
        <span className="lv-num type-hint text-[var(--color-text-muted)]">
          {line.frames} change{line.frames === 1 ? '' : 's'}
          {line.turn ? ` · turn ${turnShort(line.turn)}` : ''}
        </span>
        <Button size="sm" variant="ghost" aria-expanded={open} onClick={() => setOpen(!open)} className="inline-flex items-center gap-1" data-testid="lv-history-toggle">
          <RecentsIcon size={14} />
          History
          <span style={{ transform: open ? 'none' : 'rotate(180deg)', display: 'inline-flex' }}>
            <ChevronDownIcon size={12} />
          </span>
        </Button>
      </div>
      {open ? (
        <div className="max-h-[220px] overflow-y-auto border-t border-[var(--color-border)] bg-[var(--color-bg-inset)] px-2 py-1" role="region" aria-label="Recent changes" data-testid="lv-history">
          {groups.length === 0 ? <div className="px-2 py-1 type-hint text-[var(--color-text-muted)]">No changes yet</div> : null}
          {[...groups].reverse().map((g, gi) => {
            const n = g.items.filter((e) => e.kind === 'frame').length
            return (
              <div key={`${g.turn ?? 'solo'}${gi}`} className="py-1">
                <div className="flex items-center gap-2 px-2 py-1 type-hint text-[var(--color-text-secondary)]">
                  <Avatar actor={g.actor} size={16} />
                  <b className="text-[var(--color-text-primary)]">{g.actor.name}</b>
                  <span className="lv-num">
                    {n} change{n === 1 ? '' : 's'}
                  </span>
                  {g.ended && g.turn ? <span>· done{g.endedBy === 'idle' ? ' (went quiet)' : ''}</span> : null}
                  <span className="flex-1" />
                  <span className="font-[family-name:var(--font-code)] text-[var(--color-text-muted)]">{g.turn ? `turn ${turnShort(g.turn)}` : 'no turn'}</span>
                </div>
                {[...g.items].reverse().map((e) => (
                  <HistoryItem key={e.seq} e={e} fileName={fileName} t0={t0} onGo={onGo} />
                ))}
              </div>
            )
          })}
        </div>
      ) : null}
    </div>
  )
}

function HistoryItem({ e, fileName, t0, onGo }: { e: LogEntry; fileName: string; t0: number | null; onGo: (sheet: string, target: PhraseTarget | null) => void }): React.JSX.Element {
  const row = (lead: string, body: React.ReactNode, sheet: string | null, target: PhraseTarget | null = null): React.JSX.Element => (
    <button
      type="button"
      className="lv-hist-item flex w-full items-center gap-2 px-2 py-0.5 text-left type-hint hover:bg-[var(--color-bg-hover)] disabled:cursor-default"
      disabled={!sheet}
      onClick={() => sheet && onGo(sheet, target)}
    >
      <span className="lv-num w-[52px] flex-none text-[var(--color-text-muted)]">{lead}</span>
      <span className="min-w-0 flex-1 truncate text-[var(--color-text-primary)]">{body}</span>
      <span className="lv-num flex-none text-[var(--color-text-muted)]">{secondsFrom(t0, e.at)}</span>
    </button>
  )
  if (e.kind === 'frame') return row(`rev ${e.rev}`, `${e.phrase.who ? '' : `${fileName} `}${e.phrase.lead}${e.phrase.text}`, e.phrase.sheet, e.phrase.target)
  if (e.kind === 'claim') return row('claim', `“${e.label}”${e.place ? ` · ${e.place}` : ''}${e.hold ? ' · held' : ''}`, e.sheet)
  if (e.kind === 'claim_end') return row(e.how, `claim ended${e.label ? ` “${e.label}”` : ''}${e.lines ? ` · rows ${e.lines.first}–${e.lines.last}` : ''}`, null)
  return row('end', 'turn ended', null)
}

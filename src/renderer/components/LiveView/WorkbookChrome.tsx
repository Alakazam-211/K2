// The workbook's chrome: the header (file, live state, who's here, follow,
// and the file tab's "File | Watch live" switch while watching),
// the formula bar, and the sheet tabs at the bottom (with the hidden-sheets
// menu).

import { Button, OverflowMenu, Segmented, Tag } from '@/components/ui'
import { IconLock } from '@/components/icons/IconLock'
import { modeOf, others, type LiveWorkbookState } from '@/lib/live-view/reducer'
import { colLetters } from '@k2/mpc-client'
import { cellShown, formatNumber, gridOf, sheetsOf, styleOf, titleOf, type Book } from '@/lib/live-view/workbook'
import { actorFrom, type Actor } from '@/lib/live-view/wording'
import { LiveEntrySwitch } from './LiveEntrySwitch'
import { Avatar, EyeOffGlyph, FollowGlyph, InfoGlyph, SheetGlyph } from './parts'

export function WorkbookHeader({
  state,
  book,
  fileName,
  following,
  agent,
  canFollow,
  onFollow,
}: {
  state: LiveWorkbookState
  book: Book | null
  fileName: string
  following: boolean
  agent: Actor | null
  canFollow: boolean
  onFollow: (on: boolean) => void
}): React.JSX.Element {
  const loading = !!state.building || (!book && state.tier === 'live')
  const people = state.tier === 'live' && book ? others(state) : []
  const self = state.welcome?.self ? state.peers[state.welcome.self] : undefined
  return (
    <div data-tab-header="" className="flex min-h-[40px] flex-wrap items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-tab-active)] px-3 py-1.5" data-testid="lv-header">
      <span className="inline-flex items-center gap-1.5 text-[var(--color-text-secondary)]">
        <SheetGlyph size={14} />
        <span className="type-row font-medium text-[var(--color-text-primary)]">{fileName}</span>
      </span>
      {loading ? (
        <Tag tone="muted">Loading</Tag>
      ) : state.tier === 'live' ? (
        <>
          <Tag tone="ok" title="Watching the live doc">
            Live
          </Tag>
          <Tag tone="neutral" title="The live doc's change number" className="lv-num">
            rev {book?.rev}
          </Tag>
        </>
      ) : state.tier === 'blob' ? (
        <Tag tone="warn">Saved version · read-only</Tag>
      ) : (
        <Tag tone="warn">Not opened</Tag>
      )}
      <span className="flex-1" />
      <div className="flex flex-wrap items-center gap-3" aria-label="Who's here">
        {people.map((p) => {
          const mode = book ? modeOf(book, p, state.claims) : 'viewing'
          const sheet = p.focus && book ? titleOf(book, p.focus.key) : ''
          return (
            <span key={p.handle} className="inline-flex items-center gap-1.5 type-hint" data-testid={`lv-person-${p.handle}`}>
              <Avatar actor={{ name: p.name, kind: p.kind, colour: p.colour }} size={20} ring={p.kind === 'agent' && mode === 'editing'} away={p.away} />
              <span className="text-[var(--color-text-primary)]">{p.name}</span>
              <span className="text-[var(--color-text-muted)]">
                {mode}
                {sheet ? ` · ${sheet}` : ''}
              </span>
            </span>
          )
        })}
        {self ? (
          <span className="inline-flex items-center gap-1.5 type-hint text-[var(--color-text-secondary)]" title="You">
            <Avatar actor={{ name: self.name, kind: self.kind, colour: 'var(--color-accent)' }} size={20} />
            You ({self.name})
          </span>
        ) : null}
      </div>
      {agent ? (
        <Button
          size="sm"
          variant={following ? 'accent' : 'default'}
          aria-pressed={following}
          disabled={!canFollow}
          onClick={() => onFollow(!following)}
          data-testid="lv-follow"
          className="inline-flex items-center gap-1.5"
        >
          {following ? <Avatar actor={agent} size={14} /> : <FollowGlyph size={14} />}
          {following ? `Following ${agent.name}` : `Follow ${agent.name}`}
        </Button>
      ) : null}
      <LiveEntrySwitch />
    </div>
  )
}

export function FormulaBar({
  state,
  book,
  sheet,
  target,
  agent,
  formulas,
  onFormulas,
}: {
  state: LiveWorkbookState
  book: Book | null
  sheet: string | null
  target: { r: number; c: number; who: 'you' | 'agent' } | null
  agent: Actor | null
  formulas: boolean
  onFormulas: (on: boolean) => void
}): React.JSX.Element {
  const g = book && sheet ? gridOf(book, sheet) : null
  const cell = g && target ? g.cellAt(target.r, target.c) : undefined
  const shown = cell ? cellShown(cell) : null
  const name = target ? `${colLetters(target.c)}${target.r}` : ''
  let value: React.ReactNode
  let chip: React.ReactNode = null
  if (!book) value = <span className="text-[var(--color-text-muted)]">{state.tier === 'none' ? 'No workbook' : 'Loading…'}</span>
  else if (!target) value = <span className="text-[var(--color-text-muted)]">Select a cell</span>
  else if (shown?.kind === 'formula') {
    value = <span className="font-[family-name:var(--font-code)] text-[var(--color-accent-text)]">{shown.formula}</span>
    chip =
      shown.result != null ? (
        <Tag tone="neutral" title="The file's cached result: K2 has no calculator" className="lv-num">
          as last calculated {g && cell ? formatNumber(shown.result, styleOf(g, cell).numfmt).text : shown.result}
        </Tag>
      ) : (
        <Tag tone="warn" title="Written live: no result until the file is calculated">
          not calculated
        </Tag>
      )
  } else if (shown && shown.text !== '') value = <span className="text-[var(--color-text-primary)]">{shown.text}</span>
  else value = <span className="text-[var(--color-text-muted)]">Empty cell</span>
  return (
    <div className="flex flex-wrap items-center gap-2 border-b border-[var(--color-border)] px-3 py-1" data-testid="lv-formula-bar">
      <span className="lv-num inline-flex h-6 min-w-[56px] items-center justify-center border border-[var(--color-border)] bg-[var(--color-bg-inset)] px-2 type-hint" data-k2-chamfer="chip" title={target ? `Row ${target.r}, column ${colLetters(target.c)}` : undefined} data-testid="lv-namebox">
        {name || '—'}
      </span>
      <span className="font-[family-name:var(--font-code)] italic text-[var(--color-text-muted)]">fx</span>
      <div key={`${name}|${cell?.raw ?? ''}|${cell?.a1 ?? ''}`} className="lv-swap min-w-0 flex-1 truncate type-row" data-testid="lv-formula-text">
        {value}
      </div>
      {chip}
      {target && agent ? (
        <span className="inline-flex items-center gap-1 type-hint text-[var(--color-text-secondary)]">
          {target.who === 'agent' ? <Avatar actor={agent} size={14} /> : null}
          {target.who === 'agent' ? `${agent.name}’s cell` : 'Your cell'}
        </span>
      ) : null}
      {state.firstLiveEdit != null && state.tier === 'live' ? (
        <span className="inline-flex items-center gap-1 type-hint text-[var(--color-text-muted)]" title="K2 has no calculator: every cached result stays as the file last calculated it">
          <InfoGlyph size={12} />
          Results not recalculated since rev {state.firstLiveEdit}
        </span>
      ) : null}
      <Segmented<'values' | 'formulas'>
        aria-label="Show in cells"
        value={formulas ? 'formulas' : 'values'}
        onChange={(v) => onFormulas(v === 'formulas')}
        options={[
          { value: 'values', label: 'Values' },
          { value: 'formulas', label: 'Formulas' },
        ]}
      />
    </div>
  )
}

export function SheetTabs({
  state,
  book,
  stage,
  reduced,
  onPick,
}: {
  state: LiveWorkbookState
  book: Book | null
  stage: string | null
  reduced: boolean
  onPick: (key: string) => void
}): React.JSX.Element {
  if (!book) {
    return (
      <nav className="flex h-9 items-center gap-2 border-t border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 type-hint text-[var(--color-text-muted)]" aria-label="Sheets">
        <SheetGlyph size={14} />
        {state.tier === 'none' ? 'No sheets' : 'Sheets load first…'}
      </nav>
    )
  }
  const live = state.tier === 'live'
  const all = sheetsOf(book)
  const latest = state.recent.find((r) => r.age === 0)
  const flashKey = (() => {
    if (!latest) return null
    for (const o of latest.frame.ops) if (o.kind !== 'text' && (o.root === book.schema.containers || o.root === book.state.schema.composite?.find((c) => c.list === book.schema.containers)?.map)) return o.key
    return null
  })()
  const visible = all.filter((s) => !s.hidden || s.key === stage)
  const hidden = all.filter((s) => s.hidden)
  return (
    <nav className="flex min-h-9 items-stretch gap-0.5 overflow-x-auto border-t border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-2" aria-label="Sheets" data-testid="lv-tabs">
      <div role="tablist" aria-label="Sheets" className="flex items-stretch gap-0.5">
        {visible.map((s) => {
          const here = live ? others(state).filter((p) => p.focus?.key === s.key) : []
          const claims = live ? state.claims.filter((c) => c.container === s.key) : []
          const label = `${s.title}${here.length ? `, ${here.map((p) => p.name).join(' and ')} here` : ''}${claims.length ? `, ${claims.map((c) => `${c.actor.name}: ${c.label}`).join('; ')}` : ''}`
          const flash = flashKey === s.key && latest && !reduced
          return (
            <button
              key={`${s.key}${flash ? `:${latest.frame.rev}` : ''}`}
              type="button"
              role="tab"
              aria-selected={s.key === stage}
              aria-label={label}
              data-flash={flash ? '' : undefined}
              data-testid={`lv-tab-${s.key}`}
              className="lv-tab inline-flex items-center gap-1.5 px-3 py-1 type-hint text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]"
              style={flash ? { ['--tc' as string]: actorFrom(latest.frame.actor).colour } : undefined}
              onClick={() => onPick(s.key)}
            >
              {s.hidden ? <EyeOffGlyph size={12} /> : null}
              <span>{s.title}</span>
              {here.length ? (
                <span className="inline-flex -space-x-1">
                  {here.map((p) => (
                    <Avatar key={p.handle} actor={{ name: p.name, kind: p.kind, colour: p.colour }} size={14} away={p.away} />
                  ))}
                </span>
              ) : null}
              {claims.map((c) => (
                <span key={c.id} style={{ color: actorFrom(c.actor).colour }} title={`${c.actor.name}: ${c.label}`}>
                  <IconLock className="h-3 w-3" />
                </span>
              ))}
            </button>
          )
        })}
      </div>
      <span className="flex-1" />
      {hidden.length ? (
        <div className="flex items-center">
          <OverflowMenu
            label={`${hidden.length} hidden sheet${hidden.length === 1 ? '' : 's'}`}
            menuLabel="Hidden sheets"
            align="end"
            trigger={
              <span className="inline-flex items-center gap-1 type-hint text-[var(--color-text-secondary)]">
                <EyeOffGlyph size={12} />
                {hidden.length} hidden
              </span>
            }
            items={hidden.map((h) => ({
              id: h.key,
              label: h.title,
              hint: h.state === 'very_hidden' ? 'hidden by the file' : 'hidden',
              onSelect: () => onPick(h.key),
            }))}
          />
        </div>
      ) : null}
    </nav>
  )
}

// The live file's side panel (editors-notes.md §2 Live, v3, v3.2): a tab
// strip ("Live 3"; Outline joins it in E3), then
//   - Here N (you first) with "Live edits" at its end, and Away N under it;
//     while disconnected the head says "Last seen", the rows "last seen in…",
//     and Live edits and every Undo are off with "Back online to use this";
//   - Changes, newest first: Go there, Undo on your own newest edit (the
//     server's per-person undo; Undo on anyone's edit waits for the kit's
//     ChangeInfo), Redo after it, and "Ask Sam" on a refusal;
//   - Live edits (a list you open): one line per finished edit by someone
//     else, with the time, and "Read finished edits aloud" (off by default);
//   - Messages: what you said to someone from the say row, and their replies;
//   - one hint at the panel's foot, dismissed once on this computer.
// A Here row follows its person (E4b), marked "Following" while you do.

import { useState } from 'react'
import { cx } from '@/components/ui'
import { ChangesList } from '@/components/LiveView/shell/ChangesList'
import { ChevronGlyph, CloseGlyph } from '@/components/LiveView/shell/glyphs'
import { LiveAvatar } from '@/components/LiveView/shell/LiveAvatar'
import { MessagesList } from '@/components/LiveView/shell/MessagesList'
import { PanelSection } from '@/components/LiveView/shell/PanelSection'
import { WatchingList } from '@/components/LiveView/shell/WatchingList'
import { useLocalFlag } from '@/lib/editor/local-flag'
import type { LiveDocSnapshot } from '@/lib/editor/live/live-doc-source'
import type { LiveChange } from '@/lib/live-view/shared/source'
import { personById, watching, type LivePerson } from '@/lib/live-view/shared/people'

const UNDO_TIP_KEY = 'k2.editor.liveUndoTip.v1'
export const READ_ALOUD_KEY = 'k2.editor.readEditsAloud.v1'
export const UNDO_TIP = '⌘Z undoes only your own edits.'
const UNDO_ROW_TIP = '⌘Z undoes only your own edits; this undoes this change'
const OFF = 'Back online to use this'

const clock = (at: number): string => new Intl.DateTimeFormat('en-GB', { hour: '2-digit', minute: '2-digit', hour12: false }).format(new Date(at))

function LiveEdits({ snap, people, readAloud, onReadAloud, onBack }: { snap: LiveDocSnapshot; people: readonly LivePerson[]; readAloud: boolean; onReadAloud: (on: boolean) => void; onBack: () => void }): React.JSX.Element {
  const rows = [...snap.liveEdits].reverse()
  return (
    <div data-testid="ed-live-edits">
      <button type="button" data-k2-focus="" onClick={onBack} className="lv-row-btn no-drag type-row flex cursor-pointer items-center gap-1.5 rounded-[var(--radius-selector)] px-3.5 pb-1 pt-2.5 text-[var(--color-text-secondary)]">
        <span aria-hidden="true" className="inline-flex rotate-90">
          <ChevronGlyph size={13} />
        </span>
        Here
      </button>
      <PanelSection id="live-edits" title="Live edits" count={rows.length} countTestId="ed-live-edits-count">
        <label className="type-row flex cursor-pointer items-center gap-2 px-3.5 py-1 text-[var(--color-text-primary)]">
          <input type="checkbox" className="k2-checkbox" data-k2-focus="" checked={readAloud} onChange={(e) => onReadAloud(e.target.checked)} />
          Read finished edits aloud
        </label>
        {rows.length === 0 ? <p className="type-hint m-0 px-3.5 py-1 text-[var(--color-text-secondary)]">Edits by others show here as they finish.</p> : null}
        <ul className="m-0 list-none p-0">
          {rows.map((e) => {
            const p = personById(people, e.who)
            return (
              <li key={e.id} data-testid="ed-live-edit" className="flex items-start gap-2 px-3.5 py-1">
                {p ? <LiveAvatar person={p} size={18} /> : null}
                <span className="type-row min-w-0 flex-1 text-[var(--color-text-primary)]">
                  {p?.name ?? e.who} {e.text}
                </span>
                <span className="type-hint flex-none text-[var(--color-text-secondary)]">{clock(e.at_ms)}</span>
              </li>
            )
          })}
        </ul>
      </PanelSection>
    </div>
  )
}

export function LivePanel({
  snap,
  people,
  onGoThere,
  onUndo,
  onRedo,
  onAsk,
  followingId = null,
  onFollow,
}: {
  snap: LiveDocSnapshot
  /** The panel's people (you say what you're doing). */
  people: readonly LivePerson[]
  onGoThere: (c: LiveChange) => void
  onUndo: (c: LiveChange) => void
  onRedo: (c: LiveChange) => void
  onAsk: (c: LiveChange) => void
  followingId?: string | null
  /** A Here row follows its person (none while disconnected). */
  onFollow?: (id: string) => void
}): React.JSX.Element {
  const [view, setView] = useState<'here' | 'edits'>('here')
  const [tipGone, setTipGone] = useLocalFlag(UNDO_TIP_KEY)
  const [readAloud, setReadAloud] = useLocalFlag(READ_ALOUD_KEY)
  const down = snap.connection !== null && snap.connection !== 'back'
  const here = watching(people).count
  const liveEditsBtn = (
    <button
      type="button"
      data-k2-focus=""
      data-k2-chamfer="control"
      data-testid="ed-live-edits-open"
      aria-disabled={down || undefined}
      title={down ? OFF : 'One line per finished edit'}
      onClick={() => (down ? undefined : setView('edits'))}
      className="ed-chip-btn no-drag type-hint flex-none cursor-pointer rounded-[var(--radius-pill)] border border-[var(--color-control-edge)] px-2.5 py-0.5 text-[var(--color-text-primary)]"
    >
      Live edits
    </button>
  )
  return (
    <div data-testid="ed-live-panel" className="flex min-h-full flex-col">
      <div role="tablist" aria-label="Side panel" className="flex flex-none items-center gap-1 px-3 pb-1 pt-2">
        <span role="tab" aria-selected="true" data-k2-chamfer="control" className="ed-panel-tab type-row inline-flex items-center gap-1.5 rounded-[var(--radius-pill)] border border-[var(--color-control-edge)] px-3 py-1 font-semibold text-[var(--color-text-primary)]">
          Live
          <span data-testid="ed-live-tab-count" data-stale={down ? '' : undefined} title={down ? 'As you last saw it' : undefined} className={cx('font-normal text-[var(--color-text-secondary)]', down ? 'line-through' : '')}>
            {here}
          </span>
        </span>
      </div>
      {view === 'edits' && !down ? (
        <LiveEdits snap={snap} people={people} readAloud={readAloud} onReadAloud={setReadAloud} onBack={() => setView('here')} />
      ) : (
        <>
          <WatchingList title="Here" countAway people={people} stale={down} staleHead={{ title: 'Last seen', line: 'As you last saw them, before the connection dropped.' }} action={liveEditsBtn} followingId={followingId} onFollow={down ? undefined : onFollow} />
          <ChangesList
            changes={snap.changes}
            people={people}
            now={snap.clock ?? Date.now()}
            empty="Edits by others show here as they finish."
            undoOff={snap.undoOff}
            undoTip={UNDO_ROW_TIP}
            onGoThere={onGoThere}
            onUndo={onUndo}
            onRedo={onRedo}
            onAsk={onAsk}
          />
          <MessagesList messages={snap.messages} people={people} now={snap.clock ?? Date.now()} />
        </>
      )}
      <span className="flex-1" />
      {view === 'edits' ? (
        <p className="ed-panel-foot type-hint m-0 flex-none border-t border-[var(--color-border)] px-3.5 py-2 text-[var(--color-text-secondary)]">One line per finished edit.</p>
      ) : !tipGone ? (
        <div data-testid="ed-undo-tip" className="ed-panel-foot type-hint flex flex-none items-center gap-2 border-t border-[var(--color-border)] px-3.5 py-2 text-[var(--color-text-secondary)]">
          <span className="flex-1">{UNDO_TIP}</span>
          <button type="button" data-k2-focus="" aria-label="Dismiss the tip" title="Dismiss" onClick={() => setTipGone(true)} className="lv-icon-btn no-drag inline-flex h-[22px] w-[22px] cursor-pointer items-center justify-center rounded-[var(--radius-field)]">
            <CloseGlyph size={12} />
          </button>
        </div>
      ) : null}
    </div>
  )
}

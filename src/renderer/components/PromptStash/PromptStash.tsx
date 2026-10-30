// Prompt stash UI (spec-prompt-stash-v1 "The stash button and drawer"):
//   - `StashTab`: the permanent bookmark-with-a-count on the Thread box's
//     top edge, just right of the activity bar (the Zen ear's look);
//   - `StashSquareButton`: the same button in the terminal compose bar's
//     square look, beside Send;
//   - `composePlusMenuItems`: the composer "+" menu's rows, its attach
//     choice then "Save to stash" (the phone's + menu; Rosson 2026-10-30);
//   - `StashDrawer`: the list, newest first; each row has a preview, its
//     source, its age, an attachment badge and ✕. ↑/↓ move, Enter picks,
//     Esc closes, Delete / ⌘⌫ remove the row.
// The list itself is `stores/prompt-stash`.

import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type JSX } from 'react'
import { stashAge, stashPreview, stashSourceLabel, type StashRecord } from '@/lib/prompt-stash/record'
import type { ContextMenuItemDef } from '@/stores/context-menu'
import { ZEN_DRAWER_EAR_FILL } from '@/components/Zen/widgets/ZenActivityDrawer'

/** Slide length (ms), the activity drawer's. */
export const STASH_DRAWER_MS = 200

export const STASH_CSS = `
@keyframes k2-stash-in { from { transform: translateY(calc(100% + 8px)); opacity: 0.4; } to { transform: none; opacity: 1; } }
@keyframes k2-stash-pop { 0% { transform: scale(1); } 40% { transform: scale(1.18); } 100% { transform: scale(1); } }
[data-stash-drawer] { animation: k2-stash-in ${STASH_DRAWER_MS}ms cubic-bezier(0.2, 0.9, 0.3, 1.12) both; }
[data-stash-count][data-pop="true"] { animation: k2-stash-pop 360ms ease-out; }
[data-stash-row][data-active="true"] { background: var(--stash-row-active); }
[data-stash-row]:hover { background: var(--stash-row-active); }
[data-stash-row] [data-stash-remove] { opacity: 0.55; }
[data-stash-row]:hover [data-stash-remove], [data-stash-row][data-active="true"] [data-stash-remove] { opacity: 1; }
@media (prefers-reduced-motion: reduce) {
  [data-stash-drawer], [data-stash-count] { animation: none !important; }
}
`

export function StashGlyph({ size = 12 }: { size?: number }): JSX.Element {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden style={{ display: 'block' }}>
      <path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" />
    </svg>
  )
}

function PaperclipGlyph({ size = 11 }: { size?: number }): JSX.Element {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden style={{ display: 'inline-block', verticalAlign: '-0.15em' }}>
      <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
    </svg>
  )
}

/** The count pops when it goes up (a save landed). */
function useCountPop(count: number): boolean {
  const prev = useRef(count)
  const [pop, setPop] = useState(false)
  useEffect(() => {
    if (count > prev.current) {
      setPop(true)
      const t = setTimeout(() => setPop(false), 400)
      prev.current = count
      return () => clearTimeout(t)
    }
    prev.current = count
  }, [count])
  return pop
}

/** The composer "+" menu: the attach choice (when the composer has one),
 *  then "Save to stash", which does what the chord does (save, then clear)
 *  and is dimmed while there is nothing typed or attached. */
export function composePlusMenuItems({
  attach,
  canStash,
  chord,
}: {
  attach: boolean
  canStash: boolean
  chord: string
}): ContextMenuItemDef[] {
  const items: ContextMenuItemDef[] = []
  if (attach) {
    items.push({ id: 'attach', label: 'Attach a file or image', icon: <PaperclipGlyph size={12} /> })
    items.push({ id: 'sep', label: '', type: 'separator' })
  }
  items.push({
    id: 'stash',
    label: 'Save to stash',
    icon: <StashGlyph size={12} />,
    shortcut: chord,
    enabled: canStash,
    tooltip: canStash ? undefined : 'Type or attach something to save it',
  })
  return items
}

/** About how tall `composePlusMenuItems` draws (ContextMenu rows and
 *  separators), to open it above the "+" at the bottom of the window. */
export function composePlusMenuHeight(items: readonly ContextMenuItemDef[]): number {
  let h = 10
  for (const i of items) h += i.type === 'separator' ? 9 : 27
  return h
}

function stashButtonLabel(count: number): string {
  return count === 1 ? 'Stash (1 item)' : `Stash (${count} items)`
}

/** The stash tab's width (px), the phone's. */
export const STASH_TAB_WIDTH = 44

/** The Thread box's stash button: an ear on the box's top edge. */
export function StashTab({
  count,
  open,
  onToggle,
  chord,
}: {
  count: number
  open: boolean
  onToggle: () => void
  /** "⌘S" / "Ctrl+S". */
  chord: string
}): JSX.Element {
  const pop = useCountPop(count)
  return (
    <button
      type="button"
      data-testid="stash-button"
      data-stash-tab=""
      aria-label={stashButtonLabel(count)}
      aria-expanded={open}
      aria-haspopup="listbox"
      title={`Stash: ${chord} saves what you're writing; ${chord} on an empty box opens this`}
      // Keep the caret in the message box.
      onMouseDown={(e) => e.preventDefault()}
      onClick={onToggle}
      className="inline-flex flex-none cursor-pointer items-center justify-center"
      style={{
        gap: 4,
        // 44 wide, the phone's tab (Rosson 2026-10-30; was its padding).
        width: STASH_TAB_WIDTH,
        height: 26,
        padding: 0,
        fontSize: 12,
        lineHeight: '18px',
        color: open ? 'var(--zen-accent)' : 'var(--zen-text-muted)',
        background: ZEN_DRAWER_EAR_FILL,
        border: '1px solid var(--zen-border)',
        borderBottom: 'none',
        borderRadius: 'calc(var(--zen-radius) + 2px) calc(var(--zen-radius) + 2px) 0 0',
      }}
    >
      <StashGlyph size={12} />
      <span data-stash-count="" data-pop={pop ? 'true' : 'false'} className="tabular-nums">
        {count}
      </span>
    </button>
  )
}

/** The terminal compose bar's stash button (its square look). */
export function StashSquareButton({
  count,
  open,
  onToggle,
  chord,
  size,
  marginBottom,
}: {
  count: number
  open: boolean
  onToggle: () => void
  chord: string
  size: number
  marginBottom: number
}): JSX.Element {
  const pop = useCountPop(count)
  return (
    <button
      type="button"
      data-testid="stash-button"
      aria-label={stashButtonLabel(count)}
      aria-expanded={open}
      aria-haspopup="listbox"
      title={`Stash: ${chord} saves what you're writing; ${chord} on an empty box opens this`}
      onMouseDown={(e) => e.preventDefault()}
      onClick={onToggle}
      className={`inline-flex flex-shrink-0 items-center justify-center transition-colors ${
        open
          ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)]'
          : 'bg-[var(--color-bg-hover)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-border)]'
      }`}
      style={{ borderRadius: 0, minWidth: size, height: size, marginBottom, gap: 3, padding: '0 4px', fontSize: 10 }}
    >
      <StashGlyph size={Math.max(10, Math.round(size * 0.5))} />
      {count > 0 ? (
        <span data-stash-count="" data-pop={pop ? 'true' : 'false'} className="tabular-nums">
          {count}
        </span>
      ) : null}
    </button>
  )
}

export interface StashDrawerProps {
  items: readonly StashRecord[]
  loaded: boolean
  onPick: (rec: StashRecord) => void
  onRemove: (id: string) => void
  onClose: () => void
  removeAfterUse: boolean
  onRemoveAfterUseChange: (on: boolean) => void
  chord: string
  /** `zen`: on the Thread box's top edge; `square`: the terminal bar's. */
  look: 'zen' | 'square'
  /** Clock (tests). */
  now?: number
}

/** The stash list. Takes focus when it opens, so the keys work at once. */
export function StashDrawer({
  items,
  loaded,
  onPick,
  onRemove,
  onClose,
  removeAfterUse,
  onRemoveAfterUseChange,
  chord,
  look,
  now,
}: StashDrawerProps): JSX.Element {
  const listRef = useRef<HTMLDivElement>(null)
  const [active, setActive] = useState(0)
  const at = Math.min(active, Math.max(0, items.length - 1))
  const clock = now ?? Date.now()

  useLayoutEffect(() => {
    listRef.current?.focus()
  }, [])
  useEffect(() => {
    listRef.current?.querySelector(`[data-stash-index="${at}"]`)?.scrollIntoView?.({ block: 'nearest' })
  }, [at])

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>): void => {
    const mod = e.metaKey || e.ctrlKey
    if (e.key === 'Escape' || (mod && e.key.toLowerCase() === 's')) {
      e.preventDefault()
      e.stopPropagation()
      onClose()
      return
    }
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      e.stopPropagation()
      if (items.length === 0) return
      const step = e.key === 'ArrowDown' ? 1 : -1
      setActive((cur) => (Math.min(cur, items.length - 1) + step + items.length) % items.length)
      return
    }
    if (e.key === 'Enter') {
      e.preventDefault()
      e.stopPropagation()
      const rec = items[at]
      if (rec) onPick(rec)
      return
    }
    if (e.key === 'Delete' || (mod && e.key === 'Backspace')) {
      e.preventDefault()
      e.stopPropagation()
      const rec = items[at]
      if (rec) onRemove(rec.id)
      return
    }
    // Plain keys stay here (terminal shortcuts don't fire under the list).
    if (!mod) e.stopPropagation()
  }

  const zen = look === 'zen'
  const shell: CSSProperties = zen
    ? {
        background: ZEN_DRAWER_EAR_FILL,
        border: '1px solid var(--zen-border)',
        borderBottom: 'none',
        borderRadius: 'calc(var(--zen-radius) + 2px) calc(var(--zen-radius) + 2px) 0 0',
        color: 'var(--zen-text)',
        ['--stash-row-active' as string]: 'var(--zen-surface-raised)',
      }
    : {
        background: 'var(--color-bg-elevated)',
        border: '1px solid var(--color-border)',
        borderRadius: 0,
        color: 'var(--color-text-primary)',
        ['--stash-row-active' as string]: 'var(--color-bg-hover)',
      }
  const muted = zen ? 'var(--zen-text-muted)' : 'var(--color-text-muted)'
  const activeId = items[at] ? `stash-row-${items[at].id}` : undefined

  return (
    <div data-stash-drawer="" data-testid="stash-drawer" data-look={look} className="flex min-w-0 flex-col" style={{ ...shell, overflow: 'hidden' }}>
      <style data-stash-styles="">{STASH_CSS}</style>
      <div className="flex items-center" style={{ gap: 8, padding: '6px 10px 4px 12px', fontSize: 12, color: muted }}>
        <StashGlyph size={12} />
        <span style={{ fontWeight: 600, color: zen ? 'var(--zen-text)' : 'var(--color-text-primary)' }}>Stash</span>
        <span className="tabular-nums">{items.length}</span>
        <span className="flex-1" />
        <button
          type="button"
          aria-label="Close stash"
          title="Close (Esc)"
          onMouseDown={(e) => e.preventDefault()}
          onClick={onClose}
          className="cursor-pointer"
          style={{ color: muted, background: 'none', border: 'none', fontSize: 13, lineHeight: 1, padding: 2 }}
        >
          ✕
        </button>
      </div>
      <div
        ref={listRef}
        role="listbox"
        tabIndex={0}
        aria-label="Stashed messages"
        aria-activedescendant={activeId}
        data-testid="stash-list"
        onKeyDown={onKeyDown}
        className="min-h-0 overflow-y-auto focus:outline-none"
        style={{ maxHeight: 260, padding: '0 4px 4px' }}
      >
        {items.length === 0 ? (
          <div data-testid="stash-empty" style={{ padding: '10px 8px 12px', fontSize: 12, color: muted }}>
            {loaded ? `Nothing stashed yet. ${chord} saves what you're writing and clears the box.` : 'Loading…'}
          </div>
        ) : (
          items.map((rec, i) => {
            const files = rec.attachments.length
            const source = stashSourceLabel(rec.source)
            return (
              <div
                key={rec.id}
                id={`stash-row-${rec.id}`}
                role="option"
                aria-selected={i === at}
                data-stash-row=""
                data-stash-index={i}
                data-active={i === at ? 'true' : 'false'}
                onMouseDown={(e) => e.preventDefault()}
                onMouseEnter={() => setActive(i)}
                onClick={() => onPick(rec)}
                className="flex min-w-0 cursor-pointer items-start"
                style={{ gap: 8, padding: '6px 8px', borderRadius: zen ? 8 : 0 }}
              >
                <div className="flex min-w-0 flex-1 flex-col" style={{ gap: 1 }}>
                  <span data-stash-preview="" className="min-w-0 truncate" style={{ fontSize: 13, lineHeight: '18px' }}>
                    {stashPreview(rec) || '(empty)'}
                  </span>
                  <span className="flex min-w-0 items-center" style={{ gap: 6, fontSize: 11, lineHeight: '15px', color: muted }}>
                    {source ? <span data-stash-source="" className="min-w-0 truncate">{source}</span> : null}
                    <span data-stash-age="" className="flex-none tabular-nums">{stashAge(rec.created_at, clock)}</span>
                    {files > 0 ? (
                      <span data-stash-files={files} className="flex-none" title={rec.attachments.map((a) => a.name).join(', ')}>
                        <PaperclipGlyph /> {files}
                      </span>
                    ) : null}
                  </span>
                </div>
                <button
                  type="button"
                  data-stash-remove=""
                  aria-label={`Remove “${stashPreview(rec, 40)}” from the stash`}
                  title="Remove from the stash"
                  onMouseDown={(e) => e.preventDefault()}
                  onClick={(e) => {
                    e.stopPropagation()
                    onRemove(rec.id)
                  }}
                  className="flex-none cursor-pointer"
                  style={{ color: muted, background: 'none', border: 'none', fontSize: 12, lineHeight: '18px', padding: '0 2px' }}
                >
                  ✕
                </button>
              </div>
            )
          })
        )}
      </div>
      <label
        className="flex cursor-pointer items-center"
        style={{ gap: 6, padding: '5px 12px 7px', fontSize: 11, color: muted, borderTop: `1px solid ${zen ? 'var(--zen-border)' : 'var(--color-border)'}` }}
      >
        <input
          type="checkbox"
          data-testid="stash-remove-after-use"
          checked={removeAfterUse}
          onChange={(e) => onRemoveAfterUseChange(e.target.checked)}
        />
        Remove after use
      </label>
    </div>
  )
}

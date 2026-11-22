// The board's name in its tab (Rosson 2026-11-11: "you can't rename the board from inside it"). K2's own DOM over the
// sealed frame's top-left corner: the frame is told how wide this is (`--zen-board-name-w`) and hides its own name
// (`--zen-board-name-host: none`), so its bar starts just right of this one. A frame from an older daemon keeps its
// name, under this one.
//
// The name is the file's name without `.k5board`; renaming renames the file (the pane's `rename`, `fs/rename`).
// It's also the board switcher: a menu of this workspace's other boards, then Rename… (F2 goes straight to the edit).

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Button } from '@/components/ui/Button'
import { Input } from '@/components/ui/Input'
import { SearchField } from '@/components/ui/SearchField'
import { floatMaterial, MENU_LIST, MENU_ROW } from '@/components/ui/float-material'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { PRODUCT_NAME } from '@/lib/brand'

export const BOARD_FILE_EXT = '.k5board'

const baseName = (p: string): string => p.split(/[\\/]/).pop() ?? p
/** What the person calls the board: the file's name without its extension. */
export const boardNameOf = (path: string): string => baseName(path).replace(/\.k5board$/i, '')

/** Why `name` can't be a board's name, or null. Nothing is sent for these. */
export function boardNameProblem(name: string): string | null {
  const v = name.trim()
  if (!v) return 'Give the board a name.'
  if (/[\\/]/.test(v)) return 'A board name can’t have a slash.'
  return null
}

/** The daemon's refusal, in words: a taken name is said plainly; anything else is general, with its reason kept. */
export function boardRenameRefusal(err: unknown): { text: string; reason: string | null } {
  const reason = err instanceof Error ? err.message : String(err)
  if (/already exists/i.test(reason)) return { text: 'A board with that name is already here.', reason: null }
  return { text: 'Couldn’t rename the board.', reason }
}

/** A board the switcher can open: a `.k5board` in the same workspace. */
export interface BoardMenuRow {
  path: string
  name: string
  /** Where it is in its workspace, for two boards with one name. */
  rel: string
}

/** The workspace's boards, now and after each change; `fail` once when the list can't be read. */
export interface BoardMenuSource {
  subscribe(push: (rows: BoardMenuRow[]) => void, fail: (err: unknown) => void): () => void
}

/** More boards than this and the menu gets a search field. */
export const BOARD_MENU_SEARCH_AT = 8

type MenuList = { kind: 'loading' } | { kind: 'error' } | { kind: 'ok'; rows: BoardMenuRow[] }

export interface BoardFileNameProps {
  filePath: string
  /** The switcher's list; without one the name only renames. */
  boards?: BoardMenuSource
  /** Open another board in this tab. */
  pick?(path: string): void
  /** Rename the file to `fileName` (with the extension). Throws the daemon's error on a refusal. */
  rename(fileName: string): Promise<void>
  /** How wide this is, so the frame's bar starts after it. */
  onWidth?(px: number): void
}

export function BoardFileName({ filePath, boards, pick, rename, onWidth }: BoardFileNameProps): React.JSX.Element {
  const name = boardNameOf(filePath)
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(name)
  const [problem, setProblem] = useState<{ text: string; reason: string | null } | null>(null)
  const [busy, setBusy] = useState(false)
  const boxRef = useRef<HTMLDivElement | null>(null)
  const nameRef = useRef<HTMLButtonElement | null>(null)
  const inputRef = useRef<HTMLInputElement | null>(null)
  const done = useRef(false)

  useLayoutEffect(() => {
    const el = boxRef.current
    if (!el || !onWidth) return undefined
    const tell = (): void => onWidth(Math.ceil(el.getBoundingClientRect().width))
    tell()
    if (typeof ResizeObserver === 'undefined') return undefined
    const ro = new ResizeObserver(tell)
    ro.observe(el)
    return () => ro.disconnect()
  }, [onWidth])

  useEffect(() => {
    if (!editing) return
    inputRef.current?.focus()
    inputRef.current?.select()
  }, [editing])

  const start = (): void => {
    done.current = false
    setDraft(name)
    setProblem(null)
    setEditing(true)
  }
  const stop = (): void => {
    done.current = true
    setEditing(false)
    setProblem(null)
    setBusy(false)
    requestAnimationFrame(() => nameRef.current?.focus({ preventScroll: true }))
  }
  const save = async (): Promise<void> => {
    if (busy || done.current) return
    const v = draft.replace(/\s+/g, ' ').trim()
    if (v === name) return stop()
    const why = boardNameProblem(v)
    if (why) {
      setProblem({ text: why, reason: null })
      return
    }
    setBusy(true)
    setProblem(null)
    try {
      await rename(`${v}${BOARD_FILE_EXT}`)
      // The pane follows the file to its new path and draws this again with the new name.
      done.current = true
      setEditing(false)
      setBusy(false)
    } catch (err) {
      setBusy(false)
      setProblem(boardRenameRefusal(err))
      inputRef.current?.focus()
    }
  }

  const [menuOpen, setMenuOpen] = useState(false)
  const onNameKey = (e: React.KeyboardEvent): void => {
    if (e.key === 'F2') {
      e.preventDefault()
      setMenuOpen(false)
      start()
    } else if (e.key === 'ArrowDown' && boards) {
      e.preventDefault()
      setMenuOpen(true)
    } else if (e.key === 'Enter' && !boards) {
      e.preventDefault()
      start()
    }
  }
  const nameRefCb = (el: HTMLButtonElement | null): void => {
    nameRef.current = el
    menuAnchor.current = el
  }
  const menuAnchor = useRef<HTMLButtonElement | null>(null)

  return (
    <div
      ref={boxRef}
      data-board-file-name=""
      data-k2-chamfer="surface"
      {...floatMaterial('popover', 'frost')}
      className="absolute left-3 top-3 z-10 flex h-[42px] max-w-[min(360px,60%)] items-center border border-[var(--color-border)] bg-[var(--color-bg-elevated)] px-1 [box-shadow:var(--shadow-2)]"
    >
      {editing ? (
        <div className="relative flex items-center gap-2">
          <Input
            ref={inputRef}
            data-testid="board-file-name-input"
            aria-label="Board name"
            aria-invalid={problem ? true : undefined}
            aria-describedby={problem ? 'board-file-name-problem' : undefined}
            className="!type-name h-[34px] w-[220px] font-semibold"
            value={draft}
            disabled={busy}
            spellCheck={false}
            onChange={(e) => {
              setDraft(e.target.value)
              setProblem(null)
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault()
                void save()
              } else if (e.key === 'Escape') {
                e.preventDefault()
                stop()
              }
            }}
            onBlur={() => {
              // Blur saves a change, as rename fields do; with no change it cancels.
              if (done.current || busy) return
              if (draft.replace(/\s+/g, ' ').trim() === name) stop()
              else void save()
            }}
          />
          {problem ? (
            <span
              id="board-file-name-problem"
              role="alert"
              data-testid="board-file-name-problem"
              className="type-hint absolute left-0 top-[calc(100%+10px)] w-max max-w-[320px] border border-[var(--color-border)] bg-[var(--color-bg-elevated)] px-2 py-1 text-[var(--color-text-primary)] [box-shadow:var(--shadow-2)]"
              data-k2-chamfer="surface"
            >
              {problem.text}
              {problem.reason ? <span className="ml-1 text-[var(--color-text-muted)]">{problem.reason}</span> : null}
            </span>
          ) : null}
        </div>
      ) : (
        <Button
          ref={nameRefCb}
          variant="quiet"
          data-testid="board-file-name"
          aria-label={boards ? `Board name: ${name}. Press Enter for your boards, F2 to rename` : `Board name: ${name}, press Enter to rename`}
          aria-haspopup={boards ? 'menu' : undefined}
          aria-expanded={boards ? menuOpen : undefined}
          title={boards ? 'Your boards, and Rename (F2)' : 'Rename the board'}
          className={`!type-name flex h-[34px] max-w-full items-center gap-1 !border-transparent font-semibold !text-[var(--color-text-primary)] ${boards ? 'cursor-pointer' : 'cursor-text'}`}
          onClick={boards ? () => setMenuOpen((v) => !v) : start}
          onKeyDown={onNameKey}
        >
          <span className="min-w-0 truncate">{name}</span>
          {boards ? (
            <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true" className={`flex-none text-[var(--color-text-muted)] transition-transform ${menuOpen ? 'rotate-180' : ''}`}>
              <path d="M3 4.5 6 7.5 9 4.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          ) : null}
        </Button>
      )}
      {boards && menuOpen && !editing ? (
        <BoardMenu
          anchor={menuAnchor}
          source={boards}
          current={filePath}
          onClose={(refocus) => {
            setMenuOpen(false)
            if (refocus) nameRef.current?.focus({ preventScroll: true })
          }}
          onPick={(path) => {
            setMenuOpen(false)
            if (path !== filePath) pick?.(path)
          }}
          onRename={() => {
            setMenuOpen(false)
            start()
          }}
        />
      ) : null}
    </div>
  )
}

/** A row lit in the menu's shape (MENU_ROW), with the shared row ring for
 *  keyboard focus (data-k2-focus) in the same shape. */
const ROW = { ...MENU_ROW, 'data-k2-focus': '' }
const ITEM = [
  'no-drag flex w-full items-center gap-2 px-3 py-1.5 text-left type-control transition-colors cursor-pointer',
  'text-[var(--color-text-secondary)] hover:bg-[var(--color-wash-1)] hover:text-[var(--color-text-primary)]',
  'relative focus:outline-none focus-visible:bg-[var(--color-wash-1)] focus-visible:text-[var(--color-text-primary)]',
].join(' ')

const itemsIn = (root: HTMLElement | null): HTMLElement[] =>
  root ? Array.from(root.querySelectorAll<HTMLElement>('[role="menuitem"], [role="menuitemradio"]')) : []

/** The switcher: the shared menu's look (cut box in Terminal, Frost in Glass) and keys, with a list that loads. */
function BoardMenu(props: {
  anchor: React.MutableRefObject<HTMLButtonElement | null>
  source: BoardMenuSource
  current: string
  onClose(refocus: boolean): void
  onPick(path: string): void
  onRename(): void
}): React.JSX.Element | null {
  const { source, current, onClose, onPick, onRename } = props
  const [list, setList] = useState<MenuList>({ kind: 'loading' })
  const [tries, setTries] = useState(0)
  const [query, setQuery] = useState('')
  const listRef = useRef<HTMLDivElement | null>(null)
  const searchRef = useRef<HTMLInputElement | null>(null)
  const menu = useAnchoredMenu<HTMLButtonElement>({ open: true, onClose: () => onClose(false), gap: 6, width: 'min', minWidth: 240, estimatedHeight: 320 })
  // The trigger is the name's button; the hook measures from it.
  menu.anchorRef.current = props.anchor.current

  useEffect(() => {
    setList({ kind: 'loading' })
    return source.subscribe(
      (rows) => setList({ kind: 'ok', rows }),
      () => setList({ kind: 'error' }),
    )
  }, [source, tries])

  const rows = list.kind === 'ok' ? list.rows : []
  const searching = rows.length > BOARD_MENU_SEARCH_AT
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase()
    return q ? rows.filter((r) => r.name.toLowerCase().includes(q) || r.rel.toLowerCase().includes(q)) : rows
  }, [rows, query])
  const others = rows.filter((r) => r.path !== current).length

  // Opening puts focus in the search field when there is one, else on the current board (or the first item).
  const ready = list.kind
  useEffect(() => {
    if (searching) {
      searchRef.current?.focus({ preventScroll: true })
      return
    }
    const items = itemsIn(listRef.current)
    const here = items.find((el) => el.getAttribute('aria-checked') === 'true')
    ;(here ?? items[0])?.focus({ preventScroll: true })
  }, [ready, searching])

  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      onClose(true)
      return
    }
    if (e.key === 'Tab') {
      onClose(false)
      return
    }
    const items = itemsIn(listRef.current)
    if (items.length === 0) return
    const at = items.indexOf(document.activeElement as HTMLElement)
    let next: number | null = null
    if (e.key === 'ArrowDown') next = at < 0 ? 0 : (at + 1) % items.length
    else if (e.key === 'ArrowUp') next = at <= 0 ? items.length - 1 : at - 1
    else if (e.key === 'Home' && at >= 0) next = 0
    else if (e.key === 'End' && at >= 0) next = items.length - 1
    else if (at >= 0 && e.key.length === 1 && /\S/.test(e.key) && !e.metaKey && !e.ctrlKey && !e.altKey) {
      // Type-ahead: the next item that starts with the key.
      const k = e.key.toLowerCase()
      for (let i = 1; i <= items.length; i++) {
        const el = items[(at + i) % items.length]
        if ((el.textContent ?? '').trim().toLowerCase().startsWith(k)) {
          next = (at + i) % items.length
          break
        }
      }
    }
    if (next === null) return
    e.preventDefault()
    items[next].focus({ preventScroll: true })
  }

  return menu.portal(
    <div ref={menu.menuRef} style={menu.style} data-k2-chamfer-float="menu" className="no-drag w-max max-w-[22rem]" data-board-file-menu="">
      <div
        ref={listRef}
        role="menu"
        aria-label="Your boards"
        data-k2-chamfer="surface"
        {...floatMaterial('menu', 'frost')}
        {...MENU_LIST}
        style={{ '--k2-menu-origin': 'top left' } as React.CSSProperties}
        className="flex max-h-[min(calc(var(--k2-vh)*0.6),420px)] flex-col border border-[var(--color-border)] bg-[var(--color-bg)] [box-shadow:var(--shadow-3)]"
        onKeyDown={onKeyDown}
      >
        {searching ? (
          <div className="pb-1">
            <SearchField
              ref={searchRef}
              data-testid="board-file-menu-search"
              aria-label="Search your boards"
              placeholder="Search boards"
              value={query}
              escapeClears={false}
              onChange={(e) => setQuery(e.target.value)}
              onClear={() => setQuery('')}
            />
          </div>
        ) : null}
        <div className="min-h-0 overflow-y-auto">
          {list.kind === 'loading' ? (
            <div role="status" className="type-control px-3 py-1.5 text-[var(--color-text-muted)]">Loading boards…</div>
          ) : list.kind === 'error' ? (
            <div className="flex items-center gap-2 px-3 py-1.5 type-control text-[var(--color-text-secondary)]" data-testid="board-file-menu-error">
              <span>{PRODUCT_NAME} couldn’t list your boards.</span>
              <button type="button" role="menuitem" {...ROW} className={`${ITEM} !w-auto !px-1 underline`} onClick={() => setTries((n) => n + 1)}>
                Try again
              </button>
            </div>
          ) : others === 0 && !query ? (
            <div className="type-control px-3 py-1.5 text-[var(--color-text-muted)]">No other boards here.</div>
          ) : shown.length === 0 ? (
            <div className="type-control px-3 py-1.5 text-[var(--color-text-muted)]">No boards match.</div>
          ) : (
            shown.map((r) => {
              const here = r.path === current
              const twin = rows.some((o) => o !== r && o.name === r.name)
              return (
                <button
                  key={r.path}
                  type="button"
                  role="menuitemradio"
                  aria-checked={here}
                  data-board-file-menu-row={r.path}
                  {...ROW}
                  title={r.rel}
                  className={`${ITEM} ${here ? '!text-[var(--color-text-primary)] font-semibold' : ''}`}
                  onClick={() => onPick(r.path)}
                >
                  <span className="w-3 flex-none text-[var(--color-accent-text)]" aria-hidden="true">{here ? '✓' : ''}</span>
                  <span className="min-w-0 truncate">{r.name}</span>
                  {twin ? <span className="min-w-0 truncate type-hint text-[var(--color-text-muted)]">{r.rel}</span> : null}
                </button>
              )
            })
          )}
        </div>
        <div role="separator" data-menu-separator="" className="my-1 border-t border-[var(--k2-menu-separator,var(--color-border))]" />
        <button type="button" role="menuitem" data-menu-item="rename" {...ROW} className={ITEM} onClick={onRename}>
          <span className="w-3 flex-none" aria-hidden="true" />
          <span>Rename…</span>
          <span className="ml-auto pl-4 type-hint text-[var(--color-text-muted)]">F2</span>
        </button>
      </div>
    </div>,
  )
}

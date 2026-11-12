// The board's name in its tab (Rosson 2026-11-11: "you can't rename the board from inside it"). K2's own DOM over the
// sealed frame's top-left corner: the frame is told how wide this is (`--zen-board-name-w`) and hides its own name
// (`--zen-board-name-host: none`), so its bar starts just right of this one. A frame from an older daemon keeps its
// name, under this one.
//
// The name is the file's name without `.k5board`; renaming renames the file (the pane's `rename`, `fs/rename`).

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/Button'
import { Input } from '@/components/ui/Input'
import { floatMaterial } from '@/components/ui/float-material'

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

export interface BoardFileNameProps {
  filePath: string
  /** Rename the file to `fileName` (with the extension). Throws the daemon's error on a refusal. */
  rename(fileName: string): Promise<void>
  /** How wide this is, so the frame's bar starts after it. */
  onWidth?(px: number): void
}

export function BoardFileName({ filePath, rename, onWidth }: BoardFileNameProps): React.JSX.Element {
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

  const onNameKey = (e: React.KeyboardEvent): void => {
    if (e.key === 'F2' || e.key === 'Enter') {
      e.preventDefault()
      start()
    }
  }

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
          ref={nameRef}
          variant="quiet"
          data-testid="board-file-name"
          aria-label={`Board name: ${name}, press Enter to rename`}
          title="Rename the board"
          className="!type-name h-[34px] max-w-full cursor-text truncate !border-transparent font-semibold !text-[var(--color-text-primary)]"
          onClick={start}
          onKeyDown={onNameKey}
        >
          {name}
        </Button>
      )}
    </div>
  )
}

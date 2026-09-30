// Home P1 (vs-live H12/H13) — the Home picker in the page header.
//
// One Home tab on the top switcher; which Home is showing is picked here
// (a user-named tab per Home would have no width limit). Create, rename,
// and delete live in the same menu. The last Home cannot be deleted.

import { useEffect, useRef, useState } from 'react'
import { useHomesStore, selectedHome, HOME_NAME_MAX } from '@/stores/homes'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { Button, Input } from '@/components/ui'

type Mode = 'menu' | 'create' | 'rename'

export default function HomePicker(): React.JSX.Element {
  const homes = useHomesStore((s) => s.homes)
  const current = useHomesStore(selectedHome)
  const [open, setOpen] = useState(false)
  const [mode, setMode] = useState<Mode>('menu')
  const [draft, setDraft] = useState('')
  const rootRef = useRef<HTMLDivElement | null>(null)

  useEffect(() => {
    if (!open) {
      setMode('menu')
      return
    }
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && e.target instanceof Node && !rootRef.current.contains(e.target)) {
        setOpen(false)
      }
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKey)
    }
  }, [open])

  const startCreate = (): void => {
    setDraft('')
    setMode('create')
  }
  const startRename = (): void => {
    setDraft(current.name)
    setMode('rename')
  }
  const submit = (): void => {
    const st = useHomesStore.getState()
    const ok = mode === 'create' ? st.createHome(draft) !== null : st.renameHome(current.id, draft)
    if (ok) setOpen(false)
  }
  const remove = async (): Promise<void> => {
    setOpen(false)
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: 'Delete Home',
      message: `Delete “${current.name}”? Its agents stay where they are. Only this list goes.`,
      confirmLabel: 'Delete',
      destructive: true,
    })
    if (confirmed) useHomesStore.getState().deleteHome(current.id)
  }

  const itemClass =
    'no-drag w-full text-left px-3 py-1.5 text-[11px] text-[var(--color-text-secondary)] hover:bg-white/[0.06] hover:text-[var(--color-text-primary)] cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-transparent'

  return (
    <div ref={rootRef} className="relative min-w-0">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="no-drag flex max-w-full items-center gap-1.5 px-2 py-1 text-sm font-medium text-[var(--color-text-primary)] hover:bg-white/[0.06] cursor-pointer"
        title="Pick a Home"
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <span className="truncate">{current.name}</span>
        <svg width="9" height="9" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.5" className="flex-shrink-0 text-[var(--color-text-muted)]">
          <path d="M2 3.5l3 3 3-3" />
        </svg>
      </button>

      {open && (
        <div
          role="menu"
          className="absolute left-0 top-full z-30 mt-1 w-64 max-w-[calc(100vw-32px)] border border-[var(--color-border)] bg-[var(--color-bg-elevated)] py-1 shadow-lg"
        >
          {mode === 'menu' ? (
            <>
              <div className="max-h-64 overflow-y-auto">
                {homes.map((h) => (
                  <button
                    key={h.id}
                    type="button"
                    role="menuitemradio"
                    aria-checked={h.id === current.id}
                    className={`${itemClass} flex items-center gap-2`}
                    onClick={() => {
                      useHomesStore.getState().selectHome(h.id)
                      setOpen(false)
                    }}
                  >
                    <span className="w-3 flex-shrink-0 text-[var(--color-accent)]">{h.id === current.id ? '✓' : ''}</span>
                    <span className="truncate flex-1">{h.name}</span>
                    <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">{h.rows.length}</span>
                  </button>
                ))}
              </div>
              <div className="my-1 border-t border-[var(--color-border)]" />
              <button type="button" role="menuitem" className={itemClass} onClick={startCreate}>
                New Home…
              </button>
              <button type="button" role="menuitem" className={itemClass} onClick={startRename}>
                Rename “{current.name}”…
              </button>
              <button
                type="button"
                role="menuitem"
                className={itemClass}
                disabled={homes.length <= 1}
                title={homes.length <= 1 ? 'The last Home can’t be deleted' : undefined}
                onClick={() => void remove()}
              >
                Delete “{current.name}”
              </button>
            </>
          ) : (
            <form
              className="flex flex-col gap-2 px-3 py-2"
              onSubmit={(e) => {
                e.preventDefault()
                submit()
              }}
            >
              <label className="text-[10px] uppercase tracking-wide text-[var(--color-text-muted)]">
                {mode === 'create' ? 'New Home name' : 'Rename Home'}
              </label>
              <Input
                autoFocus
                value={draft}
                maxLength={HOME_NAME_MAX}
                onChange={(e) => setDraft(e.target.value)}
                placeholder="Home"
              />
              <div className="flex justify-end gap-2">
                <Button variant="ghost" onClick={() => setMode('menu')}>
                  Cancel
                </Button>
                <Button variant="accent" type="submit" disabled={draft.trim().length === 0}>
                  {mode === 'create' ? 'Create' : 'Save'}
                </Button>
              </div>
            </form>
          )}
        </div>
      )}
    </div>
  )
}

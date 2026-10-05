// Home P1 (vs-live H12/H13) — the Home picker at the top of the Home
// sidebar, in the spot (and the look) of the Agents sidebar's focus-group
// dropdown (`Sidebar/FocusGroupDropdown`).
//
// One Home tab on the top switcher; which Home is showing is picked here
// (a user-named tab per Home would have no width limit). Create, rename,
// and delete live in the same menu. The last Home cannot be deleted.

import { useEffect, useState } from 'react'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { useHomesStore, selectedHome, HOME_NAME_MAX } from '@/stores/homes'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { Button, Input } from '@/components/ui'
import { ShortcutIndexBadge } from '@/components/Sidebar/Sidebar'
import { HOME_SWITCH_LIMIT, homeSwitchCombo } from '@/lib/home-shortcuts'

type Mode = 'menu' | 'create' | 'rename'

// The button carries the row count; the rows' "⌘ 1-9" hint sits beside
// the picker in the sidebar header (HomeSidebar). Each Home in the menu
// carries its Home switcher chord (HOME_SWITCH_BINDING) in the row badge's
// look.
export default function HomePicker(): React.JSX.Element {
  const homes = useHomesStore((s) => s.homes)
  const current = useHomesStore(selectedHome)
  const [open, setOpen] = useState(false)
  const [mode, setMode] = useState<Mode>('menu')
  const [draft, setDraft] = useState('')
  // Portalled and fixed to the picker: the sidebar's scroller
  // (Layout's `overflow-y-auto`) can't cut it off. Outside click / Escape close.
  const menu = useAnchoredMenu<HTMLDivElement>({ open, onClose: () => setOpen(false) })

  useEffect(() => {
    if (!open) setMode('menu')
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

  // Same classes as FocusGroupDropdown's options.
  const itemClass =
    'w-full flex items-center gap-2 px-3 py-1.5 text-left text-[11px] transition-colors cursor-pointer text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)] disabled:opacity-40 disabled:cursor-not-allowed disabled:hover:bg-transparent'

  return (
    <div ref={menu.anchorRef} className="relative no-drag">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="w-full flex items-center gap-2 px-2 py-1.5 text-left bg-[var(--color-bg)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)] transition-colors cursor-pointer"
        title="Pick a Home"
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <span className="text-[11px] font-semibold text-[var(--color-text-primary)] uppercase tracking-wide truncate flex-1">
          {current.name}
        </span>
        <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums px-1.5 py-0.5 bg-white/[0.06] font-mono flex-shrink-0">
          {current.rows.length}
        </span>
        <svg
          className={`w-3 h-3 text-[var(--color-text-muted)] flex-shrink-0 transition-transform ${open ? 'rotate-180' : ''}`}
          fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {open && menu.portal(
        <div
          ref={menu.menuRef}
          role="menu"
          data-home-picker-menu=""
          data-placement={menu.placement}
          style={menu.style}
          className="no-drag bg-[var(--color-bg)] border border-[var(--color-border)] shadow-xl"
        >
          {mode === 'menu' ? (
            <>
              <div className="max-h-48 overflow-y-auto py-0.5">
                {homes.map((h, idx) => {
                  const isCurrent = h.id === current.id
                  return (
                    <button
                      key={h.id}
                      type="button"
                      role="menuitemradio"
                      aria-checked={isCurrent}
                      className={`${itemClass} ${isCurrent ? '!text-[var(--color-accent)]' : ''}`}
                      onClick={() => {
                        useHomesStore.getState().selectHome(h.id)
                        setOpen(false)
                      }}
                    >
                      <span className="w-2 flex-shrink-0" />
                      <span className="truncate flex-1">{h.name}</span>
                      <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)] tabular-nums">{h.rows.length}</span>
                      {idx < HOME_SWITCH_LIMIT && (
                        <ShortcutIndexBadge index={idx} combo={homeSwitchCombo(idx + 1)} />
                      )}
                      {isCurrent && (
                        <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-accent)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                          <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                        </svg>
                      )}
                    </button>
                  )
                })}
              </div>
              <div className="border-t border-[var(--color-border)]" />
              <div className="py-0.5">
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
              </div>
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

// Agents sidebar right-click → "Add to Home", when this device has more
// than one Home. Lists the Homes with a checkbox each; Homes that already
// have the agent show checked and can't be picked. Add puts the agent on
// every ticked Home through the same add Home's own picker uses
// (lib/add-to-home.ts). Esc cancels, Enter adds.
//
// One Home never reaches this dialog — `addAgentToHomeFromMenu` adds
// straight away there.

import React, { useEffect, useMemo, useRef, useState } from 'react'
import { DialogScrim, SquareCheckbox } from '@/components/ui'
import { useAddToHomeDialogStore } from '@/stores/add-to-home-dialog'
import { useHomesStore } from '@/stores/homes'
import { useToastStore } from '@/stores/toast'
import { addWorkspaceToHome, addedToHomesMessage, homeAddressFor, homeHasAddress } from '@/lib/add-to-home'

export default function AddToHomeDialog(): React.JSX.Element | null {
  const isOpen = useAddToHomeDialogStore((s) => s.isOpen)
  const hostKey = useAddToHomeDialogStore((s) => s.hostKey)
  const workspace = useAddToHomeDialogStore((s) => s.workspace)
  const close = useAddToHomeDialogStore((s) => s.close)
  const homes = useHomesStore((s) => s.homes)
  const selectedId = useHomesStore((s) => s.selectedId)

  const address = workspace && hostKey ? homeAddressFor(hostKey, workspace) : null
  const already = useMemo(
    () => new Set(address ? homes.filter((h) => homeHasAddress(h, address)).map((h) => h.id) : []),
    [homes, address],
  )

  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set())
  const firstBox = useRef<HTMLInputElement>(null)

  // Reset on every open: this window's selected Home is ticked, unless the
  // agent is already on it.
  useEffect(() => {
    if (!isOpen) return
    const s = useHomesStore.getState()
    const sel = s.homes.find((h) => h.id === s.selectedId)
    const on = sel && address && !homeHasAddress(sel, address) ? [sel.id] : []
    setPicked(new Set(on))
    requestAnimationFrame(() => firstBox.current?.focus())
    // Only on open / a new agent; later Home edits keep the user's ticks.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen, address])

  // Ticks only ever name Homes that still exist and don't have the agent.
  const chosen = homes.filter((h) => picked.has(h.id) && !already.has(h.id))

  const confirm = (): void => {
    if (!workspace || !hostKey || chosen.length === 0) return
    for (const h of chosen) addWorkspaceToHome(h, hostKey, workspace)
    useToastStore.getState().addToast(addedToHomesMessage(chosen.map((h) => h.name)), 'success', 3000)
    close()
  }
  const confirmRef = useRef(confirm)
  confirmRef.current = confirm

  // Esc cancels, Enter adds — from anywhere in the dialog (a focused button
  // keeps its own Enter).
  useEffect(() => {
    if (!isOpen) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        close()
      } else if (e.key === 'Enter') {
        if (e.target instanceof HTMLButtonElement) return
        e.preventDefault()
        confirmRef.current()
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [isOpen, close])

  if (!isOpen || !workspace || !hostKey) return null

  const toggle = (id: string): void => {
    setPicked((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  const everyHomeHasIt = homes.every((h) => already.has(h.id))
  let firstEnabled = true

  return (
    <DialogScrim
      zIndex={50}
      className="flex items-center justify-center no-drag"
      style={{ backdropFilter: 'blur(4px)' }}
      onClick={close}
    >
      <div
        role="dialog"
        aria-label="Add to Home"
        className="w-[360px] flex flex-col border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-5 pt-5 pb-3">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Add {workspace.name} to Home</h2>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-1">
            {everyHomeHasIt ? 'It is already on every Home.' : 'Pick the Homes to add it to.'}
          </p>
          <ul className="mt-3 flex flex-col max-h-[50vh] overflow-y-auto" aria-label="Homes">
            {homes.map((h) => {
              const has = already.has(h.id)
              const ref = !has && firstEnabled ? firstBox : undefined
              if (!has) firstEnabled = false
              const id = `add-to-home-${h.id}`
              return (
                <li key={h.id}>
                  <label
                    htmlFor={id}
                    className={`flex items-center gap-2 px-2 py-1.5 text-xs ${
                      has
                        ? 'text-[var(--color-text-muted)] cursor-default'
                        : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)] cursor-pointer'
                    }`}
                  >
                    <SquareCheckbox
                      id={id}
                      ref={ref}
                      checked={has || picked.has(h.id)}
                      disabled={has}
                      onChange={() => toggle(h.id)}
                    />
                    <span className="truncate flex-1">{h.name}</span>
                    {has ? (
                      <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">Already added</span>
                    ) : h.id === selectedId ? (
                      <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">Current</span>
                    ) : null}
                  </label>
                </li>
              )
            })}
          </ul>
        </div>

        <div className="px-5 pb-5 flex gap-2 justify-end border-t border-[var(--color-border)] pt-3">
          <button
            type="button"
            className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]"
            onClick={close}
          >
            {everyHomeHasIt ? 'Close' : 'Cancel'}
          </button>
          <button
            type="button"
            className="px-3 py-1.5 text-xs font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] disabled:opacity-40"
            onClick={confirm}
            disabled={chosen.length === 0}
          >
            Add
          </button>
        </div>
      </div>
    </DialogScrim>
  )
}

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { registerLeaveGuard, type LeaveChoice } from '@/stores/tabs'

// Unsaved-changes leave guard UI.
//
// When the user switches AWAY from a workspace that has dirty file tabs, the
// projects store (`setActiveProject` / `setActiveWorkspace`) calls the guard
// registered here. The guard shows this modal and returns a Promise that the
// Save / Discard / Cancel buttons resolve:
//   • Save changes (primary) → 'save'    — proceed; the FileViewerPane
//        unmount-flush writes each dirty buffer to disk.
//   • Discard               → 'discard'  — proceed; dirty tabs are marked
//        discard-pending so the unmount-flush SKIPS the write.
//   • Cancel / Esc / overlay → 'cancel'  — stay on the current workspace.
//
// Mount <UnsavedChangesModalHost /> once at the app's top level. It registers
// the guard on mount; before it mounts (startup) the store's runLeaveGuard
// defaults to 'save', so a switch is never blocked.

interface UnsavedChangesModalProps {
  onChoose: (choice: LeaveChoice) => void
}

function UnsavedChangesModal({ onChoose }: UnsavedChangesModalProps): React.JSX.Element {
  // Esc == Cancel.
  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        onChoose('cancel')
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [onChoose])

  return (
    <div
      className="fixed inset-0 z-[60] flex items-center justify-center no-drag"
      style={{ backgroundColor: 'rgba(0, 0, 0, 0.6)', backdropFilter: 'blur(4px)' }}
      onClick={() => onChoose('cancel')}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Unsaved changes"
        className="w-[440px] max-w-[92vw] border border-[var(--color-border)] bg-[var(--color-bg-surface)] shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-5 pt-5 pb-2">
          <h2 className="text-sm font-medium text-[var(--color-text-primary)]">
            Unsaved changes
          </h2>
          <p className="text-xs text-[var(--color-text-muted)] mt-2 leading-relaxed">
            You have unsaved edits in this workspace. Save them before leaving?
          </p>
        </div>

        <div className="px-5 pb-5 pt-3 flex items-center justify-end gap-2">
          <button
            onClick={() => onChoose('cancel')}
            className="px-3 py-1.5 text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] transition-colors no-drag cursor-pointer"
          >
            Cancel
          </button>
          <button
            onClick={() => onChoose('discard')}
            className="px-3 py-1.5 text-xs border border-[var(--color-border)] bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-surface)] transition-colors no-drag cursor-pointer"
          >
            Discard
          </button>
          <button
            onClick={() => onChoose('save')}
            autoFocus
            className="px-3 py-1.5 text-xs border border-[var(--color-accent)]/40 bg-[var(--color-accent)]/10 text-[var(--color-accent)] hover:bg-[var(--color-accent)]/20 transition-colors no-drag cursor-pointer"
          >
            Save changes
          </button>
        </div>
      </div>
    </div>
  )
}

/** Top-level host: registers the leave guard and renders the modal on demand.
 *  Mount once near the app root. */
export function UnsavedChangesModalHost(): React.JSX.Element | null {
  const [open, setOpen] = useState(false)
  // The pending guard promise's resolver. Set when the guard fires; called by
  // a button click to resolve the projects-store await with the user's choice.
  const resolveRef = useRef<((choice: LeaveChoice) => void) | null>(null)

  useEffect(() => {
    registerLeaveGuard((_leavingProjectId: string): Promise<LeaveChoice> => {
      // If a prior modal is somehow still pending, resolve it as cancel so we
      // never leak a hung promise; then open fresh.
      if (resolveRef.current) {
        resolveRef.current('cancel')
        resolveRef.current = null
      }
      return new Promise<LeaveChoice>((resolve) => {
        resolveRef.current = resolve
        setOpen(true)
      })
    })
    // Note: there is no unregister API by design — a single host lives for the
    // app session. If this host unmounts on a layout-branch swap, the next
    // mount simply re-registers; in the gap the store defaults to 'save'.
  }, [])

  const handleChoose = useCallback((choice: LeaveChoice) => {
    setOpen(false)
    const resolve = resolveRef.current
    resolveRef.current = null
    resolve?.(choice)
  }, [])

  if (!open) return null
  return <UnsavedChangesModal onChoose={handleChoose} />
}

export default UnsavedChangesModalHost

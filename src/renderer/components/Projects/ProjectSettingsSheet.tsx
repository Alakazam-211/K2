// A project's settings in a sheet over the Projects page (projects reorg
// §3.6, Q1): the page stays behind it, so a person changes a project and
// is back where they were with one ×. The sheet is the 0.46.1 Project
// settings detail (fixed header, two halves) for ONE project: no Settings
// sidebar, no project list, and no "Open project" (it is already open).
// Settings → Projects is unchanged and still works on its own.
//
// Entry points: the nav's right-click "Project settings" and the project
// bar's ⚙ both call `openProjectSettings(groupId, opener)`; on the Projects
// page that opens this sheet (store: `openProjectSettingsSheet`), anywhere
// else it opens Settings → Projects as before. One switch below
// (`PROJECT_SETTINGS_IN_SHEET`) puts every entry point back on Settings.
//
// The frame is the shared Dialog (Tile in Glass, the chamfered solid in
// Terminal, the palette's scrim), centred and as large as the window allows.
// Esc, the × and a press on the scrim close it; focus stays inside it while
// it is open and goes back to what opened it after.

import React, { useCallback, useEffect, useId, useRef } from 'react'
import { createPortal } from 'react-dom'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import { settingMenuOpen } from '@/components/Settings/settings-esc'
import { SettingsLoadState } from '@/components/Settings/controls/SettingControls'
import { Button, DialogCloseButton, DialogFrame, DialogScrim } from '@/components/ui'
import { ProjectSettingsDetail, useProjectSettingsData } from './ProjectSettings'
import { partitionPinned } from './projects-api'

/** false: every "Project settings" entry point opens Settings → Projects
 *  instead of the sheet (the pre-sheet behaviour). */
export const PROJECT_SETTINGS_IN_SHEET = true

/** Open a project's settings: the sheet over the Projects page while that
 *  page is showing, else Settings → Projects on that project. `opener` gets
 *  the focus back when the sheet closes. */
export function openProjectSettings(groupId: string, opener?: HTMLElement | null): void {
  const onPage = usePageViewStore.getState().page === 'projects' && !useSettingsStore.getState().settingsOpen
  if (PROJECT_SETTINGS_IN_SHEET && onPage) {
    useProjectGroupsStore.getState().openProjectSettingsSheet(groupId, opener)
  } else {
    useSettingsStore.getState().openSettings('project-groups', groupId)
  }
}

/** The project to show after `deletedId` is gone: the next one in the
 *  nav's order (Pinned first), else the one before it, else none. */
export function projectAfterDelete<T extends { id: string; pinned: boolean }>(groups: T[], deletedId: string): T | null {
  const { pinned, unpinned } = partitionPinned(groups)
  const order = [...pinned, ...unpinned]
  const at = order.findIndex((g) => g.id === deletedId)
  const rest = order.filter((g) => g.id !== deletedId)
  if (rest.length === 0) return null
  return rest[Math.min(Math.max(at, 0), rest.length - 1)] ?? null
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])'

/** Focus may leave the sheet only for things it opened on top of itself. */
const OWN_LAYERS =
  '[aria-modal="true"], [role="dialog"], [role="menu"], [role="listbox"], [data-setting-dropdown-menu], [data-floating]'

/** Focus in one of these owns Esc first (dialog-cancel's rule). */
const OWNS_ESCAPE =
  '[aria-haspopup][aria-expanded="true"], [role="combobox"][aria-expanded="true"], [role="menu"], [role="listbox"]'

function focusables(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => el.getClientRects().length > 0 && !el.closest('[aria-hidden="true"], [inert]'),
  )
}

function isEditable(el: EventTarget | null): boolean {
  if (!(el instanceof HTMLElement)) return false
  return el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable
}

export default function ProjectSettingsSheet(): React.JSX.Element | null {
  const sheet = useProjectGroupsStore((s) => s.settingsSheet)
  const pageOpen = usePageViewStore((s) => s.page === 'projects')
  const settingsOpen = useSettingsStore((s) => s.settingsOpen)
  const groups = useProjectGroupsStore((s) => s.groups)

  // Leaving the page (another page, Settings opening over it) or the
  // project going away (deleted on another client) closes the sheet.
  const gone = sheet !== null && groups !== null && !groups.some((g) => g.id === sheet.groupId)
  useEffect(() => {
    if (sheet && (!pageOpen || settingsOpen || gone)) useProjectGroupsStore.getState().closeProjectSettingsSheet()
  }, [sheet, pageOpen, settingsOpen, gone])

  if (!sheet || !pageOpen || settingsOpen) return null
  return <SheetFrame key={sheet.groupId} groupId={sheet.groupId} opener={sheet.opener} />
}

function SheetFrame({ groupId, opener }: { groupId: string; opener: HTMLElement | null }): React.JSX.Element {
  const titleId = useId()
  const frameRef = useRef<HTMLDivElement>(null)
  const groupName = useProjectGroupsStore((s) => s.groups?.find((g) => g.id === groupId)?.name ?? null)
  const { role, shown, switching, detailError, docs, docsError, edit, reload, reloadDocs } =
    useProjectSettingsData(groupId)

  const close = useCallback((): void => {
    // A field being typed in saves as it would on leaving it.
    const active = document.activeElement
    if (active instanceof HTMLElement && frameRef.current?.contains(active)) active.blur()
    useProjectGroupsStore.getState().closeProjectSettingsSheet()
  }, [])

  // Focus: into the sheet on open; back to what opened it on close (or,
  // when that is gone — its project was deleted — the page's selected
  // project in the nav).
  useEffect(() => {
    frameRef.current?.focus({ preventScroll: true })
    return () => {
      requestAnimationFrame(() => {
        if (opener && opener.isConnected) {
          opener.focus({ preventScroll: true })
          return
        }
        const sel = useProjectGroupsStore.getState().selectedGroupId
        const row = sel ? document.querySelector<HTMLElement>(`[data-group-id="${sel.replace(/["\\]/g, '\\$&')}"] button`) : null
        row?.focus({ preventScroll: true })
      })
    }
  }, [opener])

  // Keep focus inside: anything focused behind the sheet (a page shortcut,
  // a stray click target) comes back to it. Layers the sheet opened over
  // itself (a confirm, a menu, a drop-down) keep theirs.
  useEffect(() => {
    const onFocusIn = (e: FocusEvent): void => {
      const frame = frameRef.current
      const target = e.target
      if (!frame || !(target instanceof Element)) return
      if (frame.contains(target) || target.closest(OWN_LAYERS)) return
      ;(focusables(frame)[0] ?? frame).focus({ preventScroll: true })
    }
    document.addEventListener('focusin', onFocusIn)
    return () => document.removeEventListener('focusin', onFocusIn)
  }, [])

  // Esc closes the sheet, last: a confirm or crop dialog on top, an open
  // menu or drop-down, and a field's own Esc (a name reverting, a search
  // clearing) go first. Capture for everything but fields, so the page's
  // own Esc (focus the last terminal tile) never runs behind the sheet;
  // fields get the key first and stop it when they used it.
  useEffect(() => {
    // Another dialog than this sheet (whose own aria-modal box wraps the
    // frame's content).
    const otherModal = (): boolean => {
      const frame = frameRef.current
      return Array.from(document.querySelectorAll('[aria-modal="true"]')).some(
        (el) => !frame || (!el.contains(frame) && !frame.contains(el)),
      )
    }
    const blocked = (e: KeyboardEvent): boolean =>
      e.key !== 'Escape' ||
      e.defaultPrevented ||
      e.isComposing ||
      (e.target instanceof Element && e.target.closest(OWNS_ESCAPE) !== null) ||
      settingMenuOpen() ||
      otherModal()
    const onCapture = (e: KeyboardEvent): void => {
      if (blocked(e) || isEditable(e.target)) return
      e.preventDefault()
      e.stopImmediatePropagation()
      close()
    }
    const onBubble = (e: KeyboardEvent): void => {
      if (blocked(e) || !isEditable(e.target)) return
      if (!(e.target instanceof Element) || !frameRef.current?.contains(e.target)) return
      e.preventDefault()
      close()
    }
    window.addEventListener('keydown', onCapture, true)
    window.addEventListener('keydown', onBubble)
    return () => {
      window.removeEventListener('keydown', onCapture, true)
      window.removeEventListener('keydown', onBubble)
    }
  }, [close])

  // Tab and Shift+Tab wrap inside the sheet.
  const onKeyDown = (e: React.KeyboardEvent): void => {
    const frame = frameRef.current
    if (e.key !== 'Tab' || !frame || !(e.target instanceof Node) || !frame.contains(e.target)) return
    const list = focusables(frame)
    if (list.length === 0) {
      e.preventDefault()
      return
    }
    const first = list[0]
    const last = list[list.length - 1]
    const at = document.activeElement
    if (e.shiftKey && (at === first || at === frame)) {
      e.preventDefault()
      last.focus()
    } else if (!e.shiftKey && at === last) {
      e.preventDefault()
      first.focus()
    }
  }

  const onDeleted = (name: string): void => {
    const st = useProjectGroupsStore.getState()
    const list = st.groups ?? []
    const next = projectAfterDelete(list, groupId)
    // Off the list now (the refetch confirms), so nothing re-selects it.
    useProjectGroupsStore.setState({ groups: list.filter((g) => g.id !== groupId) })
    if (st.selectedGroupId === groupId || st.selectedGroupId === null) st.selectGroup(next?.id ?? null)
    st.closeProjectSettingsSheet()
    useToastStore
      .getState()
      .addToast(`Deleted “${name}”. Its agents and their folders are still on this server.`, 'success')
    void st.fetchGroups()
  }

  const closeButton = <DialogCloseButton onClick={close} data-testid="project-settings-sheet-close" />

  /** The title row while the project loads or fails: its name, the × */
  const plainHead = (
    <div className="flex-shrink-0 flex items-center gap-3 px-6 pt-4 pb-4 pr-4 border-b border-[var(--color-border)]">
      <div className="flex-1 min-w-0">
        <h2 id={titleId} className="type-title text-[var(--color-text-primary)] truncate">
          {groupName ?? 'Project settings'}
        </h2>
        {groupName && <p className="type-hint text-[var(--color-text-secondary)] mt-0.5">Project settings</p>}
      </div>
      {closeButton}
    </div>
  )

  return createPortal(
    <>
      <DialogScrim
        data-testid="project-settings-sheet-scrim"
        onPointerDown={(e) => {
          if (e.button === 0) close()
        }}
      />
      <DialogFrame
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-testid="project-settings-sheet"
        className="w-[min(76rem,calc(100vw-2.5rem))] h-[min(56rem,calc(100vh-2.5rem))] overflow-hidden"
      >
        <div ref={frameRef} tabIndex={-1} onKeyDown={onKeyDown} className="h-full flex flex-col min-h-0 outline-none">
          {detailError && detailError.id === groupId ? (
            <>
              {plainHead}
              <div className="flex-1 min-h-0 overflow-y-auto p-6 bg-[var(--color-bg)]">
                <SettingsLoadState
                  kind="error"
                  title="Couldn’t load this project’s settings"
                  detail={detailError.message}
                  testId="project-settings-sheet-error"
                  action={
                    <Button variant="quiet" onClick={reload}>
                      Try again
                    </Button>
                  }
                >
                  Its settings can’t be shown right now. The project itself is fine.
                </SettingsLoadState>
              </div>
            </>
          ) : shown === null || switching ? (
            <>
              {plainHead}
              <div className="flex-1 min-h-0 p-6 bg-[var(--color-bg)]">
                <SettingsLoadState kind="loading" title="Loading the project’s settings…" testId="project-settings-sheet-loading" />
              </div>
            </>
          ) : (
            <ProjectSettingsDetail
              detail={shown}
              role={role}
              edit={(patch, run) => edit(shown.id, patch, run)}
              docs={docs}
              docsError={docsError}
              onReloadDocs={reloadDocs}
              notice={null}
              onDeleted={onDeleted}
              variant="sheet"
              closeButton={closeButton}
              titleId={titleId}
            />
          )}
        </div>
      </DialogFrame>
    </>,
    document.body,
  )
}

// Single owner of external OS → window drag-drop routing.
//
// Tauri fires one window-level `tauri://drag-drop` event per drop. Historically
// FileTree, every TerminalPane / AlacrittyTerminalView, and App's miss handler
// each subscribed independently. Async listen() teardown races + unstable
// effect deps (FileTree's loadDir) leaked handlers → N uploads of the same
// file (collision_free_path → report.pdf, report (1).pdf, …).
//
// This module owns the ONE subscriber. Hit-test order matches product rules:
//   1. Terminal  → remote upload + inject path; local path paste
//   2. Files     → remote folder upload; local fs/copy plan
//   3. Remote miss → "Save to…" picker
//
// Terminal inject reuses existing seams:
//   - v2 (`data-terminal-kind="v2"`): `k2so:terminal-write` CustomEvent on the
//     container (TerminalPane already listens)
//   - v1: `terminalWrite(id, payload)`
//
// FileTree refresh after a folder drop is a CustomEvent on the panel so the
// tree does not need a second drag-drop listener.

import { executeRemoteDrop } from './handle-remote-drop'
import { planLocalExternalDrop } from './external-drop'
import { daemonCliPost } from './daemon-cli'
import { terminalWrite } from './terminal-daemon'
import {
  isImagePath,
  quotePathForImageDrop,
  bracketPaste,
} from './file-drag'
import { useConnectHostStore } from '@/stores/connect-host'
import { useToastStore } from '@/stores/toast'
import { useFileUndoStore } from '@/stores/file-undo'

// ── Public event names ────────────────────────────────────────────────

/** Dispatched on `[data-file-tree-panel]` after a successful external folder drop. */
export const FILE_TREE_EXTERNAL_DROP_EVENT = 'k2so:file-tree-external-drop'

// ── Pure hit-test / classify helpers (unit-tested) ────────────────────

export type ExternalDropTarget =
  | {
      kind: 'terminal'
      terminalId: string | undefined
      terminalKind: string | undefined
      workspacePath: string
      /** Container element that accepts inject (v2 CustomEvent target). */
      element: HTMLElement
    }
  | { kind: 'folder'; path: string }
  | { kind: 'miss' }

/** Parent directory of a POSIX-style path. */
export function parentDir(path: string): string {
  const idx = path.lastIndexOf('/')
  return idx > 0 ? path.slice(0, idx) : '/'
}

/**
 * Resolve the file-tree destination folder from panel + optional row hit.
 * Pure: no DOM. Returns null when the drop is outside the panel.
 */
export function resolveFileTreeFolder(input: {
  inPanel: boolean
  rootPath: string
  /** `data-path` of the row under the cursor, if any. */
  rowPath: string | null
  /** Whether that row is a directory (`data-is-directory === 'true'`). */
  rowIsDirectory: boolean
}): string | null {
  if (!input.inPanel) return null
  if (input.rowPath) {
    return input.rowIsDirectory ? input.rowPath : parentDir(input.rowPath)
  }
  return input.rootPath
}

/**
 * Pure classify: terminal wins over files panel over miss.
 * DOM probing is done by the caller / `hitTestExternalDrop`.
 */
export function classifyExternalDrop(input: {
  terminal: {
    terminalId: string | undefined
    terminalKind: string | undefined
    workspacePath: string
    element: HTMLElement
  } | null
  fileTreeFolder: string | null
}): ExternalDropTarget {
  if (input.terminal) {
    return {
      kind: 'terminal',
      terminalId: input.terminal.terminalId,
      terminalKind: input.terminal.terminalKind,
      workspacePath: input.terminal.workspacePath,
      element: input.terminal.element,
    }
  }
  if (input.fileTreeFolder) {
    return { kind: 'folder', path: input.fileTreeFolder }
  }
  return { kind: 'miss' }
}

/**
 * DOM hit-test at a drop position. Uses the same selectors the old
 * multi-handler paths used (`[data-terminal-id|container]`,
 * `[data-file-tree-panel]`, `[data-path]`).
 */
export function hitTestExternalDrop(
  position: { x: number; y: number },
  doc: Document = document,
): ExternalDropTarget {
  const el = doc.elementFromPoint(position.x, position.y) as HTMLElement | null

  // 1. Terminal — prefer the id-bearing container; fall back to the focus
  //    container so a pre-session pane still claims the drop over miss.
  const termEl = (el?.closest?.('[data-terminal-id]') ??
    el?.closest?.('[data-terminal-container]')) as HTMLElement | null
  if (termEl) {
    return classifyExternalDrop({
      terminal: {
        terminalId: termEl.dataset.terminalId,
        terminalKind: termEl.dataset.terminalKind,
        workspacePath: termEl.dataset.workspacePath ?? '',
        element: termEl,
      },
      fileTreeFolder: null,
    })
  }

  // 2. Files panel — rect check so empty padding still counts as in-tree.
  const panel = doc.querySelector(
    '[data-file-tree-panel]',
  ) as HTMLElement | null
  let fileTreeFolder: string | null = null
  if (panel) {
    const rect = panel.getBoundingClientRect()
    const inPanel =
      position.x >= rect.left &&
      position.x <= rect.right &&
      position.y >= rect.top &&
      position.y <= rect.bottom
    if (inPanel) {
      const rootPath = panel.dataset.rootPath ?? ''
      const pathEl = el?.closest?.('[data-path]') as HTMLElement | null
      fileTreeFolder = resolveFileTreeFolder({
        inPanel: true,
        rootPath,
        rowPath: pathEl?.dataset.path ?? null,
        rowIsDirectory: pathEl?.dataset.isDirectory === 'true',
      })
    }
  }

  return classifyExternalDrop({ terminal: null, fileTreeFolder })
}

// ── Terminal payload builder (matches TerminalPane / Alacritty) ───────

/** Backslash-escape specials the same way v1/v2 terminal drop did. */
function shellEscape(path: string): string {
  return path.replace(/[ '"\\()&|;<>$`!#*?[\]{}~]/g, '\\$&')
}

function formatPathForTerminal(path: string): string {
  return isImagePath(path) ? quotePathForImageDrop(path) : shellEscape(path)
}

/**
 * Build the inject payload for dropped paths. Images get bracketed paste
 * so Claude Code's paste-event image detector fires.
 */
export function buildTerminalDropPayload(paths: string[]): string {
  const formatted = paths.map(formatPathForTerminal).join(' ')
  const trailing = formatted + ' '
  return paths.some(isImagePath) ? bracketPaste(trailing) : trailing
}

// ── Inject / refresh side-effects ─────────────────────────────────────

function injectIntoTerminal(
  target: Extract<ExternalDropTarget, { kind: 'terminal' }>,
  payload: string,
): void {
  // v2 sessions own their WS; file-drag / TerminalPane use this event.
  if (target.terminalKind === 'v2') {
    target.element.dispatchEvent(
      new CustomEvent('k2so:terminal-write', { detail: { data: payload } }),
    )
    return
  }
  // v1 legacy TerminalManager path.
  if (target.terminalId) {
    terminalWrite(target.terminalId, payload).catch((e) =>
      console.warn('[external-drop-router] terminalWrite failed', e),
    )
    return
  }
  // Last resort: still try the CustomEvent in case a v2 pane hasn't set
  // data-terminal-kind yet but has the write listener.
  target.element.dispatchEvent(
    new CustomEvent('k2so:terminal-write', { detail: { data: payload } }),
  )
}

function notifyFileTreeRefresh(folderPath: string, doc: Document = document): void {
  const panel = doc.querySelector('[data-file-tree-panel]')
  if (!panel) return
  panel.dispatchEvent(
    new CustomEvent(FILE_TREE_EXTERNAL_DROP_EVENT, {
      detail: { path: folderPath },
    }),
  )
}

// ── Route + execute (one upload site) ─────────────────────────────────

/**
 * Handle a single external drop end-to-end. Safe to call from the router
 * listener; never re-enters another tauri://drag-drop handler.
 */
export async function routeExternalDrop(
  paths: string[],
  position: { x: number; y: number },
  doc: Document = document,
): Promise<void> {
  if (!paths || paths.length === 0) return

  const target = hitTestExternalDrop(position, doc)
  const isRemote = useConnectHostStore.getState().activeHost !== 'local'

  switch (target.kind) {
    case 'terminal': {
      if (isRemote) {
        const payload = await executeRemoteDrop(
          paths,
          { kind: 'terminal' },
          { workspacePath: target.workspacePath || undefined },
          buildTerminalDropPayload,
        )
        if (payload) injectIntoTerminal(target, payload)
      } else {
        injectIntoTerminal(target, buildTerminalDropPayload(paths))
      }
      return
    }
    case 'folder': {
      if (isRemote) {
        await executeRemoteDrop(paths, { kind: 'folder', path: target.path }, {})
        notifyFileTreeRefresh(target.path, doc)
        return
      }
      // LOCAL: always COPY (never move) — see external-drop.ts invariant.
      const toast = useToastStore.getState()
      const undo = useFileUndoStore.getState()
      const plan = planLocalExternalDrop(paths, target.path)
      try {
        await daemonCliPost(plan.endpoint, plan.payload)
        undo.push(plan.undo)
        toast.addToast(plan.toast, 'success')
        notifyFileTreeRefresh(target.path, doc)
      } catch (err) {
        toast.addToast(`Failed: ${err}`, 'error')
      }
      return
    }
    case 'miss': {
      // Local window-chrome drops have no product meaning.
      if (!isRemote) return
      await executeRemoteDrop(paths, { kind: 'miss' }, {})
      return
    }
  }
}

// ── Single-subscriber mount ───────────────────────────────────────────

/**
 * Mount the ONE window-level `tauri://drag-drop` listener for external OS
 * drops. Call once from App. Returns a teardown that unsubscribes even if
 * `listen()` is still pending (Strict Mode / remount leak guard — same
 * pattern as the former App miss handler's `ddTorndown`).
 */
export function mountExternalDropRouter(): () => void {
  const unlisteners: Array<() => void> = []
  let torndown = false
  const track = (fn: () => void) => {
    if (torndown) fn()
    else unlisteners.push(fn)
  }

  import('@tauri-apps/api/event')
    .then(({ listen }) => {
      if (torndown) return
      return listen<{ paths: string[]; position: { x: number; y: number } }>(
        'tauri://drag-drop',
        (event) => {
          const { paths, position } = event.payload
          if (!paths || paths.length === 0 || !position) return
          void routeExternalDrop(paths, position)
        },
      ).then(track)
    })
    .catch((err) => {
      // Hosted web / non-Tauri: no native OS path drops — ignore.
      if (import.meta.env.DEV) {
        console.debug('[external-drop-router] listen unavailable', err)
      }
    })

  return () => {
    torndown = true
    unlisteners.forEach((fn) => fn())
  }
}

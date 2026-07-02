// Glue between the workspace context-menu "Clone to ▸ <host>" action and
// the orchestration (`cloneWorkspaceTo`) + the progress modal store.
//
// The context-menu handlers (IconRail / Sidebar) call `startCloneTo` with
// the source workspace + the chosen destination host. This opens the modal
// at its pre-flight OPTIONS phase (the "Include secrets" toggle). Only when
// the user clicks Clone does the store's `confirm()` invoke our `onConfirm`
// runner — receiving the chosen `carrySecrets` value — which then drives the
// orchestration, piping its hooks into the modal store. Errors are swallowed
// here (the modal already surfaces them via setError) so the menu handler
// doesn't need its own try/catch.

import {
  cloneWorkspaceTo,
  defaultCloneDeps,
  CloneCancelledError,
  type CloneDeps,
  type CloneStage,
} from './clone-to'
import {
  cloneWorkspaceToThisComputer,
  defaultClonePullDeps,
  type ClonePullDeps,
} from './clone-pull'
import { useCloneToDialogStore } from '@/stores/clone-to-dialog'
import { useProjectsStore } from '@/stores/projects'
import { useToastStore } from '@/stores/toast'
import type { ConnectHost } from '@/stores/connect-host'

/**
 * Open the "Clone to" modal at its pre-flight options phase. The actual run
 * (`cloneWorkspaceTo`) is deferred until the user clicks Clone, at which
 * point the store invokes `onConfirm(carrySecrets)` with the chosen toggle
 * value — wiring its hooks into the modal store. `deps` is injectable for
 * tests; production passes the default bag.
 */
export function startCloneTo(
  projectPath: string,
  projectName: string,
  host: ConnectHost,
  deps: CloneDeps = defaultCloneDeps(),
): void {
  const store = useCloneToDialogStore.getState()
  store.start({
    projectPath,
    projectName,
    host,
    onConfirm: (carrySecrets, includeAllHistory) => {
      void runClone(projectPath, host, deps, carrySecrets, includeAllHistory)
    },
  })
}

/**
 * Open the "Clone to" modal for a PULL run — "Clone to this computer"
 * (0.40.22). The source workspace lives on the ACTIVE REMOTE host; the
 * renderer orchestrates pack-on-remote → download → unpack-on-local (see
 * clone-pull.ts). Same deferred-run shape as {@link startCloneTo}: the
 * options panel shows first, Clone starts the run.
 *
 * PRECONDITION (enforced by the menu entry points): the active host is
 * remote — that's the only context where the entry is offered.
 */
export function startCloneToThisComputer(
  projectPath: string,
  projectName: string,
  deps: ClonePullDeps = defaultClonePullDeps(),
): void {
  const store = useCloneToDialogStore.getState()
  store.start({
    projectPath,
    projectName,
    host: null,
    pull: true,
    onConfirm: (carrySecrets, includeAllHistory) => {
      void runPull(projectPath, projectName, deps, carrySecrets, includeAllHistory)
    },
  })
}

/** Human-readable name of a pull stage, for the fail-loud toast. */
const PULL_STAGE_LABELS: Partial<Record<CloneStage, string>> = {
  packing: 'packing on the server',
  'choosing-folder': 'choosing the destination folder',
  downloading: 'downloading the bundle',
  unpacking: 'unpacking on this computer',
}

/** Drive the PULL orchestration once the user has confirmed the options
 *  panel. Toasts the outcome (success with the local workspace name;
 *  failure names the stage that failed) on top of the modal's own
 *  done/error screens — the transfer overlay card is only visible during
 *  the download, so the toast is the durable signal. */
async function runPull(
  projectPath: string,
  projectName: string,
  deps: ClonePullDeps,
  carrySecrets: boolean,
  includeAllHistory: boolean,
): Promise<void> {
  let lastStage: CloneStage = 'packing'
  try {
    const result = await cloneWorkspaceToThisComputer(
      projectPath,
      projectName,
      deps,
      {
        onStage: (stage) => {
          if (stage !== 'error') lastStage = stage
          useCloneToDialogStore.getState().setStage(stage)
        },
        onBundled: (summary) => useCloneToDialogStore.getState().setSummary(summary),
        onDone: (r) => useCloneToDialogStore.getState().setDone(r),
        onError: (message) => useCloneToDialogStore.getState().setError(message),
      },
      carrySecrets,
      includeAllHistory,
    )
    const localName = result.project?.name ?? projectName
    useToastStore.getState().addToast(
      `Cloned “${localName}” to this computer — it's in your workspace list when you switch to This Computer.`,
      'success',
      6000,
    )
  } catch (err) {
    if (err instanceof CloneCancelledError) {
      // User-driven abort: the modal already shows the message; close it
      // and confirm the abort quietly.
      useCloneToDialogStore.getState().close()
      useToastStore.getState().addToast('Clone cancelled', 'info', 3000)
      return
    }
    // The modal shows the message via onError → setError; the toast names
    // the STAGE for fail-loud visibility even if the modal was dismissed.
    const message = err instanceof Error ? err.message : String(err)
    useToastStore.getState().addToast(
      `Clone to this computer failed while ${PULL_STAGE_LABELS[lastStage] ?? lastStage}: ${message}`,
      'error',
    )
  }
}

/** Drive the orchestration once the user has confirmed the options panel. */
async function runClone(
  projectPath: string,
  host: ConnectHost,
  deps: CloneDeps,
  carrySecrets: boolean,
  includeAllHistory: boolean,
): Promise<void> {
  try {
    await cloneWorkspaceTo(
      projectPath,
      host,
      deps,
      {
        onStage: (stage) => useCloneToDialogStore.getState().setStage(stage),
        onBundled: (summary) => useCloneToDialogStore.getState().setSummary(summary),
        onDone: (result) => useCloneToDialogStore.getState().setDone(result),
        onError: (message) => useCloneToDialogStore.getState().setError(message),
      },
      carrySecrets,
      includeAllHistory,
    )
    // The clone unpacked + registered the workspace on the remote daemon, but
    // the renderer's project list (already pointed at that host) won't reflect
    // it until something re-fetches — otherwise the cloned workspace stays
    // invisible until a manual window reload (#18). The active host is the
    // destination here, so this lists the freshly-cloned workspace. Best-
    // effort: the clone already succeeded, so a refresh hiccup must NOT be
    // surfaced as a clone failure.
    try {
      await useProjectsStore.getState().fetchProjects()
    } catch (e) {
      console.warn('[clone-to] post-clone project refresh failed:', e)
    }
  } catch {
    // The modal already reflects the failure via onError → setError; the
    // user closes it manually. Nothing more to do here.
  }
}

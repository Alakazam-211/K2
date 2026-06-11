import { create } from 'zustand'

import type {
  CloneStage,
  CloneManifestSummary,
  CloneUnpackResult,
} from '@/lib/clone-to'
import type { ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'

// Store for the "Clone to" modal. Mirrors the open/close idiom of
// add-workspace-dialog / remote-folder-picker. The modal subscribes to this
// and renders the current phase/stage; the orchestration (`cloneWorkspaceTo`)
// pushes progress updates here via its hooks.
//
// Lifecycle:
//   1. The context-menu handler calls `start({ projectPath, host, onConfirm })`.
//      The modal opens in the 'options' PHASE — a pre-flight panel with the
//      "Include secrets" toggle (default INCLUDE) + a Clone button.
//   2. The user picks the toggle and clicks Clone → `confirm()` flips the
//      phase to 'running' and invokes the stored `onConfirm(carrySecrets)`,
//      which drives `cloneWorkspaceTo` whose hooks call setStage / setSummary
//      / setDone / setError on this store.
//   3. The modal stays open through 'done' / 'error' so the user sees the
//      result + the re-supply checklist; they close it manually.

/** Two-phase modal: the pre-flight options panel, then the progress run. */
export type ClonePhase = 'options' | 'running'

interface CloneToDialogState {
  isOpen: boolean
  /** Pre-flight options vs. the running progress UI. */
  phase: ClonePhase
  /** Source workspace path (LOCAL). */
  projectPath: string | null
  /** Source workspace display name (for the header). */
  projectName: string | null
  /** Destination K2 Connect host. */
  host: ConnectHost | null
  /** "Include secrets" toggle — default true (include). Read at confirm time
   *  and threaded into `cloneWorkspaceTo` → `clone/bundle` as carry_secrets. */
  carrySecrets: boolean
  /** "Include all chat history" toggle — default true (include). Read at
   *  confirm time and threaded into `cloneWorkspaceTo` → `clone/bundle` as
   *  `live_only: !includeAllHistory`. Lets advanced users slim the bundle
   *  down to just the live session (GitHub #21 — all history is the default). */
  includeAllHistory: boolean
  stage: CloneStage
  /** Manifest summary, set once the bundle is built. */
  summary: CloneManifestSummary | null
  /** Unpack result, set on success. */
  result: CloneUnpackResult | null
  /** Error message, set on failure. */
  error: string | null
  /** Runner that starts the orchestration; set by `start`, invoked by
   *  `confirm` with the chosen toggle value. Held so the dialog's Clone
   *  button can kick off the run without re-plumbing deps through the UI. */
  onConfirm: ((carrySecrets: boolean, includeAllHistory: boolean) => void) | null

  /** Open the modal at the 'options' phase for a given source + host. */
  start: (args: {
    projectPath: string
    projectName: string
    host: ConnectHost
    onConfirm: (carrySecrets: boolean, includeAllHistory: boolean) => void
  }) => void
  /** Toggle the "Include secrets" checkbox (options phase only). */
  setCarrySecrets: (carrySecrets: boolean) => void
  /** Toggle the "Include all chat history" checkbox (options phase only). */
  setIncludeAllHistory: (includeAllHistory: boolean) => void
  /** Proceed from the options panel: flip to 'running' and start the run. */
  confirm: () => void
  setStage: (stage: CloneStage) => void
  setSummary: (summary: CloneManifestSummary) => void
  setDone: (result: CloneUnpackResult) => void
  setError: (error: string) => void
  /** Open the freshly-cloned workspace on the (already-active) destination
   *  host, then dismiss the modal. The active host is still the destination
   *  at this point, so selecting the project opens it there. Ensures the
   *  project is loaded first — the post-clone refresh in `runClone` usually
   *  already listed it, but if that's still in flight we fetch before
   *  selecting so `setActiveProject` can resolve the id. No-op if the result
   *  has no project id. */
  openResult: () => Promise<void>
  /** Dismiss + reset. */
  close: () => void
}

const RESET = {
  isOpen: false,
  phase: 'options' as ClonePhase,
  projectPath: null,
  projectName: null,
  host: null,
  carrySecrets: true,
  includeAllHistory: true,
  stage: 'bundling' as CloneStage,
  summary: null,
  result: null,
  error: null,
  onConfirm: null,
}

export const useCloneToDialogStore = create<CloneToDialogState>((set, get) => ({
  ...RESET,

  start: ({ projectPath, projectName, host, onConfirm }) =>
    set({
      ...RESET,
      isOpen: true,
      phase: 'options',
      projectPath,
      projectName,
      host,
      carrySecrets: true,
      includeAllHistory: true,
      onConfirm,
    }),

  setCarrySecrets: (carrySecrets) => set({ carrySecrets }),
  setIncludeAllHistory: (includeAllHistory) => set({ includeAllHistory }),

  confirm: () => {
    const { phase, onConfirm, carrySecrets, includeAllHistory } = get()
    if (phase !== 'options') return
    set({ phase: 'running', stage: 'bundling' })
    onConfirm?.(carrySecrets, includeAllHistory)
  },

  setStage: (stage) => set({ stage }),
  setSummary: (summary) => set({ summary }),
  setDone: (result) => set({ stage: 'done', result }),
  setError: (error) => set({ stage: 'error', error }),

  openResult: async () => {
    const id = get().result?.project?.id
    if (id) {
      const projects = useProjectsStore.getState()
      // Usually already present (runClone refreshed on success); fetch only
      // if the refresh hasn't landed yet so setActiveProject can resolve it.
      if (!projects.projects.some((p) => p.id === id)) {
        try {
          await useProjectsStore.getState().fetchProjects()
        } catch (e) {
          console.warn('[clone-to] refresh before open failed:', e)
        }
      }
      useProjectsStore.getState().setActiveProject(id)
    }
    set({ ...RESET })
  },

  close: () => set({ ...RESET }),
}))

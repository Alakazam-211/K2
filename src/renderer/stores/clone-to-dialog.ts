import { create } from 'zustand'

import type {
  CloneStage,
  CloneManifestSummary,
  CloneUnpackResult,
} from '@/lib/clone-to'
import type { ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import {
  anyCopy,
  buildCloneExtras,
  CloneConfirmCancelled,
  initialSecretChoices,
  type AskPassword,
  type CloneExtras,
  type ClonePreview,
  type SecretChoice,
} from '@/lib/clone-options'

/** Source-side collaborators for the options phase (0.46.0): the preview
 *  and the two person confirms. Injected by `start` (start-clone-to.ts
 *  binds the source scope) so the store is testable with mocks. */
export interface CloneOptionDeps {
  fetchPreview: () => Promise<ClonePreview>
  /** `clone-env` confirm for sending `.env` files as files (D4). */
  envConfirm: (askPassword: AskPassword) => Promise<string>
  /** Vault slice's plan-bound `clone-carry` confirm for Copy rows. */
  carryConfirm: (secrets: Record<string, SecretChoice>, askPassword: AskPassword) => Promise<string>
}

/** The password field the dialog shows while a confirm waits on it. */
export interface PasswordPrompt {
  method: string
  label: string
}

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
  /** Destination K2 Connect host. Null for a PULL run (the destination is
   *  this computer — no ConnectHost models it). */
  host: ConnectHost | null
  /** PULL run ("Clone to this computer", 0.40.22): the source workspace
   *  lives on the ACTIVE REMOTE host and the destination is the local
   *  machine. Flips the modal's step list + header direction; the push
   *  flow leaves it false. */
  pull: boolean
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
  onConfirm: ((carrySecrets: boolean, includeAllHistory: boolean, extras?: CloneExtras) => void) | null
  /** Names-only plan from `clone/preview` (null until loaded / on error). */
  preview: ClonePreview | null
  previewLoading: boolean
  /** Why the preview failed (a gate refusal reads verbatim). */
  previewError: string | null
  /** "Bring the Thread" (D3: ticked by default, always offered). */
  bringThread: boolean
  /** Only the last N days of Thread (null = all). */
  sinceDays: number | null
  /** Per-secret Copy / Leave blank / Skip, by name. */
  secretChoices: Record<string, SecretChoice>
  /** A confirm is running (the Clone button waits). */
  confirming: boolean
  /** Error from the confirm step; the dialog stays on options. */
  optionsError: string | null
  passwordPrompt: PasswordPrompt | null
  /** Agent proposal being run, if any (passed through as proposal_id). */
  proposalId: string | null
  optionDeps: CloneOptionDeps | null

  /** Open the modal at the 'options' phase for a given source + host.
   *  `host: null` + `pull: true` is the "Clone to this computer" run. */
  start: (args: {
    projectPath: string
    projectName: string
    host: ConnectHost | null
    pull?: boolean
    onConfirm: (carrySecrets: boolean, includeAllHistory: boolean, extras?: CloneExtras) => void
    optionDeps?: CloneOptionDeps
    proposalId?: string | null
  }) => void
  setBringThread: (bring: boolean) => void
  setSinceDays: (days: number | null) => void
  setSecretChoice: (name: string, choice: SecretChoice) => void
  /** The dialog's password field answers the pending confirm (null = cancel). */
  answerPassword: (password: string | null) => void
  /** Toggle the "Include secrets" checkbox (options phase only). */
  setCarrySecrets: (carrySecrets: boolean) => void
  /** Toggle the "Include all chat history" checkbox (options phase only). */
  setIncludeAllHistory: (includeAllHistory: boolean) => void
  /** Proceed from the options panel: run the person confirms the choices
   *  need (`.env` files, Copy rows), then flip to 'running' and start. */
  confirm: () => Promise<void>
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
  pull: false,
  // D4 (0.46.0): sending `.env` files as files needs a person confirm, so
  // it is an explicit choice now, not the default.
  carrySecrets: false,
  includeAllHistory: true,
  stage: 'bundling' as CloneStage,
  summary: null,
  result: null,
  error: null,
  onConfirm: null,
  preview: null,
  previewLoading: false,
  previewError: null,
  bringThread: true,
  sinceDays: null as number | null,
  secretChoices: {} as Record<string, SecretChoice>,
  confirming: false,
  optionsError: null,
  passwordPrompt: null,
  proposalId: null,
  optionDeps: null,
}

// The resolver for the pending password prompt (module-level: functions
// don't belong in the serializable store state).
let pendingPassword: ((pw: string | null) => void) | null = null

export const useCloneToDialogStore = create<CloneToDialogState>((set, get) => ({
  ...RESET,

  start: ({ projectPath, projectName, host, pull, onConfirm, optionDeps, proposalId }) => {
    pendingPassword = null
    set({
      ...RESET,
      isOpen: true,
      phase: 'options',
      projectPath,
      projectName,
      host,
      pull: pull ?? false,
      onConfirm,
      optionDeps: optionDeps ?? null,
      proposalId: proposalId ?? null,
      previewLoading: !!optionDeps,
    })
    if (!optionDeps) return
    void optionDeps.fetchPreview().then(
      (preview) => {
        // A newer start (or close) replaced this run: drop the answer.
        if (get().optionDeps !== optionDeps) return
        set({ preview, previewLoading: false, secretChoices: initialSecretChoices(preview) })
      },
      (e: unknown) => {
        if (get().optionDeps !== optionDeps) return
        set({ previewLoading: false, previewError: e instanceof Error ? e.message : String(e) })
      },
    )
  },

  setCarrySecrets: (carrySecrets) => set({ carrySecrets, optionsError: null }),
  setIncludeAllHistory: (includeAllHistory) => set({ includeAllHistory }),
  setBringThread: (bringThread) => set({ bringThread }),
  setSinceDays: (sinceDays) => set({ sinceDays }),
  setSecretChoice: (name, choice) => {
    const { preview, secretChoices } = get()
    const row = preview?.secrets.rows.find((r) => r.name === name)
    // Copy only where the daemon allows it (D1 Member rule, copyable).
    if (choice === 'copy' && !(row && row.copyable && preview?.callerCanCopySecrets)) return
    set({ secretChoices: { ...secretChoices, [name]: choice }, optionsError: null })
  },
  answerPassword: (password) => {
    const resolve = pendingPassword
    pendingPassword = null
    set({ passwordPrompt: null })
    resolve?.(password)
  },

  confirm: async () => {
    const st = get()
    if (st.phase !== 'options' || st.confirming) return
    const extras = buildCloneExtras({
      bringThread: st.bringThread,
      sinceDays: st.sinceDays,
      secretChoices: st.secretChoices,
      proposalId: st.proposalId,
    })
    const ask: AskPassword = (method, label) =>
      new Promise<string | null>((resolve) => {
        pendingPassword = resolve
        set({ passwordPrompt: { method, label } })
      })
    const needsEnv = st.carrySecrets
    const needsCarry = anyCopy(st.secretChoices)
    if (needsEnv || needsCarry) {
      if (!st.optionDeps) {
        set({ optionsError: 'This clone needs a person confirm, and none is available here.' })
        return
      }
      set({ confirming: true, optionsError: null })
      try {
        if (needsEnv) extras.env_confirm_token = await st.optionDeps.envConfirm(ask)
        if (needsCarry) extras.confirm_token = await st.optionDeps.carryConfirm(st.secretChoices, ask)
      } catch (e) {
        set({
          confirming: false,
          passwordPrompt: null,
          optionsError: e instanceof CloneConfirmCancelled ? null : e instanceof Error ? e.message : String(e),
        })
        return
      }
      // Closed while the confirm ran: don't start.
      if (!get().isOpen) return
    }
    // A pull opens on ITS first stage (packing on the server); the
    // orchestration's first onStage overwrites either way.
    set({ phase: 'running', confirming: false, stage: st.pull ? 'packing' : 'bundling' })
    st.onConfirm?.(st.carrySecrets, st.includeAllHistory, extras)
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

  close: () => {
    const resolve = pendingPassword
    pendingPassword = null
    set({ ...RESET })
    resolve?.(null)
  },
}))

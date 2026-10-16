import { create } from 'zustand'

import type {
  CloneStage,
  CloneManifestSummary,
  CloneUnpackResult,
} from '@/lib/clone-to'
import type { ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import {
  buildCarryAsk,
  buildCloneExtras,
  CloneConfirmCancelled,
  initialSecretChoices,
  MEMBER_NO_COPY_LINE,
  needsCarryConfirm,
  type AskPassword,
  type CloneCarryAsk,
  type CloneExtras,
  type ClonePreview,
  type SecretChoice,
} from '@/lib/clone-options'
import type { CloneVaultOptions, CloneVaultOutcome, CloneVaultTarget, VaultRecipientLookup } from '@/lib/clone-vault'

/** Collaborators for the options phase (0.46.0), injected by `start`
 *  (start-clone-to.ts binds the source scope and the destination) so the
 *  store is testable with mocks. */
export interface CloneOptionDeps {
  /** The destination vault (push: the destination's `vault/recipient` over
   *  its saved login; pull: this computer's). Never throws: an older or
   *  closed vault answers a note and the clone goes on without secrets. */
  lookupTarget: () => Promise<VaultRecipientLookup>
  /** `clone/preview` on the SOURCE for this ask (slice A's plan inside). */
  fetchPreview: (ask: CloneCarryAsk, sinceDays: number | null) => Promise<ClonePreview>
  /** Slice A's `clone-carry` confirm for the FINAL ask (Copy rows and/or
   *  `.env` files), on the source. */
  carryConfirm: (ask: CloneCarryAsk, methods: string[], askPassword: AskPassword) => Promise<string>
}

/** What the run needs for the vault half (slice A's lib fields). */
export interface CloneVaultRun {
  carryVault: boolean
  vaultOptions: CloneVaultOptions
}

/** A fresh clone id (the manifest's; the confirm is bound to it). */
export function newCloneId(): string {
  const c = (globalThis as { crypto?: { randomUUID?: () => string } }).crypto
  if (c?.randomUUID) return c.randomUUID()
  return `clone-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`
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
  onConfirm: ((carrySecrets: boolean, includeAllHistory: boolean, extras?: CloneExtras, vault?: CloneVaultRun) => void) | null
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
  /** This clone's id (minted when the dialog opens). */
  cloneId: string | null
  /** The destination vault, or the note saying why secrets can't travel. */
  vaultLookup: VaultRecipientLookup | null
  /** "This is my own server/computer" (keeps "given by a person" marks). */
  ownBoxMove: boolean
  /** Names-only vault result for the done screen. */
  vaultOutcome: CloneVaultOutcome | null

  /** Open the modal at the 'options' phase for a given source + host.
   *  `host: null` + `pull: true` is the "Clone to this computer" run. */
  start: (args: {
    projectPath: string
    projectName: string
    host: ConnectHost | null
    pull?: boolean
    onConfirm: (carrySecrets: boolean, includeAllHistory: boolean, extras?: CloneExtras, vault?: CloneVaultRun) => void
    optionDeps?: CloneOptionDeps
    proposalId?: string | null
  }) => void
  setBringThread: (bring: boolean) => void
  setSinceDays: (days: number | null) => void
  setSecretChoice: (name: string, choice: SecretChoice) => void
  setOwnBoxMove: (own: boolean) => void
  setVaultOutcome: (outcome: CloneVaultOutcome) => void
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
  cloneId: null as string | null,
  vaultLookup: null as VaultRecipientLookup | null,
  ownBoxMove: false,
  vaultOutcome: null as CloneVaultOutcome | null,
}

/** The destination vault, when the lookup found one. */
export function targetOf(lookup: VaultRecipientLookup | null): CloneVaultTarget | null {
  return lookup && 'target' in lookup ? lookup.target : null
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
    const cloneId = newCloneId()
    set({ cloneId })
    // A newer start (or close) replaced this run: drop the answers.
    const stale = (): boolean => get().optionDeps !== optionDeps
    void (async () => {
      // The destination vault first: the plan's choices (and the confirm)
      // are for that vault. A note means no secrets this time.
      const vaultLookup = await optionDeps.lookupTarget()
      if (stale()) return
      set({ vaultLookup })
      const target = targetOf(vaultLookup)
      const preview = await optionDeps.fetchPreview(
        { clone_id: cloneId, ...(target ? { vault_target: target } : {}) },
        null,
      )
      if (stale()) return
      set({ preview, previewLoading: false, secretChoices: target ? initialSecretChoices(preview) : {} })
    })().catch((e: unknown) => {
      if (stale()) return
      set({ previewLoading: false, previewError: e instanceof Error ? e.message : String(e) })
    })
  },

  setCarrySecrets: (carrySecrets) => set({ carrySecrets, optionsError: null }),
  setIncludeAllHistory: (includeAllHistory) => set({ includeAllHistory }),
  setBringThread: (bringThread) => set({ bringThread }),
  setOwnBoxMove: (ownBoxMove) => set({ ownBoxMove }),
  setVaultOutcome: (vaultOutcome) => set({ vaultOutcome }),
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
    const target = targetOf(st.vaultLookup)
    const cloneId = st.cloneId ?? newCloneId()
    const extras = buildCloneExtras({
      bringThread: st.bringThread,
      sinceDays: st.sinceDays,
      cloneId,
      proposalId: st.proposalId,
    })
    // Secrets travel only to a destination vault; without one every row
    // stays behind (the Secrets block isn't shown).
    const secretChoices = target ? st.secretChoices : {}
    const ask = buildCarryAsk({
      target,
      cloneId,
      secretChoices,
      ownBoxMove: st.ownBoxMove,
      sendEnvFiles: st.carrySecrets,
    })
    const askPassword: AskPassword = (method, label) =>
      new Promise<string | null>((resolve) => {
        pendingPassword = resolve
        set({ passwordPrompt: { method, label } })
      })
    let confirmToken: string | undefined
    if (needsCarryConfirm(secretChoices, st.carrySecrets)) {
      if (!st.optionDeps) {
        set({ optionsError: 'This clone needs a person confirm, and none is available here.' })
        return
      }
      set({ confirming: true, optionsError: null })
      try {
        // The plan for the FINAL choices: its confirm binding (methods,
        // who may confirm) is what the token is minted for.
        const plan = await st.optionDeps.fetchPreview(ask, st.sinceDays)
        const binding = plan.secrets.confirm
        if (!binding) throw new Error("The source didn't offer a confirm for this clone (is its vault open?).")
        if (!binding.allowed) throw new Error(MEMBER_NO_COPY_LINE)
        confirmToken = await st.optionDeps.carryConfirm(ask, binding.methods, askPassword)
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
    // `.env` files with no destination vault: the token rides the extras
    // (the vault fields only go with a target).
    if (confirmToken && !target) extras.confirm_token = confirmToken
    const vault: CloneVaultRun = {
      carryVault: !!target,
      vaultOptions: target
        ? {
            target,
            cloneId,
            secrets: secretChoices,
            ...(confirmToken ? { confirmToken } : {}),
            ...(st.ownBoxMove ? { ownBoxMove: true } : {}),
          }
        : {},
    }
    // A pull opens on ITS first stage (packing on the server); the
    // orchestration's first onStage overwrites either way.
    set({ phase: 'running', confirming: false, stage: st.pull ? 'packing' : 'bundling' })
    st.onConfirm?.(st.carrySecrets, st.includeAllHistory, extras, vault)
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

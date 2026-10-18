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
  PULL_VAULT_LABEL,
  type ClonePullDeps,
} from './clone-pull'
import { useCloneToDialogStore, type CloneOptionDeps, type CloneVaultRun } from '@/stores/clone-to-dialog'
import { cloneVaultConfirm, fetchClonePreview, type CloneExtras } from './clone-options'
import {
  approveProposal,
  cloneProposalConfirm,
  finishProposal,
  type CloneProposal,
} from './clone-proposals'
import { askVaultRecipient, type VaultRecipientLookup } from './clone-vault'
import type { ServerScope } from '@/kessel/server-scope'
import { useProjectsStore } from '@/stores/projects'
import { useToastStore } from '@/stores/toast'
import type { ConnectHost } from '@/stores/connect-host'
import { primaryScope } from '@/kessel/server-scope'

/**
 * The options-phase collaborators (0.46.0) bound to the SOURCE server's
 * scope, captured when the dialog opens — before a push switches the
 * window to the destination. The preview (slice A's plan) and the
 * `clone-carry` confirm run on the source, where `clone/bundle` /
 * `clone/pack` check the token. `lookupTarget` names the destination vault
 * (never throws; a note means no secrets this time).
 */
export function defaultCloneOptionDeps(
  scope: ServerScope,
  projectPath: string,
  projectName: string,
  lookupTarget: () => Promise<VaultRecipientLookup>,
): CloneOptionDeps {
  const post = async <T = unknown>(s: ServerScope, route: string, body?: unknown): Promise<T> => {
    const { daemonCliPost } = await import('./daemon-cli')
    return daemonCliPost<T>(s, route, body)
  }
  return {
    lookupTarget,
    fetchPreview: (carry, sinceDays) => fetchClonePreview(post, scope, projectPath, carry, sinceDays),
    carryConfirm: (carry, methods, askPassword) =>
      cloneVaultConfirm(post, scope, projectPath, carry, methods, `clone ${projectName}`, askPassword),
    // An agent's proposal (0.46.1): one confirm approves it and covers the
    // copy; the approve and the finish go to the SOURCE server too.
    proposalConfirm: (carry, askPassword, proposalId) =>
      cloneProposalConfirm(post, scope, projectPath, carry, proposalId, `approve and clone ${projectName}`, askPassword),
    approveProposal: (id, approveToken) => approveProposal(post, scope, id, approveToken),
    finishProposal: (id, ok, detail) => finishProposal(post, scope, id, ok, detail),
  }
}

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
  optionDeps: CloneOptionDeps = defaultCloneOptionDeps(primaryScope(), projectPath, projectName, () =>
    // The destination's vault over its saved login (the window is still on
    // the source); labelled as the run labels it (part of the plan).
    askVaultRecipient(() => deps.destVaultRecipient(host), host.label),
  ),
): void {
  const store = useCloneToDialogStore.getState()
  store.start({
    projectPath,
    projectName,
    host,
    optionDeps,
    onConfirm: (carrySecrets, includeAllHistory, extras, vault) => {
      void runClone(projectPath, host, deps, carrySecrets, includeAllHistory, extras, vault)
    },
  })
}

/**
 * Open the Clone dialog prefilled from an agent's clone proposal (0.46.1,
 * `k2 clone propose`): its Thread, history and per-secret choices start as
 * the agent proposed them, the person can change them, and the primary
 * button approves and runs it with ONE confirm. `host` is the proposal's
 * target among this window's Connect hosts.
 */
export function startCloneProposal(
  proposal: CloneProposal,
  projectPath: string,
  projectName: string,
  host: ConnectHost,
  deps: CloneDeps = defaultCloneDeps(),
  optionDeps: CloneOptionDeps = defaultCloneOptionDeps(primaryScope(), projectPath, projectName, () =>
    askVaultRecipient(() => deps.destVaultRecipient(host), host.label),
  ),
): void {
  useCloneToDialogStore.getState().start({
    projectPath,
    projectName,
    host,
    optionDeps,
    proposal,
    onConfirm: (carrySecrets, includeAllHistory, extras, vault) => {
      void runClone(projectPath, host, deps, carrySecrets, includeAllHistory, extras, vault)
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
  optionDeps: CloneOptionDeps = defaultCloneOptionDeps(primaryScope(), projectPath, projectName, () =>
    askVaultRecipient(() => deps.localVaultRecipient(), PULL_VAULT_LABEL),
  ),
): void {
  const store = useCloneToDialogStore.getState()
  store.start({
    projectPath,
    projectName,
    host: null,
    pull: true,
    optionDeps,
    onConfirm: (carrySecrets, includeAllHistory, extras, vault) => {
      void runPull(projectPath, projectName, deps, carrySecrets, includeAllHistory, extras, vault)
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
  extras?: CloneExtras,
  vault?: CloneVaultRun,
): Promise<void> {
  let lastStage: CloneStage = 'packing'
  try {
    const result = await cloneWorkspaceToThisComputer(primaryScope(),
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
        onVaultCarry: (outcome) => useCloneToDialogStore.getState().setVaultOutcome(outcome),
      },
      carrySecrets,
      includeAllHistory,
      vault?.carryVault ?? false,
      vault?.vaultOptions ?? {},
      extras,
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
  extras?: CloneExtras,
  vault?: CloneVaultRun,
): Promise<void> {
  try {
    await cloneWorkspaceTo(primaryScope(),
      projectPath,
      host,
      deps,
      {
        onStage: (stage) => useCloneToDialogStore.getState().setStage(stage),
        onBundled: (summary) => useCloneToDialogStore.getState().setSummary(summary),
        onDone: (result) => useCloneToDialogStore.getState().setDone(result),
        onError: (message) => useCloneToDialogStore.getState().setError(message),
        onVaultCarry: (outcome) => useCloneToDialogStore.getState().setVaultOutcome(outcome),
      },
      carrySecrets,
      includeAllHistory,
      vault?.carryVault ?? false,
      vault?.vaultOptions ?? {},
      extras,
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

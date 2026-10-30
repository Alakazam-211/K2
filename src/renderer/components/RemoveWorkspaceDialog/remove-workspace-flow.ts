// Removing a workspace that maintains Apps (0.46.7, App-governed Spaces S7
// follow-up). The daemon decides every rule; this sequences the calls so
// the dialog never shows a raw refusal and never stops halfway:
//
// 1. Before anything changes (the file teardown included), ask which Apps
//    the workspace maintains. Any: stop and show them (step "apps"); the
//    person picks "Uninstall its Apps and remove".
// 2. Uninstall them in one all-or-nothing call (`app/release`, forRemoval).
//    If people would lose their logins, grants, shared secrets or chat
//    Spaces, the daemon refuses before any App goes: show every App and
//    what it would lose (step "loss"); "Remove anyway" repeats the call
//    with dropAccess.
// 3. Only then the teardown (keep_current / restore_original) and the
//    removal itself. A removal still refused for Apps (one appeared
//    meanwhile, or Apps (beta) hid the list) goes back to step "apps".

import { accessLossOf, appsBlockingRemovalOf, type AppLoss } from '@/lib/app-access-loss'

export type RemovalStep = { kind: 'done' } | { kind: 'apps'; apps: string[] } | { kind: 'loss'; loss: AppLoss[] }

export interface RemovalDeps {
  /** The App ids the workspace maintains. */
  appIds: (projectId: string) => Promise<string[]>
  /** Uninstall them all, or refuse with nothing uninstalled. */
  uninstallApps: (projectId: string, dropAccess: boolean) => Promise<void>
  /** The file teardown of the chosen mode (none for deregister-only). */
  teardown: () => Promise<void>
  /** The removal itself (projects/delete). */
  remove: (projectId: string) => Promise<void>
}

export interface RemovalChoice {
  /** The person chose "Uninstall its Apps and remove". */
  uninstallApps: boolean
  /** The person chose "Remove anyway" after seeing what would be lost. */
  dropAccess: boolean
}

export async function runRemoval(projectId: string, choice: RemovalChoice, deps: RemovalDeps): Promise<RemovalStep> {
  if (choice.uninstallApps) {
    try {
      await deps.uninstallApps(projectId, choice.dropAccess)
    } catch (e) {
      const loss = accessLossOf(e)
      if (loss) return { kind: 'loss', loss }
      throw e
    }
  } else {
    let apps: string[] = []
    try {
      apps = await deps.appIds(projectId)
    } catch {
      // Apps (beta) off or an older daemon: the removal's own guard
      // (workspace_has_apps) still answers below.
      apps = []
    }
    if (apps.length > 0) return { kind: 'apps', apps }
  }
  await deps.teardown()
  try {
    await deps.remove(projectId)
  } catch (e) {
    const apps = appsBlockingRemovalOf(e)
    if (apps) return { kind: 'apps', apps }
    throw e
  }
  return { kind: 'done' }
}

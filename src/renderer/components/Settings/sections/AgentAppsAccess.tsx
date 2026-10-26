// The workspace card's App permissions (prd-agent-app-permissions-v1 rev 2
// §2, §3, §5; slice H, 0.46.3). Replaces the single "Allow this agent to
// manage Apps" switch:
//
//   Switch 1  "Build and run Apps"
//     Switch 2  "Manage its Apps end to end"
//   "Let this agent manage its Apps' users" (0225: its own switch, the
//   owner's both ways; Rosson 2026-10-26: in this box with the others)
//   "Loosening the wall always asks you."
//   Cores     Database · Files and rooms · Thread · Mail · Sew
//
// The owner turns things on; an Admin may only turn them off (the controls
// are shown, and what an Admin can't do is disabled with the reason); a
// Member sees them read-only. Every write is POST /cli/agent-access/set to
// the window's server; the daemon checks the role again and is the truth.
// Optimistic store patch, rolled back on a refusal; it never refetches the
// projects list itself.
// Live: the projects store re-reads on the daemon's SyncProjects event, so
// a change from another client (or `k2 app access`) shows here.

import React, { useEffect, useState } from 'react'
import { SquareCheckbox } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import { emit } from '@tauri-apps/api/event'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { useProjectsStore, noteOptimisticProjectsMutationSuccess, type ProjectWithWorkspaces } from '@/stores/projects'
import {
  APP_CORES,
  canSet,
  canSetUsers,
  lockedUsersReason,
  lockedReason,
  parseAppCores,
  setAppCore,
  setAppSwitch,
  type AppCoreId,
  type ViewerRole,
} from './app-access-api'

/** Other windows re-read projects after a host-aware write. */
function emitProjectsChanged(): void {
  void emit('sync:projects').catch((e) => console.warn('[apps-access] sync:projects emit failed:', e))
}

type Patch = Partial<
  Pick<ProjectWithWorkspaces, 'agentsCanManageSkin' | 'agentsCanManageAppsFull' | 'agentsCanManageAppUsers' | 'appCores'>
>

function patchProject(id: string, patch: Patch): void {
  useProjectsStore.setState((s) => ({
    projects: s.projects.map((p) => (p.id === id ? { ...p, ...patch } : p)),
  }))
}

function Switch({
  on,
  label,
  disabled,
  title,
  testId,
  settingsId,
  onFlip,
}: {
  on: boolean
  label: string
  disabled: boolean
  title: string
  testId: string
  settingsId?: string
  onFlip: () => void
}): React.JSX.Element {
  return (
    <button
      type="button"
      onClick={onFlip}
      role="switch"
      aria-checked={on}
      aria-label={label}
      disabled={disabled}
      title={title}
      data-testid={testId}
      data-settings-id={settingsId}
      className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 disabled:cursor-not-allowed ${
        on ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
      }`}
    >
      <span
        className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${on ? 'translate-x-3.5' : 'translate-x-0.5'}`}
      />
    </button>
  )
}

export function AgentAppsAccess({
  project,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: labels and controls only (no descriptions). */
  compact?: boolean
  /** Shown but not changeable: a room on another server. */
  readOnly?: boolean
}): React.JSX.Element {
  const [role, setRole] = useState<ViewerRole | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void fetchViewerRole().then((r) => {
      if (!cancelled) setRole(r)
    })
    return () => {
      cancelled = true
    }
  }, [])

  const build = (project.agentsCanManageSkin ?? 0) === 1
  const full = build && (project.agentsCanManageAppsFull ?? 0) === 1
  const users = (project.agentsCanManageAppUsers ?? 0) === 1
  const cores = parseAppCores(project.appCores ?? '[]')
  const may = (cur: boolean, next: boolean): boolean => !readOnly && busy === null && canSet(role, cur, next)

  const run = async (key: string, optimistic: Patch, write: () => Promise<unknown>): Promise<void> => {
    const before: Patch = {
      agentsCanManageSkin: project.agentsCanManageSkin,
      agentsCanManageAppsFull: project.agentsCanManageAppsFull,
      agentsCanManageAppUsers: project.agentsCanManageAppUsers,
      appCores: project.appCores,
    }
    setBusy(key)
    setError(null)
    patchProject(project.id, optimistic)
    try {
      await write()
      noteOptimisticProjectsMutationSuccess()
      emitProjectsChanged()
    } catch (e) {
      patchProject(project.id, before)
      setError(daemonErrorText(e))
    } finally {
      setBusy(null)
    }
  }

  const flipBuild = (): void => {
    const next = !build
    if (!may(build, next)) return
    void run('build', next ? { agentsCanManageSkin: 1 } : { agentsCanManageSkin: 0, agentsCanManageAppsFull: 0 }, () =>
      setAppSwitch(project.path, 'apps_build', next),
    )
  }

  const flipFull = (): void => {
    const next = !full
    if (!build || !may(full, next)) return
    void run('full', { agentsCanManageAppsFull: next ? 1 : 0 }, () => setAppSwitch(project.path, 'apps_manage_full', next))
  }

  const flipUsers = (): void => {
    const next = !users
    if (readOnly || busy !== null || !canSetUsers(role, users, next)) return
    void run('users', { agentsCanManageAppUsers: next ? 1 : 0 }, () => setAppSwitch(project.path, 'app_users', next))
  }

  const flipCore = (core: AppCoreId): void => {
    const on = cores.includes(core)
    if (!may(on, !on)) return
    const after = on ? cores.filter((c) => c !== core) : [...cores, core]
    const ordered = APP_CORES.map((c) => c.id).filter((id) => after.includes(id))
    void run(`core:${core}`, { appCores: JSON.stringify(ordered) }, () => setAppCore(project.path, core, !on))
  }

  const lock = readOnly ? null : role === 'owner' ? null : role === 'admin' ? 'admin' : role === null ? 'pending' : 'member'
  const desc = compact ? 'hidden' : 'text-[10px] text-[var(--color-text-muted)] mt-0.5 leading-relaxed'

  return (
    <div
      className={compact ? 'px-3 py-1.5 space-y-2' : 'border border-[var(--color-border)] p-3 space-y-3'}
      data-testid="agent-apps-access"
    >
      <div className="flex items-start gap-3">
        <Switch
          on={build}
          label="Build and run Apps"
          disabled={!may(build, !build)}
          title={lockedReason(role, build) ?? (build ? 'This agent builds and runs its own Apps' : 'This agent can’t change Apps')}
          testId="apps-switch-build"
          settingsId="projects.agents-can-manage-skin"
          onFlip={flipBuild}
        />
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">
            Build and run Apps{' '}
            <span className="text-[10px] font-normal text-[var(--color-text-muted)]">Apps (beta)</span>
          </div>
          <div className={desc}>
            Builds, installs, deploys, runs and uninstalls its own Apps inside default limits (512 MiB each, up to 3 Apps),
            moves its own services and labels in, and edits their roles and App passes. Off: every change waits for you.
          </div>
        </div>
      </div>

      <div className="flex items-start gap-3 pl-6" data-testid="apps-switch-full-row">
        <Switch
          on={full}
          label="Manage its Apps end to end"
          disabled={!build || !may(full, !full)}
          title={
            !build
              ? 'Turn on Build and run Apps first'
              : (lockedReason(role, full) ?? (full ? 'This agent widens its own Apps' : 'Its raises wait for you'))
          }
          testId="apps-switch-full"
          onFlip={flipFull}
        />
        <div className="flex-1 min-w-0">
          <div className={`text-xs font-medium ${build ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-muted)]'}`}>
            Manage its Apps end to end
          </div>
          <div className={desc}>
            Raises its own Apps&rsquo; budgets and quotas, allows outbound (egress) hosts and box services, mints App keys,
            resumes an App it suspended and sends push, with no approval. Never adds a core; never its Apps&rsquo; users.
          </div>
        </div>
      </div>

      <div className="flex items-start gap-3" data-testid="apps-switch-users-row">
        <Switch
          on={users}
          label="Let this agent manage its Apps’ users"
          disabled={readOnly || busy !== null || !canSetUsers(role, users, !users)}
          title={lockedUsersReason(role) ?? (users ? 'This agent manages its own Apps’ users' : 'Its Apps’ users wait for you')}
          testId="apps-switch-users"
          settingsId="projects.agents-can-manage-app-users"
          onFlip={flipUsers}
        />
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">Let this agent manage its Apps&rsquo; users</div>
          <div className={desc}>
            Adds and removes App users and test guests of its own Apps, sets their passwords, email and names, unlocks them,
            gives them roles and its own room, invites people, and gives its Apps the guests-rooms grant in its own room. The
            users it makes are tied to it. Never a box login or another workspace&rsquo;s guests. Off: adding a test guest or
            the guests-rooms grant is a card for you; the rest is refused until you turn it on.
          </div>
        </div>
      </div>

      <p className="text-[10px] text-[var(--color-text-secondary)]" data-testid="apps-wall-line">
        Loosening the wall always asks you.
      </p>

      <div className="space-y-1.5" role="group" aria-label="App cores">
        <div className="text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">What its Apps may use</div>
        {APP_CORES.map((c) => {
          const on = cores.includes(c.id)
          const disabled = !may(on, !on)
          return (
            <label
              key={c.id}
              className={`flex items-start gap-2 text-xs no-drag ${disabled ? 'cursor-default' : 'cursor-pointer'}`}
              title={lockedReason(role, on) ?? undefined}
            >
              <SquareCheckbox
                checked={on}
                disabled={disabled}
                onChange={() => flipCore(c.id)}
                aria-label={c.label}
                data-testid={`apps-core-${c.id}`}
                className="mt-0.5"
              />
              <span className="min-w-0">
                <span className="text-[var(--color-text-primary)]">{c.label}</span>
                {c.later ? (
                  <span className="ml-1.5 text-[10px] text-[var(--color-text-muted)]" data-testid={`apps-core-${c.id}-later`}>
                    {c.later}
                  </span>
                ) : null}
                {!compact ? <span className="block text-[10px] text-[var(--color-text-muted)]">{c.detail}</span> : null}
              </span>
            </label>
          )
        })}
      </div>

      {lock === 'admin' ? (
        <p className="text-[10px] text-[var(--color-text-secondary)]" data-testid="apps-access-locked">
          You can turn these off. Only the server&rsquo;s owner turns them on.
        </p>
      ) : lock === 'member' ? (
        <p className="text-[10px] text-[var(--color-text-secondary)]" data-testid="apps-access-locked">
          Only the server&rsquo;s owner or an admin can change these.
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="text-[10px] text-[var(--color-status-error-soft)]" data-testid="apps-access-error">
          {error}
        </p>
      ) : null}
    </div>
  )
}

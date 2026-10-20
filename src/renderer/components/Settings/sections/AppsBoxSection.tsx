// Settings → Apps → This server: every App on the box with the agent that
// maintains it (prd-agent-app-permissions-v1 rev 2 §4.1, §5, slice H;
// 0.46.3), and the one-time review note (Q4).
//
// - Rows come from the box view (GET /cli/app/list?box=1, `k2 app list
//   --box`): the App, its maintaining agent (name, address, picture and
//   colour), its state (parked / unassigned included), memory budget and
//   hostname, and that agent's switches and cores, read-only, with a link
//   to the agent's workspace card (where they are changed).
// - A parked or orphaned App shows the owner's ways out: Unpark (to an
//   agent), Hand over, Park, Uninstall (no purge). The daemon checks who
//   may (owner for hand over / unpark; owner or admin for park and
//   uninstall).
// - The review note: the agents that kept "Build and run Apps" from the
//   old "Allow this agent to manage Apps" switch, each with Review (opens
//   its card) and Dismiss (the daemon drops it from the list, so every
//   client agrees).
// - A Thread with each App's agent beside its settings is 0.46.4 (slice I).
//
// Thin client: no polling. It re-reads on open, on a server switch, after
// each gesture and when the window regains focus; the switches and cores
// shown come from the projects store when this server lists the agent (so
// a change on the card shows at once). It never refetches the projects list.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Callout } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useConnectHostStore } from '@/stores/connect-host'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { SettingsGroup } from '../controls/SettingControls'
import {
  APP_CORES,
  dismissReview,
  fetchBoxApps,
  fetchReview,
  handOverApp,
  memoryLabel,
  needsOwner,
  parkApp,
  parseAppCores,
  stateLabel,
  uninstallApp,
  unparkApp,
  type BoxApp,
  type ReviewRow,
  type ViewerRole,
} from './app-access-api'
import { isAppsOff } from './apps-server-api'

const muted = 'text-[var(--color-text-muted)]'
const secondary = 'text-[var(--color-text-secondary)]'

type Load = { kind: 'loading' } | { kind: 'off' } | { kind: 'ready'; apps: BoxApp[] } | { kind: 'error'; text: string }

function openCard(workspaceId: string): void {
  useSettingsStore.getState().openSettings('projects', workspaceId, 'agent')
}

/** What the row shows for the agent's switches and cores: the projects
 *  store's live values when this server lists the workspace, else the box
 *  view's. */
function accessLine(app: BoxApp, project: ProjectWithWorkspaces | undefined): string {
  const build = project ? (project.agentsCanManageSkin ?? 0) === 1 : app.buildAndRun === true
  const full = build && (project ? (project.agentsCanManageAppsFull ?? 0) === 1 : app.manageEndToEnd === true)
  const cores = project ? parseAppCores(project.appCores ?? '[]') : app.cores
  const names = APP_CORES.filter((c) => cores.includes(c.id)).map((c) => c.label)
  const sw = !build ? 'Build and run Apps off' : full ? 'Build and run, manage end to end' : 'Build and run Apps'
  return `${sw} · uses ${names.length ? names.join(', ') : 'no cores'}`
}

export function AppsBoxSection(): React.JSX.Element | null {
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const projects = useProjectsStore((s) => s.projects)
  const [role, setRole] = useState<ViewerRole | null>(null)
  const [load, setLoad] = useState<Load>({ kind: 'loading' })
  const [review, setReview] = useState<ReviewRow[]>([])
  const [notice, setNotice] = useState<{ tone: 'ok' | 'error'; text: string } | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [target, setTarget] = useState<Record<string, string>>({})
  const [confirmUninstall, setConfirmUninstall] = useState<string | null>(null)
  const mounted = useRef(true)

  const refresh = useCallback(async (): Promise<void> => {
    try {
      const apps = await fetchBoxApps()
      if (mounted.current) setLoad({ kind: 'ready', apps })
    } catch (e) {
      if (!mounted.current) return
      setLoad(isAppsOff(e) ? { kind: 'off' } : { kind: 'error', text: daemonErrorText(e) })
      return
    }
    try {
      const rows = await fetchReview()
      if (mounted.current) setReview(rows)
    } catch {
      // The review list is a nicety: a refusal leaves it empty.
      if (mounted.current) setReview([])
    }
  }, [])

  useEffect(() => {
    mounted.current = true
    setLoad({ kind: 'loading' })
    setNotice(null)
    void refresh()
    void fetchViewerRole().then((r) => {
      if (mounted.current) setRole(r)
    })
    const onFocus = (): void => void refresh()
    window.addEventListener('focus', onFocus)
    return () => {
      mounted.current = false
      window.removeEventListener('focus', onFocus)
    }
  }, [refresh, activeHost])

  const act = async (key: string, ok: string, f: () => Promise<void>): Promise<void> => {
    setBusy(key)
    setNotice(null)
    try {
      await f()
      setNotice({ tone: 'ok', text: ok })
    } catch (e) {
      setNotice({ tone: 'error', text: daemonErrorText(e) })
    } finally {
      setBusy(null)
      setConfirmUninstall(null)
      await refresh()
    }
  }

  if (load.kind === 'off') return null

  const isOwner = role === 'owner'
  const isStaff = role === 'owner' || role === 'admin'
  const byId = new Map(projects.map((p) => [p.id, p]))
  const agents = projects.filter((p) => p.path).sort((a, b) => a.name.localeCompare(b.name))

  return (
    <div className="max-w-2xl space-y-4 mt-6" data-testid="apps-box">
      {isStaff && review.length > 0 ? (
        <Callout tone="info" data-testid="apps-review-note">
          <div className="space-y-1.5">
            <p className="text-[11px]">
              These agents could manage Apps before; they now have Build and run Apps on. Review what each one may do.
            </p>
            <ul className="space-y-1">
              {review.map((r) => (
                <li key={r.workspaceId} className="flex items-center gap-2 text-[11px]" data-testid={`apps-review-${r.workspaceId}`}>
                  <span className="font-mono text-[var(--color-text-primary)]">{byId.get(r.workspaceId)?.name ?? r.handle}</span>
                  <button
                    type="button"
                    className={`${secondary} underline no-drag cursor-pointer`}
                    onClick={() => openCard(r.workspaceId)}
                  >
                    Review
                  </button>
                  <button
                    type="button"
                    className={`${muted} underline no-drag cursor-pointer disabled:opacity-50`}
                    disabled={busy !== null}
                    onClick={() =>
                      void act(`review:${r.workspaceId}`, `${r.handle} reviewed.`, () => dismissReview(r.workspaceId))
                    }
                  >
                    Dismiss
                  </button>
                </li>
              ))}
            </ul>
          </div>
        </Callout>
      ) : null}

      <SettingsGroup title="Apps on this server">
        <div className="py-2 space-y-2">
          <p className={`text-[11px] ${muted}`}>
            Every App here and the agent that maintains it. What an agent may do is set on its card: Settings → Workspaces →
            the agent → Agent → Apps.
          </p>
          {notice ? (
            <Callout tone={notice.tone} data-testid="apps-box-notice">
              {notice.text}
            </Callout>
          ) : null}
          {load.kind === 'loading' ? <p className={`text-[11px] ${muted}`}>Loading Apps…</p> : null}
          {load.kind === 'error' ? <Callout tone="error">{load.text}</Callout> : null}
          {load.kind === 'ready' && load.apps.length === 0 ? (
            <p className={`text-[11px] ${muted}`} data-testid="apps-box-empty">
              No Apps on this server yet.
            </p>
          ) : null}
          {load.kind === 'ready' ? (
            <ul className="divide-y divide-[var(--color-border)] border border-[var(--color-border)]">
              {load.apps.map((app) => {
                const wsId = app.maintainer.workspaceId
                const project = wsId ? byId.get(wsId) : undefined
                const owner = needsOwner(app)
                const to = target[app.appId] ?? ''
                return (
                  <li key={app.appId} className="p-3 space-y-1.5" data-testid={`apps-row-${app.appId}`} data-state={app.state}>
                    <div className="flex items-start gap-3">
                      {project ? (
                        <ProjectAvatar
                          projectPath={project.path}
                          projectName={project.name}
                          projectColor={project.color}
                          projectId={project.id}
                          iconUrl={project.iconUrl}
                          size={24}
                        />
                      ) : (
                        <span
                          aria-hidden
                          className="w-6 h-6 flex-shrink-0 flex items-center justify-center text-[10px] border border-[var(--color-border)] text-[var(--color-text-muted)]"
                        >
                          ?
                        </span>
                      )}
                      <div className="flex-1 min-w-0">
                        <div className="flex items-baseline gap-2 flex-wrap">
                          <span className="text-xs font-mono text-[var(--color-text-primary)]">{app.appId}</span>
                          <span className={`text-[10px] ${owner ? 'text-[var(--color-status-warn-amber-soft)]' : muted}`}>
                            {stateLabel(app)}
                          </span>
                        </div>
                        <div className={`text-[11px] ${secondary}`} data-testid={`apps-row-${app.appId}-agent`}>
                          {app.maintainer.kind === 'agent'
                            ? `${app.maintainer.agent ?? app.maintainer.handle ?? 'an agent'}${app.maintainer.address ? ` · ${app.maintainer.address}` : ''}`
                            : 'No agent maintains it'}
                        </div>
                        <div className={`text-[10px] ${muted}`}>
                          {memoryLabel(app.memoryLimitBytes)}
                          {app.hostnames.length ? ` · ${app.hostnames.join(', ')}` : ' · no public hostname'}
                        </div>
                        {app.maintainer.kind === 'agent' ? (
                          <div className={`text-[10px] ${muted}`} data-testid={`apps-row-${app.appId}-access`}>
                            {accessLine(app, project)}
                            {wsId ? (
                              <>
                                {' · '}
                                <button
                                  type="button"
                                  className={`${secondary} underline no-drag cursor-pointer`}
                                  onClick={() => openCard(wsId)}
                                >
                                  Agent settings
                                </button>
                              </>
                            ) : null}
                          </div>
                        ) : null}
                        {/* 0.46.4 (slice I): a Thread with this App's agent beside its settings. */}
                      </div>
                    </div>
                    {owner && isStaff ? (
                      <div className="flex items-center gap-2 flex-wrap pl-9" data-testid={`apps-row-${app.appId}-actions`}>
                        {isOwner ? (
                          <>
                            <select
                              aria-label={`Agent for ${app.appId}`}
                              className="px-2 py-1 text-[11px] bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] no-drag"
                              value={to}
                              onChange={(e) => setTarget((t) => ({ ...t, [app.appId]: e.target.value }))}
                            >
                              <option value="">Choose an agent…</option>
                              {agents.map((p) => (
                                <option key={p.id} value={p.id}>
                                  {p.name}
                                </option>
                              ))}
                            </select>
                            <Button
                              disabled={!to || busy !== null}
                              onClick={() =>
                                void act(
                                  `give:${app.appId}`,
                                  app.parked
                                    ? `${app.appId} is unparked: it is stopped until its agent starts it.`
                                    : `${app.appId} is handed over.`,
                                  () => (app.parked ? unparkApp(app.appId, to) : handOverApp(app.appId, to)),
                                )
                              }
                            >
                              {app.parked ? 'Unpark' : 'Hand over'}
                            </Button>
                          </>
                        ) : null}
                        {!app.parked ? (
                          <Button
                            variant="ghost"
                            disabled={busy !== null}
                            onClick={() => void act(`park:${app.appId}`, `${app.appId} is parked.`, () => parkApp(app.appId))}
                          >
                            Park
                          </Button>
                        ) : null}
                        {confirmUninstall === app.appId ? (
                          <>
                            <Button
                              variant="danger"
                              disabled={busy !== null}
                              onClick={() =>
                                void act(`rm:${app.appId}`, `${app.appId} is uninstalled; its data stays.`, () =>
                                  uninstallApp(app.appId),
                                )
                              }
                            >
                              Uninstall {app.appId} (data stays)
                            </Button>
                            <Button variant="ghost" onClick={() => setConfirmUninstall(null)}>
                              Cancel
                            </Button>
                          </>
                        ) : (
                          <Button variant="ghost" disabled={busy !== null} onClick={() => setConfirmUninstall(app.appId)}>
                            Uninstall
                          </Button>
                        )}
                      </div>
                    ) : null}
                  </li>
                )
              })}
            </ul>
          ) : null}
        </div>
      </SettingsGroup>
    </div>
  )
}

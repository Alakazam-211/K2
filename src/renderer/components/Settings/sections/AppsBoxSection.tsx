// Settings → Apps → This server: every App on the box with the agent that
// maintains it (prd-agent-app-permissions-v1 rev 2 §4.1, §5, slice H;
// 0.46.3), and the one-time review note (Q4).
//
// - Rows come from the box view (GET /cli/app/list?box=1, `k2 app list
//   --box`): the App, its maintaining agent (name, address, picture and
//   colour), its state (parked / unassigned included), memory budget and
//   hostname, and that agent's switches and cores, read-only, with a link
//   to the agent's workspace card (where they are changed).
// - Every App has "Hand over…" for the owner (0.46.5, prd-app-handoff-v1
//   §4.13; an admin sees it disabled): the sheet shows what moves and
//   approves the handover card with a fresh vault confirm. A parked App's
//   is "Unpark…" (the same card). Within the 72 h window a handed-over App
//   shows Undo and Finish.
// - A parked or orphaned App also shows Park and Uninstall (no purge). The
//   daemon checks who may (owner for hand over / unpark / undo; owner or
//   admin for park and uninstall).
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
import { accessLossOf, type AppLoss } from '@/lib/app-access-loss'
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
  fetchHandovers,
  fetchReview,
  finishHandover,
  memoryLabel,
  needsOwner,
  parkApp,
  parseAppCores,
  postgresChip,
  requestUndo,
  stateLabel,
  uninstallApp,
  type BoxApp,
  type HandoverRow,
  type ReviewRow,
  type ViewerRole,
} from './app-access-api'
import { AppHandoverSheet, approveWithConfirm } from './AppHandoverSheet'
import { APP_APPROVE_CONFIRM_ACTION } from './app-approvals-api'
import { vaultApi } from './vault-api'
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
  const [sheet, setSheet] = useState<string | null>(null)
  const [handovers, setHandovers] = useState<Map<string, HandoverRow>>(new Map())
  const [confirmUninstall, setConfirmUninstall] = useState<string | null>(null)
  // 0.46.7: an uninstall the daemon refused because people would lose
  // access (409 uninstall_drops_access): what they'd lose, and "anyway".
  const [loss, setLoss] = useState<AppLoss | null>(null)
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
    try {
      // 0.46.5: handovers inside their undo window (newest per App).
      const rows = await fetchHandovers()
      const m = new Map<string, HandoverRow>()
      for (const r of rows) if (r.inWindow && !m.has(r.appId)) m.set(r.appId, r)
      if (mounted.current) setHandovers(m)
    } catch {
      // A Member (or an older daemon) sees no handovers: no Undo/Finish.
      if (mounted.current) setHandovers(new Map())
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

  const uninstall = async (appId: string, dropAccess: boolean): Promise<void> => {
    setBusy(`rm:${appId}`)
    setNotice(null)
    try {
      await uninstallApp(appId, dropAccess)
      setLoss(null)
      setNotice({ tone: 'ok', text: `${appId} is uninstalled; its data stays.` })
    } catch (e) {
      const lost = accessLossOf(e, appId)
      if (lost) setLoss(lost[0])
      else setNotice({ tone: 'error', text: daemonErrorText(e) })
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

  const undo = async (appId: string): Promise<void> => {
    const { request } = await requestUndo(appId)
    const { methods } = await vaultApi.confirmStart(APP_APPROVE_CONFIRM_ACTION, request)
    if (!methods.includes('local-auth')) {
      throw new Error(JSON.stringify({ error: { code: 'confirm_elsewhere', hint: 'The undo card waits in App Approvals: approve it there with your password.' } }))
    }
    await approveWithConfirm(request, `undo the handover of ${appId}`, 'local-auth')
  }

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
                const recent = handovers.get(app.appId)
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
                          {postgresChip(app.database) ? (
                            <span
                              className="ml-2 px-1.5 py-px border border-[var(--color-border)] text-[10px]"
                              data-testid={`apps-row-${app.appId}-postgres`}
                              title={app.database?.name ? `database ${app.database.name}` : undefined}
                            >
                              {postgresChip(app.database)}
                            </span>
                          ) : null}
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
                    {recent && isStaff ? (
                      <div className={`flex items-center gap-2 flex-wrap pl-9 text-[10px] ${muted}`} data-testid={`apps-row-${app.appId}-handover`}>
                        <span>
                          Handed over from {recent.fromHandle ?? 'the owner'} to {recent.toHandle}
                          {recent.rekeyState === 'failed' ? ' · its keys didn’t change yet' : ''}
                        </span>
                        {isOwner ? (
                          <>
                            <Button
                              variant="ghost"
                              disabled={busy !== null}
                              onClick={() => void act(`undo:${app.appId}`, `${app.appId} went back to ${recent.fromHandle ?? 'the owner'}.`, () => undo(app.appId))}
                            >
                              Undo
                            </Button>
                            <Button
                              variant="ghost"
                              disabled={busy !== null}
                              onClick={() => void act(`finish:${app.appId}`, `${app.appId}: the undo window is closed.`, () => finishHandover(app.appId))}
                            >
                              Finish
                            </Button>
                          </>
                        ) : null}
                      </div>
                    ) : null}
                    {isStaff ? (
                      <div className="flex items-center gap-2 flex-wrap pl-9" data-testid={`apps-row-${app.appId}-actions`}>
                        <Button
                          variant="ghost"
                          disabled={!isOwner || busy !== null}
                          title={isOwner ? undefined : 'Only the server’s owner hands an App over'}
                          onClick={() => setSheet(sheet === app.appId ? null : app.appId)}
                        >
                          {app.parked ? 'Unpark…' : 'Hand over…'}
                        </Button>
                        {owner && !app.parked ? (
                          <Button
                            variant="ghost"
                            disabled={busy !== null}
                            onClick={() => void act(`park:${app.appId}`, `${app.appId} is parked.`, () => parkApp(app.appId))}
                          >
                            Park
                          </Button>
                        ) : null}
                        {owner && confirmUninstall === app.appId ? (
                          <>
                            <Button
                              variant="danger"
                              disabled={busy !== null}
                              onClick={() => void uninstall(app.appId, false)}
                            >
                              Uninstall {app.appId} (data stays)
                            </Button>
                            <Button variant="ghost" onClick={() => setConfirmUninstall(null)}>
                              Cancel
                            </Button>
                          </>
                        ) : owner ? (
                          <Button variant="ghost" disabled={busy !== null} onClick={() => setConfirmUninstall(app.appId)}>
                            Uninstall
                          </Button>
                        ) : null}
                      </div>
                    ) : null}
                    {loss && loss.app === app.appId ? (
                      <div className="pl-9" data-testid={`apps-row-${app.appId}-loss`}>
                        <Callout tone="warn">
                          <p>Uninstalling {app.appId} would take away:</p>
                          <ul className="list-disc pl-4 my-1">
                            {loss.parts.map((p) => (
                              <li key={p}>{p}</li>
                            ))}
                          </ul>
                          <p>Reinstalling makes a new App that gets none of this back. To add a public URL instead, use k2 app expose {app.appId} --tunnel.</p>
                          <div className="flex gap-2 mt-2">
                            <Button variant="danger" disabled={busy !== null} onClick={() => void uninstall(app.appId, true)}>
                              Uninstall anyway
                            </Button>
                            <Button variant="ghost" disabled={busy !== null} onClick={() => setLoss(null)}>
                              Keep it
                            </Button>
                          </div>
                        </Callout>
                      </div>
                    ) : null}
                    {sheet === app.appId && isOwner ? (
                      <AppHandoverSheet
                        appId={app.appId}
                        parked={app.parked}
                        currentWorkspace={wsId ?? null}
                        agents={agents}
                        onClose={() => setSheet(null)}
                        onDone={async (text) => {
                          setSheet(null)
                          setNotice({ tone: 'ok', text })
                          await refresh()
                        }}
                      />
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

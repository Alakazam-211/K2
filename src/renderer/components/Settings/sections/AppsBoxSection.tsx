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
import { Button, Callout, OverflowMenu, type OverflowMenuItem } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import { accessLossOf, type AppLoss } from '@/lib/app-access-loss'
import { useSettingsStore } from '@/stores/settings'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useConnectHostStore } from '@/stores/connect-host'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import {
  AVATAR_ROUND,
  SETTING_LIST_ROW_HOVER,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SettingRow,
  SettingsGroup,
  SettingsLoadState,
} from '../controls/SettingControls'
import { AccessSheet } from './AccessDialogs'
import { AppMonogram, StatusDot, appStateWords, serverWords } from './apps-ui'
import {
  APP_CORES,
  dismissReview,
  fetchBoxApps,
  fetchHandovers,
  fetchReview,
  finishHandover,
  needsOwner,
  parkApp,
  parseAppCores,
  postgresChip,
  requestUndo,
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
import { PRODUCT_NAME } from '@/lib/brand'

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
  const sw = !build ? 'Can’t build Apps' : full ? 'Builds, runs and fully manages its Apps' : 'Builds and runs its Apps'
  return `${sw} · ${names.length ? `uses ${names.join(', ')}` : 'uses no shared services'}`
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
      setLoad(isAppsOff(e) ? { kind: 'off' } : { kind: 'error', text: serverWords(daemonErrorText(e)) })
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

  const act = async (key: string, ok: string, f: () => Promise<void>, what = 'That didn’t work'): Promise<void> => {
    setBusy(key)
    setNotice(null)
    try {
      await f()
      setNotice({ tone: 'ok', text: ok })
    } catch (e) {
      setNotice({ tone: 'error', text: `${what}: ${serverWords(daemonErrorText(e))}. Nothing changed; try again.` })
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
      setConfirmUninstall(null)
      setNotice({ tone: 'ok', text: `${appId} is uninstalled; its data stays.` })
    } catch (e) {
      const lost = accessLossOf(e, appId)
      if (lost) setLoss(lost[0])
      else {
        setConfirmUninstall(null)
        setNotice({ tone: 'error', text: `Couldn’t uninstall ${appId}: ${serverWords(daemonErrorText(e))}. It’s still installed; try again.` })
      }
    } finally {
      setBusy(null)
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
      throw new Error(JSON.stringify({ error: { code: 'confirm_elsewhere', hint: 'The undo request waits in App Approvals: approve it there with your password.' } }))
    }
    await approveWithConfirm(request, `undo the handover of ${appId}`, 'local-auth')
  }

  const sheetApp = load.kind === 'ready' && sheet ? load.apps.find((a) => a.appId === sheet) ?? null : null
  const removing = load.kind === 'ready' && confirmUninstall ? load.apps.find((a) => a.appId === confirmUninstall) ?? null : null
  const lossFor = loss ? loss.app : null

  return (
    <div className="space-y-6" data-testid="apps-box">
      {isStaff && review.length > 0 ? (
        <Callout tone="info" className="type-hint" data-testid="apps-review-note">
          <div className="space-y-2">
            <p>These agents could manage Apps before; they now have Build and run Apps on. Review what each one may do.</p>
            <ul className="space-y-1.5">
              {review.map((r) => (
                <li key={r.workspaceId} className="flex items-center justify-between gap-3" data-testid={`apps-review-${r.workspaceId}`}>
                  <span className="text-[var(--color-text-primary)]">{byId.get(r.workspaceId)?.name ?? r.handle}</span>
                  <span className="flex items-center gap-2">
                    <Button onClick={() => openCard(r.workspaceId)}>Review</Button>
                    <Button
                      variant="ghost"
                      disabled={busy !== null}
                      onClick={() => void act(`review:${r.workspaceId}`, `${r.handle} reviewed.`, () => dismissReview(r.workspaceId), `Couldn’t dismiss ${r.handle}`)}
                    >
                      Dismiss
                    </Button>
                  </span>
                </li>
              ))}
            </ul>
          </div>
        </Callout>
      ) : null}

      {notice ? (
        <Callout tone={notice.tone} className="type-hint" role={notice.tone === 'error' ? 'alert' : 'status'} data-testid="apps-box-notice">
          {notice.text}
        </Callout>
      ) : null}

      <SettingsGroup
        card
        title="Apps on this server"
        hint="Each App and the agent that keeps it up. What an agent may do is set on its card."
        testId="apps-box-group"
        state={
          load.kind === 'loading' ? (
            <SettingsLoadState kind="loading" title="Loading Apps…" />
          ) : load.kind === 'error' ? (
            <SettingsLoadState
              kind="error"
              title={`${PRODUCT_NAME} couldn’t list the Apps on this server`}
              detail={load.text}
              action={<Button onClick={() => void refresh()}>Try again</Button>}
            >
              Nothing changed. The list comes back when the server answers.
            </SettingsLoadState>
          ) : load.apps.length === 0 ? (
            <SettingsLoadState kind="empty" title="No Apps on this server yet" testId="apps-box-empty">
              An agent you allow builds one and installs it from its workspace.
            </SettingsLoadState>
          ) : undefined
        }
      >
        {load.kind === 'ready'
          ? load.apps.map((app) => {
              const wsId = app.maintainer.workspaceId
              const project = wsId ? byId.get(wsId) : undefined
              const owner = needsOwner(app)
              const recent = handovers.get(app.appId)
              const st = app.parked ? { tone: 'idle' as const, words: 'Parked' } : appStateWords(app.state)
              const handLabel = app.parked ? 'Unpark…' : 'Hand over…'
              const handOver = (): void => setSheet(app.appId)
              const menu: OverflowMenuItem[] = []
              if (!owner) {
                menu.push({
                  id: 'handover',
                  label: 'Hand over…',
                  disabled: !isOwner || busy !== null,
                  hint: isOwner ? 'Give it to another agent' : 'Only the server’s owner hands an App over',
                  onSelect: handOver,
                })
              }
              if (owner && !app.parked) {
                menu.push({
                  id: 'park',
                  label: 'Park',
                  hint: 'Stop it and hold it until you hand it on',
                  disabled: busy !== null,
                  onSelect: () => void act(`park:${app.appId}`, `${app.appId} is parked.`, () => parkApp(app.appId), `Couldn’t park ${app.appId}`),
                })
              }
              if (owner) {
                menu.push({
                  id: 'uninstall',
                  label: 'Uninstall…',
                  danger: true,
                  group: menu.length > 0,
                  disabled: busy !== null,
                  onSelect: () => {
                    setLoss(null)
                    setConfirmUninstall(app.appId)
                  },
                })
              }
              return (
                <div key={app.appId} className={`px-4 py-3 ${SETTING_LIST_ROW_HOVER}`} data-testid={`apps-row-${app.appId}`} data-state={app.state}>
                  <div className="flex items-start gap-3">
                    {project ? (
                      <span className={AVATAR_ROUND}>
                        <ProjectAvatar
                          projectPath={project.path}
                          projectName={project.name}
                          projectColor={project.color}
                          projectId={project.id}
                          iconUrl={project.iconUrl}
                          size={24}
                        />
                      </span>
                    ) : (
                      <AppMonogram name={app.appId} />
                    )}
                    <div className="flex-1 min-w-0">
                      <div className="type-name text-[var(--color-text-primary)]">{app.appId}</div>
                      <p className={`${SETTING_ROW_HINT} flex flex-wrap items-center gap-x-1.5`}>
                        <span className="inline-flex items-center gap-1.5" data-testid={`apps-row-${app.appId}-state`}>
                          <StatusDot tone={st.tone} />
                          {st.words}
                        </span>
                        <span data-testid={`apps-row-${app.appId}-agent`}>
                          ·{' '}
                          {app.maintainer.kind === 'agent'
                            ? `kept up by ${app.maintainer.agent ?? app.maintainer.handle ?? 'an agent'}`
                            : app.parked
                              ? 'no agent: you hold it until you hand it on'
                              : 'no agent: its workspace was removed'}
                        </span>
                      </p>
                      <p className={SETTING_ROW_HINT} data-testid={`apps-row-${app.appId}-meta`}>
                        {app.hostnames.length ? app.hostnames.join(', ') : 'No public address'} · {memoryWords(app.memoryLimitBytes).replace(/^Up to/, 'up to').replace(/^No memory/, 'no memory')}
                      </p>
                      {recent && isStaff ? (
                        <div className="mt-2 flex items-center justify-between gap-3 flex-wrap" data-testid={`apps-row-${app.appId}-handover`}>
                          <span className={SETTING_ROW_HINT}>
                            Handed over from {recent.fromHandle ?? 'the owner'} to {recent.toHandle}
                            {recent.rekeyState === 'failed' ? ' · its keys didn’t change yet' : ''}. You can undo it for 72 hours.
                          </span>
                          {isOwner ? (
                            <span className="flex items-center gap-2">
                              <Button
                                disabled={busy !== null}
                                onClick={() => void act(`undo:${app.appId}`, `${app.appId} went back to ${recent.fromHandle ?? 'the owner'}.`, () => undo(app.appId), `Couldn’t undo the handover of ${app.appId}`)}
                              >
                                Undo
                              </Button>
                              <Button
                                variant="ghost"
                                disabled={busy !== null}
                                onClick={() => void act(`finish:${app.appId}`, `${app.appId}: the undo window is closed.`, () => finishHandover(app.appId), `Couldn’t finish the handover of ${app.appId}`)}
                              >
                                Finish
                              </Button>
                            </span>
                          ) : null}
                        </div>
                      ) : null}
                    </div>
                    {isStaff ? (
                      <div className="flex items-center gap-2 flex-shrink-0" data-testid={`apps-row-${app.appId}-actions`}>
                        {owner ? (
                          <Button
                            variant="accent"
                            disabled={!isOwner || busy !== null}
                            title={isOwner ? undefined : 'Only the server’s owner hands an App over'}
                            onClick={handOver}
                          >
                            {handLabel}
                          </Button>
                        ) : wsId ? (
                          <Button onClick={() => openCard(wsId)}>Agent settings</Button>
                        ) : null}
                        {menu.length > 0 ? <OverflowMenu label={`More actions for ${app.appId}`} items={menu} /> : null}
                      </div>
                    ) : wsId ? (
                      <div className="flex-shrink-0">
                        <Button onClick={() => openCard(wsId)}>Agent settings</Button>
                      </div>
                    ) : null}
                  </div>
                </div>
              )
            })
          : null}
      </SettingsGroup>

      {removing ? (
        <AccessSheet
          title={`Uninstall ${removing.appId}?`}
          intro={
            lossFor === removing.appId
              ? undefined
              : 'It stops and leaves this server. Its data stays, so nothing is deleted.'
          }
          onClose={() => {
            setConfirmUninstall(null)
            setLoss(null)
          }}
          busy={busy === `rm:${removing.appId}`}
          testId={`apps-uninstall-${removing.appId}`}
          footer={
            lossFor === removing.appId && loss ? (
              <>
                <Button
                  variant="ghost"
                  size="md"
                  autoFocus
                  disabled={busy !== null}
                  onClick={() => {
                    setLoss(null)
                    setConfirmUninstall(null)
                  }}
                >
                  Keep it
                </Button>
                <Button variant="danger-muted" size="md" disabled={busy !== null} onClick={() => void uninstall(removing.appId, true)}>
                  Uninstall anyway
                </Button>
              </>
            ) : (
              <>
                <Button variant="ghost" size="md" autoFocus disabled={busy !== null} onClick={() => setConfirmUninstall(null)}>
                  Cancel
                </Button>
                <Button
                  variant="danger-muted"
                  size="md"
                  disabled={busy !== null}
                  data-testid="apps-uninstall-confirm"
                  onClick={() => void uninstall(removing.appId, false)}
                >
                  {busy === `rm:${removing.appId}` ? 'Uninstalling…' : 'Uninstall'}
                </Button>
              </>
            )
          }
        >
          {lossFor === removing.appId && loss ? (
            <div data-testid={`apps-row-${removing.appId}-loss`} className="space-y-2">
              <p className={SETTING_ROW_LABEL}>People would lose access. Uninstalling {removing.appId} takes away:</p>
              <ul className="list-disc pl-5 type-hint text-[var(--color-text-primary)] space-y-0.5">
                {loss.parts.map((p) => (
                  <li key={p}>{p}</li>
                ))}
              </ul>
              <p className={SETTING_ROW_HINT}>Reinstalling makes a new App that gets none of this back.</p>
            </div>
          ) : (
            <p className={SETTING_ROW_HINT}>
              {removing.maintainer.kind === 'agent' ? `${removing.maintainer.agent ?? 'Its agent'} can install it again later.` : 'You can install it again later from a workspace.'}
            </p>
          )}
        </AccessSheet>
      ) : null}

      {sheetApp && isOwner ? (
        <AppHandoverSheet
          appId={sheetApp.appId}
          parked={sheetApp.parked}
          currentWorkspace={sheetApp.maintainer.workspaceId ?? null}
          agents={agents}
          onClose={() => setSheet(null)}
          onDone={async (text) => {
            setSheet(null)
            setNotice({ tone: 'ok', text })
            await refresh()
          }}
        />
      ) : null}
    </div>
  )
}

/** "Up to 512 MB" / "No memory limit", in decimal units as the rest of the page. */
export function memoryWords(bytes: number | null): string {
  if (bytes === null || bytes <= 0) return 'No memory limit'
  const mb = bytes / 1024 ** 2
  if (mb >= 1024) {
    const gb = mb / 1024
    return `Up to ${Number.isInteger(gb) ? gb : gb.toFixed(1)} GB`
  }
  return `Up to ${Math.round(mb)} MB`
}

/** On an App's own page: who keeps it up, what that agent may do, its
 *  memory and its database (the details the list leaves out). Reads the box
 *  view and shows the App whose id is `appId`; nothing when there is none. */
export function AppAboutCard({ appId }: { appId: string }): React.JSX.Element | null {
  const projects = useProjectsStore((s) => s.projects)
  const [app, setApp] = useState<BoxApp | null | 'loading'>('loading')
  useEffect(() => {
    let live = true
    fetchBoxApps().then(
      (apps) => live && setApp(apps.find((a) => a.appId === appId) ?? null),
      () => live && setApp(null),
    )
    return () => {
      live = false
    }
  }, [appId])
  if (app === 'loading' || app === null) return null
  const wsId = app.maintainer.workspaceId
  const project = wsId ? projects.find((p) => p.id === wsId) : undefined
  const chip = postgresChip(app.database)
  return (
    <SettingsGroup card title="About this App" testId="apps-about">
      <SettingRow
        label="Kept up by"
        hint={
          app.maintainer.kind === 'agent'
            ? (app.maintainer.agent ?? app.maintainer.handle ?? 'An agent')
            : app.parked
              ? 'No agent: you hold it until you hand it on.'
              : 'No agent: its workspace was removed.'
        }
      >
        {wsId ? <Button onClick={() => openCard(wsId)}>Agent settings</Button> : null}
      </SettingRow>
      {app.maintainer.kind === 'agent' ? (
        <SettingRow label="What its agent may do" hint={<span data-testid="apps-about-access">{accessLine(app, project)}</span>}>
          {null}
        </SettingRow>
      ) : null}
      <SettingRow label="Memory" hint={memoryWords(app.memoryLimitBytes)}>
        {null}
      </SettingRow>
      {chip ? (
        <SettingRow label="Database" hint={<span data-testid="apps-about-postgres">{chip}</span>}>
          {null}
        </SettingRow>
      ) : null}
    </SettingsGroup>
  )
}

import { useCallback, useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useRoom } from '@/components/Room/RoomContext'
import { onAppHello, onPublishServicesChanged } from '@/stores/session-events'
import { useSettingsStore } from '@/stores/settings'
import { openSite } from '@/lib/published-sites'
import { scopedKey } from '@/kessel/server-scope'
import { AppsIcon } from '@/components/icons/K2Icons'
import { SectionManageCog } from './SectionManageCog'
import { DRAWER_HEADER_GLYPH_PX } from './drawer-glyph'
import { PublishedServiceDetailsModal } from './PublishedServiceDetailsModal'
import { parsePublishList, publishedHostLabel, type PublishedService } from './urls-ports'
import {
  APP_DEPLOY_EXAMPLE,
  appAddresses,
  appListErrorMessage,
  appStateView,
  parseWorkspaceApps,
  type AppStateTone,
  type WorkspaceApp,
} from './workspace-apps'

// Apps: a collapsible Workspace-drawer section listing the Apps this
// workspace's agent maintains (Rosson 2026-11-15: "now that agents oversee
// apps"). Daemon-first: `GET /cli/app/list?workspace=` filters on the
// App's owner_workspace; this only renders the answer. Each row: the
// state as a dot and a word, the address (the k2 host, then "+N" custom
// domains with the full list in the tooltip) which opens the App the way
// Published opens a site, and Details: Published's own details sheet for
// the App's service row (status, address, deploy history and Roll back).

const DETAILS_BTN =
  'px-2 py-0.5 text-[9px] font-medium text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 transition-opacity no-drag cursor-pointer flex-shrink-0'

/** Published's dot sizes; the in-progress pulse is the app's working dot
 *  (the phone toolbar's), still under reduced motion. */
const DOT: Record<AppStateTone, string> = {
  ok: 'bg-[var(--color-status-ok)]',
  working: 'bg-[var(--color-status-working)] animate-pulse motion-reduce:animate-none',
  danger: 'bg-[var(--color-status-error)]',
  quiet: 'bg-[var(--color-text-muted)]',
}
const WORD: Record<AppStateTone, string> = {
  ok: 'text-[var(--color-text-muted)]',
  working: 'text-[var(--color-text-muted)]',
  danger: 'text-[var(--color-status-error-text)]',
  quiet: 'text-[var(--color-text-muted)]',
}

/** Until the daemon sends publish_services_changed on its own App state
 *  changes (crash, restart, park), re-read on the heartbeat badge's 30 s.
 *  Drop this poll once that event covers state changes. */
const POLL_MS = 30_000

export function AppsSection({ projectId }: { projectId: string }): React.JSX.Element {
  const room = useRoom()
  const scope = room.scope

  const collapseKey = projectId ? scopedKey(scope, `apps.section-collapsed.${projectId}`) : null
  const [open, setOpen] = useState<boolean>(() => {
    if (!collapseKey) return true
    return localStorage.getItem(collapseKey) !== 'closed'
  })
  useEffect(() => {
    setOpen(collapseKey ? localStorage.getItem(collapseKey) !== 'closed' : true)
  }, [collapseKey])
  const toggle = useCallback((): void => {
    setOpen((cur) => {
      const next = !cur
      if (collapseKey) localStorage.setItem(collapseKey, next ? 'open' : 'closed')
      return next
    })
  }, [collapseKey])

  const [apps, setApps] = useState<WorkspaceApp[] | undefined>(undefined)
  const [listError, setListError] = useState<string | null>(null)
  const [details, setDetails] = useState<PublishedService | null>(null)
  const hostLabel = publishedHostLabel(scope.connectHost() ?? 'local')

  useEffect(() => {
    setDetails(null)
    if (!projectId) {
      setApps([])
      setListError(null)
      return
    }
    let cancelled = false
    setApps(undefined)
    const refresh = async (): Promise<void> => {
      try {
        const raw = await daemonCliGet<unknown>(scope, 'app/list', { workspace: projectId })
        if (!cancelled) {
          setApps(parseWorkspaceApps(raw))
          setListError(null)
        }
      } catch (err) {
        if (!cancelled) {
          // Keep rows we already have; a failure is never "no Apps".
          setApps((prev) => prev ?? [])
          setListError(appListErrorMessage(err))
        }
      }
    }
    void refresh()
    const offHello = onAppHello(scope, () => {
      void refresh()
    })
    const offChanged = onPublishServicesChanged(scope, (e) => {
      if (e.projectId && e.projectId !== projectId) return
      void refresh()
    })
    const poll = setInterval(() => void refresh(), POLL_MS)
    return () => {
      cancelled = true
      offHello()
      offChanged()
      clearInterval(poll)
    }
  }, [projectId])

  // Details is Published's sheet for the App's service row (its name is
  // the App id). An App with no service row yet opens Settings → Apps.
  const openDetails = useCallback(
    async (appId: string): Promise<void> => {
      try {
        const raw = await daemonCliGet<unknown>(scope, 'publish/list', { project: projectId })
        const svc = parsePublishList(raw).find((s) => s.name.toLowerCase() === appId.toLowerCase())
        if (svc) {
          setDetails(svc)
          return
        }
      } catch {
        /* fall through to Settings → Apps */
      }
      if (room.isPrimary) useSettingsStore.getState().openSettings('skin-access')
    },
    [projectId, room.isPrimary],
  )

  const list = apps ?? []
  const count = list.length

  return (
    <>
      {/* ── Header (the drawer's collapsible idiom) ── */}
      <button
        onClick={toggle}
        data-testid="apps-section-header"
        className="w-full flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] cursor-pointer no-drag hover:bg-[var(--color-wash-1)] transition-colors"
        title={open ? 'Collapse Apps' : 'Expand Apps'}
      >
        <span className="flex items-center gap-1.5 text-[11px] text-[var(--color-text-secondary)]">
          <svg
            className={`w-2 h-2 text-[var(--color-text-muted)] transition-transform ${open ? 'rotate-90' : ''}`}
            viewBox="0 0 8 8"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden
          >
            <path d="M2 1 L6 4 L2 7" />
          </svg>
          <AppsIcon size={DRAWER_HEADER_GLYPH_PX} className="text-[var(--color-accent)] flex-shrink-0" />
          Apps
          {count > 0 && (
            <span className="text-[9px] tabular-nums font-medium px-1 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]">
              {count}
            </span>
          )}
        </span>
        {/* Settings → Apps is the window's server's (as Connected Agents' cog). */}
        {room.isPrimary && (
          <SectionManageCog
            title="Manage Apps"
            onClick={() => useSettingsStore.getState().openSettings('skin-access')}
          />
        )}
      </button>

      {open && (
        <div className="px-3 py-2 border-b border-[var(--color-border)]" data-testid="apps-section-body">
          {listError ? (
            <p
              className={`text-[10px] text-[var(--color-status-error-text)] ${count ? 'mb-1.5' : ''}`}
              data-apps-list-error=""
            >
              {listError}
            </p>
          ) : null}
          {apps === undefined ? (
            <p className="text-[10px] text-[var(--color-text-muted)]">Checking Apps…</p>
          ) : count === 0 ? (
            listError ? null : (
              <p className="text-[10px] text-[var(--color-text-muted)]" data-testid="apps-empty">
                Ask your agent to build one and deploy it with{' '}
                <span className="font-mono">{APP_DEPLOY_EXAMPLE}</span>
              </p>
            )
          ) : (
            <div className="space-y-1.5">
              {list.map((app) => (
                <AppRow
                  key={app.appId}
                  app={app}
                  onOpen={(url) => void openSite(scope, url)}
                  onDetails={() => void openDetails(app.appId)}
                />
              ))}
            </div>
          )}
        </div>
      )}

      {details ? (
        <PublishedServiceDetailsModal
          service={details}
          hostLabel={hostLabel}
          scope={scope}
          onClose={() => setDetails(null)}
        />
      ) : null}
    </>
  )
}

function AppRow({
  app,
  onOpen,
  onDetails,
}: {
  app: WorkspaceApp
  onOpen: (url: string) => void
  onDetails: () => void
}): React.JSX.Element {
  const { tone, word } = appStateView(app.state)
  const addr = appAddresses(app)
  const everyAddress = addr.all.join('\n')
  return (
    <div className="min-w-0" data-app-row={app.appId} data-app-state={app.state}>
      <div className="flex items-center gap-1.5 min-w-0">
        <span
          aria-hidden
          data-app-dot={tone}
          className={`w-1.5 h-1.5 rounded-[var(--radius-badge)] flex-shrink-0 ${DOT[tone]}`}
        />
        <span className="text-[11px] text-[var(--color-text-primary)] truncate flex-1" title={app.appId}>
          {app.appId}
        </span>
        <button
          type="button"
          data-k2-focus=""
          data-testid="app-details"
          aria-label={`Details for ${app.appId}`}
          onClick={onDetails}
          className={`${DETAILS_BTN} outline-none`}
        >
          Details
        </button>
      </div>
      <div className="flex items-center gap-1 min-w-0 text-[10px] text-[var(--color-text-muted)]">
        <span
          className={`flex-shrink-0 ${WORD[tone]}`}
          data-app-state-word=""
          title={app.stateReason ?? word}
        >
          {word}
        </span>
        {app.label ? (
          <>
            <span aria-hidden className="flex-shrink-0">·</span>
            <span className="truncate min-w-0" title={app.label}>
              {app.label}
            </span>
          </>
        ) : null}
      </div>
      {addr.first && addr.openUrl ? (
        <div className="flex items-center gap-1 min-w-0">
          <button
            type="button"
            data-k2-focus=""
            data-testid="app-open"
            aria-label={`Open ${app.appId} at ${addr.first}`}
            title={everyAddress}
            onClick={() => onOpen(addr.openUrl as string)}
            className="min-w-0 truncate text-left px-1 -mx-1 text-[11px] font-mono text-[var(--color-accent)] hover:underline no-drag cursor-pointer outline-none"
          >
            {addr.first}
          </button>
          {addr.more > 0 ? (
            <span
              className="flex-shrink-0 text-[9px] tabular-nums font-medium px-1 py-0.5 bg-[var(--color-wash-1)] text-[var(--color-text-muted)]"
              title={everyAddress}
              aria-label={`${addr.more} more ${addr.more === 1 ? 'address' : 'addresses'}: ${addr.all.slice(1).join(', ')}`}
              data-testid="app-more-hosts"
            >
              +{addr.more}
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}

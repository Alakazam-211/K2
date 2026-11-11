import { useMemo } from 'react'
import { primaryScope } from '@/kessel/server-scope'
import { useProjectsStore } from '@/stores/projects'
import { useConnectHostStore } from '@/stores/connect-host'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useTunnelUrls, type TunnelStatus } from '@/hooks/useTunnelUrls'
import { publishedSubdomains } from '@/lib/published-subdomains'
import { SiteVisibilityControls } from '@/components/WorkspacePanel/SiteVisibilityControls'
import {
  AddPublishedSubdomainForm,
  PublishedSubdomainRowActions,
} from './PublishedSubdomainControls'
import {
  nestedPublicUrl,
  sortedTargets,
} from '@/components/WorkspacePanel/urls-ports'
import { SettingRow } from '../controls/SettingControls'

// Addresses — the SERVER-WIDE tunnel view inside Settings → Tunnel, as rows
// of the Addresses card: this server's address (with its local port and
// relay), then every site published under it with the workspace it
// belongs to (0074 attribution). This is the
// generic surface the workspace drawer deliberately is NOT (the drawer
// shows only the active workspace's attributed URLs); state comes from
// the same shared `useTunnelUrls` hook so both can never disagree.
// Workspace names resolve through the projects store when the id is
// registered here; a foreign/dangling id renders as the raw id — honest,
// never fabricated.
//
// 0.45.7: the local server's owner or an admin also gets **Add published
// subdomain** and per-row **Change target** / **Remove** (the desktop twin
// of `k2 publish subdomain create | point | rm`; calls in
// lib/published-subdomains.ts). `canManage` comes from the Connect section
// (local host + whoami role); the panel re-checks the window's host so a
// remote server never shows them.

const LINK = 'relative px-0.5 -mx-0.5 font-mono text-[var(--color-accent-text)] hover:underline truncate no-drag cursor-pointer'

/** Rows for the Addresses card (Settings → Tunnel). Render inside a
 *  `SettingsGroup card`: each child is one row of the card.
 *
 *  `serverStatus` is the status the page's Status card shows (this
 *  computer's tunnel, polled and refreshed on every Turn on / Forget).
 *  When given, "This server" reads it, so the two always agree, and the
 *  sites re-read whenever it changes; without it (a remote server) the
 *  panel reads its own. */
export function TunnelUrlsPanel({
  canManage = false,
  serverStatus,
}: { canManage?: boolean; serverStatus?: TunnelStatus | null } = {}): React.JSX.Element {
  const scope = primaryScope()
  const refreshKey = serverStatus ? `${serverStatus.running}|${serverStatus.public_url ?? ''}|${serverStatus.subdomain ?? ''}` : undefined
  const own = useTunnelUrls(scope, refreshKey)
  const status = serverStatus !== undefined ? serverStatus : own.status
  const subs = own.subs
  const projects = useProjectsStore((s) => s.projects)
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const confirm = useConfirmDialogStore((s) => s.confirm)
  const manage = canManage && activeHost === 'local'
  const api = useMemo(() => publishedSubdomains(scope), [scope])

  const running = status?.running ?? false
  const publicUrl = status?.public_url ?? null
  const targets = subs && typeof subs === 'object' ? subs.targets : {}
  const primary = subs && typeof subs === 'object' ? subs.primary : ''
  const rows = sortedTargets(targets)

  const workspaceName = (projectId: string | null): string | null => {
    if (!projectId) return null
    return projects.find((p) => p.id === projectId)?.name ?? projectId
  }

  const serverDetails = [
    typeof status?.local_port === 'number' ? `local port ${status.local_port}` : null,
    status?.server_addr ? `relay ${status.server_addr}` : null,
  ].filter(Boolean)

  return (
    <>
      <SettingRow
        label="This server"
        testId="tunnel-address-server"
        hint={
          status === null ? (
            'Checking the tunnel…'
          ) : !running ? (
            'No address while the tunnel is off.'
          ) : (
            <>
              {publicUrl ? (
                <a href={publicUrl} target="_blank" rel="noreferrer" data-k2-focus="" className={LINK}>
                  {publicUrl}
                </a>
              ) : (
                'The relay gives this server its address.'
              )}
              {serverDetails.length > 0 && <> · {serverDetails.join(' · ')}</>}
            </>
          )
        }
      >
        {null}
      </SettingRow>

      {subs === undefined ? (
        <SettingRow label="Sites" hint="Loading the sites this server publishes…">
          {null}
        </SettingRow>
      ) : subs === null ? (
        <SettingRow label="Sites" hint="This server's version of K2 can't list its published sites. Update K2 there to see them.">
          {null}
        </SettingRow>
      ) : rows.length === 0 ? (
        <SettingRow
          label="Sites"
          hint={
            manage
              ? 'No sites yet. A site shares a port on this computer at its own address, like docs.' + (primary || 'you') + '.k2.dev.'
              : 'No sites published yet.'
          }
        >
          {null}
        </SettingRow>
      ) : (
        rows.map(([label, info]) => {
          const url = nestedPublicUrl(label, primary, publicUrl)
          const ws = workspaceName(info.projectId)
          return (
            <SettingRow
              key={label}
              testId="tunnel-address-site"
              label={
                url ? (
                  <a href={url} target="_blank" rel="noreferrer" data-k2-focus="" className={LINK}>
                    {url}
                  </a>
                ) : (
                  <span className="font-mono">{label}</span>
                )
              }
              hint={
                <>
                  Shows <span className="font-mono">{info.target}</span>
                  {' · '}
                  <span title={info.projectId ?? undefined}>{ws ? `${ws} workspace` : 'No workspace'}</span>
                </>
              }
              below={manage ? <SiteVisibilityControls scope={scope} label={label} settings /> : undefined}
            >
              {manage && (
                <PublishedSubdomainRowActions
                  api={api}
                  label={label}
                  target={info.target}
                  projectId={info.projectId}
                  host={(url ?? label).replace(/^https:\/\//, '')}
                  confirmRemove={confirm}
                />
              )}
            </SettingRow>
          )
        })
      )}

      {manage && subs !== null && subs !== undefined && (
        <div className="px-4 py-3">
          <AddPublishedSubdomainForm
            api={api}
            primary={primary || status?.subdomain || ''}
            publicUrl={publicUrl}
            workspaces={projects}
          />
        </div>
      )}
    </>
  )
}

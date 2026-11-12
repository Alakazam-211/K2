// Settings → Access → Remote Desktop (Beta; prd-remote-desktop-tab-v1 §5,
// §8): a front end for `/cli/desktop/*` on the window's server. The
// daemon owns every rule: the switch is an Owner's (on until an Owner turns
// it off), Owners and Admins open the desktop, a Member only with a grant
// and only to watch. This page keeps no logic of its own: it shows what
// `desktop/status` says and sends the Owner's gestures.
//
// Owner: the switch, the hide-list, grants (add a Member to watch; revoke),
// viewers (Kick), the access log (last 50). Everyone else: the state, read
// only.

import { useCallback, useEffect, useState } from 'react'
import { Button, Input, Toggle, Textarea } from '@/components/ui'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { onDesktopState } from '@/stores/session-events'
import { useToastStore } from '@/stores/toast'
import {
  disableDesktop,
  enableDesktop,
  fetchDesktopAudit,
  fetchDesktopStatus,
  grantDesktop,
  kickDesktopViewer,
  revokeDesktop,
  setDesktopHideApps,
  type DesktopAuditLine,
} from '@/components/Desktop/desktop-api'
import {
  DESKTOP_ENABLE_CARD,
  DESKTOP_FEATURE,
  VIRTUAL_DESKTOP_FEATURE,
  VIRTUAL_ENABLE_CARD,
  type DesktopStatus,
} from '@/components/Desktop/desktop-state'
import type { SettingEntry } from '../searchManifest'

export const REMOTE_DESKTOP_MANIFEST: SettingEntry[] = [
  {
    id: 'remote-desktop.switch',
    section: 'remote-desktop',
    label: 'Remote Desktop',
    description: "See and drive this computer's screen from another K2 (people only)",
    keywords: ['remote desktop', 'screen', 'screen sharing', 'vnc', 'control', 'desktop tab', 'k2 screen'],
  },
  {
    id: 'remote-desktop.grants',
    section: 'remote-desktop',
    label: 'Remote Desktop grants',
    description: 'Let a Member watch this screen',
    keywords: ['grant', 'member', 'watch', 'view only'],
  },
]

/** Is the page shown for the window's server? */
export function remoteDesktopAvailable(scope: ServerScope): boolean {
  return scope.serverSupports(DESKTOP_FEATURE)
}

function errText(err: unknown): string {
  return err instanceof Error ? err.message : String(err)
}

export function RemoteDesktopSection({ scope = primaryScope() }: { scope?: ServerScope }): React.JSX.Element {
  const [status, setStatus] = useState<DesktopStatus | null>(null)
  const [audit, setAudit] = useState<DesktopAuditLine[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [confirmOn, setConfirmOn] = useState(false)
  const [grantUser, setGrantUser] = useState('')
  const [hideDraft, setHideDraft] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const s = await fetchDesktopStatus(scope)
      setStatus(s)
      setError(null)
      if (s.you.mayManage) setAudit(await fetchDesktopAudit(scope, 50))
    } catch (err) {
      setError(errText(err))
    }
  }, [scope])

  const supported = remoteDesktopAvailable(scope)
  useEffect(() => {
    // An older server has no routes: never ask it.
    if (!supported) return
    void load()
    return onDesktopState(scope, () => void load())
  }, [scope, load, supported])

  const act = async (what: string, fn: () => Promise<unknown>): Promise<void> => {
    setBusy(true)
    try {
      await fn()
    } catch (err) {
      useToastStore.getState().addToast(`${what}: ${errText(err)}`, 'error')
    } finally {
      setBusy(false)
      await load()
    }
  }

  if (!supported) {
    return (
      <div className="max-w-2xl" data-testid="remote-desktop-unsupported">
        <h2 className="type-title text-[var(--color-text-primary)] mb-1">Remote Desktop</h2>
        <p className="type-row text-[var(--color-text-secondary)]">Update K2 on this computer to use Remote desktop.</p>
      </div>
    )
  }

  const manage = !!status?.you.mayManage
  // A Linux server with no screen: This server is its `virtual` desktop,
  // and the switch turns on every virtual desktop there (§4 item 2), with
  // its own card (LD-18).
  const virtual = scope.serverSupports(VIRTUAL_DESKTOP_FEATURE) && !!status?.desktop.startsWith('virtual')
  const hideApps = hideDraft ?? (status?.hideApps ?? []).join('\n')
  return (
    <div className="max-w-2xl" data-settings-id="remote-desktop.switch" data-testid="remote-desktop-section">
      <h2 className="type-title text-[var(--color-text-primary)] mb-1">Remote Desktop</h2>
      <p className="type-hint text-[var(--color-text-muted)] mb-4">
        A Desktop tab shows this computer’s screen, live, and lets an allowed person drive it. People only: agents can’t use
        it. Owners and Admins can open it; a Member only with a grant, and only to watch.
      </p>
      {error ? (
        <p className="type-hint text-[var(--color-status-error)] mb-2" role="alert">
          {error}
        </p>
      ) : null}
      {status === null ? (
        <p className="type-hint text-[var(--color-text-muted)]">Loading…</p>
      ) : (
        <>
          <div className="flex items-center justify-between gap-3 border border-[var(--color-border)] px-3 py-2 mb-2">
            <div className="min-w-0">
              <div className="type-row text-[var(--color-text-primary)]">{status.machine}</div>
              <div className="type-hint text-[var(--color-text-muted)]" data-testid="remote-desktop-state">
                {status.enabled ? 'On' : 'Off'} · {status.message || status.state}
              </div>
            </div>
            {manage ? (
              <Toggle
                checked={status.enabled}
                disabled={busy}
                aria-label="Remote desktop"
                data-testid="remote-desktop-switch"
                onChange={(next) => {
                  if (next) setConfirmOn(true)
                  else void act('Couldn’t turn off Remote desktop', () => disableDesktop(scope))
                }}
              />
            ) : (
              <span className="type-hint text-[var(--color-text-muted)]" data-testid="remote-desktop-readonly">
                Only an Owner can change this.
              </span>
            )}
          </div>
          {confirmOn ? (
            <div className="border border-[var(--color-border)] p-3 mb-4" data-testid="remote-desktop-enable-card">
              <p className="type-hint text-[var(--color-text-secondary)] mb-2">{virtual ? VIRTUAL_ENABLE_CARD : DESKTOP_ENABLE_CARD}</p>
              <div className="flex gap-2">
                <Button
                  variant="accent"
                  disabled={busy}
                  data-testid="remote-desktop-enable-confirm"
                  onClick={() => {
                    setConfirmOn(false)
                    void act('Couldn’t turn on Remote desktop', () => enableDesktop(scope, virtual ? 'virtual' : undefined))
                  }}
                >
                  Turn on
                </Button>
                <Button variant="ghost" onClick={() => setConfirmOn(false)}>
                  Cancel
                </Button>
              </div>
            </div>
          ) : null}

          {manage ? (
            <>
              <h3 className="type-heading text-[var(--color-text-primary)] mt-4 mb-1">Hidden apps</h3>
              <p className="type-hint text-[var(--color-text-muted)] mb-2">
                Their windows never show in the picture (macOS). One bundle id a line.
              </p>
              <Textarea
                className="font-mono"
                rows={4}
                aria-label="Hidden apps"
                data-testid="remote-desktop-hide-apps"
                value={hideApps}
                onChange={(e) => setHideDraft(e.target.value)}
              />
              <div className="mt-1 mb-4">
                <Button
                  variant="ghost"
                  disabled={busy || hideDraft === null}
                  data-testid="remote-desktop-hide-save"
                  onClick={() => {
                    const list = hideApps
                      .split('\n')
                      .map((l) => l.trim())
                      .filter(Boolean)
                    setHideDraft(null)
                    void act('Couldn’t save hidden apps', () => setDesktopHideApps(scope, list))
                  }}
                >
                  Save
                </Button>
              </div>

              <h3 className="type-heading text-[var(--color-text-primary)] mb-1" data-settings-id="remote-desktop.grants">
                Grants
              </h3>
              <p className="type-hint text-[var(--color-text-muted)] mb-2">
                A Member you grant can watch this screen, never drive it. Owners and Admins need no grant.
              </p>
              <div className="flex gap-2 mb-2">
                <Input
                  value={grantUser}
                  placeholder="Member's username"
                  aria-label="Member's username"
                  data-testid="remote-desktop-grant-user"
                  onChange={(e) => setGrantUser(e.target.value)}
                />
                <Button
                  variant="ghost"
                  disabled={busy || !grantUser.trim()}
                  data-testid="remote-desktop-grant-add"
                  onClick={() => {
                    const user = grantUser.trim()
                    setGrantUser('')
                    void act(`Couldn’t grant ${user}`, () => grantDesktop(scope, user))
                  }}
                >
                  Let watch
                </Button>
              </div>
              <ul className="border border-[var(--color-border)] divide-y divide-[var(--color-border)] mb-4" data-testid="remote-desktop-grants">
                {(status.grants ?? []).length === 0 ? (
                  <li className="px-3 py-2 type-hint text-[var(--color-text-muted)]">No grants.</li>
                ) : (
                  (status.grants ?? []).map((g) => (
                    <li key={g.userId} className="flex items-center justify-between gap-3 px-3 py-2" data-testid="remote-desktop-grant">
                      <span className="type-row text-[var(--color-text-primary)]">
                        {g.user} <span className="type-hint text-[var(--color-text-muted)]">· {g.mode === 'view' ? 'watch' : 'control'} · by {g.grantedBy}</span>
                      </span>
                      <Button variant="ghost" disabled={busy} onClick={() => void act(`Couldn’t revoke ${g.user}`, () => revokeDesktop(scope, g.user))}>
                        Revoke
                      </Button>
                    </li>
                  ))
                )}
              </ul>

              <h3 className="type-heading text-[var(--color-text-primary)] mb-1">Viewers</h3>
              <ul className="border border-[var(--color-border)] divide-y divide-[var(--color-border)] mb-4" data-testid="remote-desktop-viewers">
                {(status.viewers ?? []).length === 0 ? (
                  <li className="px-3 py-2 type-hint text-[var(--color-text-muted)]">Nobody is watching.</li>
                ) : (
                  (status.viewers ?? []).map((v) => (
                    <li key={v.id} className="flex items-center justify-between gap-3 px-3 py-2" data-testid="remote-desktop-viewer">
                      <span className="type-row text-[var(--color-text-primary)]">
                        {v.user}{' '}
                        <span className="type-hint text-[var(--color-text-muted)]">
                          · {v.mode === 'control' ? 'controlling' : 'watching'} · {v.device || v.ingress}
                        </span>
                      </span>
                      <Button variant="ghost" disabled={busy} onClick={() => void act(`Couldn’t disconnect ${v.user}`, () => kickDesktopViewer(scope, v.id))}>
                        Kick
                      </Button>
                    </li>
                  ))
                )}
              </ul>

              <h3 className="type-heading text-[var(--color-text-primary)] mb-1">Access log</h3>
              <ul className="border border-[var(--color-border)] divide-y divide-[var(--color-border)]" data-testid="remote-desktop-audit">
                {audit === null || audit.length === 0 ? (
                  <li className="px-3 py-2 type-hint text-[var(--color-text-muted)]">Nothing yet.</li>
                ) : (
                  audit.map((l, i) => (
                    <li key={`${l.ts}-${i}`} className="px-3 py-1.5 type-hint text-[var(--color-text-secondary)]">
                      {l.ts} · {l.event} · {l.who}
                      {l.target ? ` → ${l.target}` : ''}
                      {l.outcome ? ` · ${l.outcome}` : ''}
                    </li>
                  ))
                )}
              </ul>
            </>
          ) : null}
        </>
      )}
    </div>
  )
}

export default RemoteDesktopSection

// Settings → Apps → <App> → Releases (Rosson 10-29, 0.46.7): the App's
// update channels and current versions; every version's files with download
// links and sizes; downloads (the download host's numbers) and active users
// (update checks) per version; promote, roll back and pull. Not a GitHub
// clone: pushes come from `k2 app updates push`. Thin: the daemon owns the
// rules and answers in words (app-updates-api.ts).

import React, { useCallback, useEffect, useState } from 'react'
import { Button } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import { SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { ConfirmSheet } from './AccessDialogs'
import { fetchDownloads, fetchUpdates, promote, pull, rollback, size, type Downloads, type Release, type Updates } from './app-updates-api'

const muted = 'type-hint text-[var(--color-text-secondary)] leading-relaxed'
const secondary = 'type-hint text-[var(--color-text-secondary)]'

function errCode(e: unknown): string | null {
  const c = (e as { code?: unknown } | null)?.code
  return typeof c === 'string' ? c : null
}

export function AppReleases({ app }: { app: string }): React.JSX.Element | null {
  const [u, setU] = useState<Updates | null>(null)
  const [dl, setDl] = useState<Downloads | null>(null)
  const [hidden, setHidden] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [asking, setAsking] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      setU(await fetchUpdates(app))
    } catch (e) {
      // Not a runtime App (a --cmd or --skin service): no Releases panel.
      if (errCode(e) === 'app_not_found' || errCode(e) === 'apps_off') setHidden(true)
      else setError(daemonErrorText(e))
      return
    }
    try {
      setDl(await fetchDownloads(app))
    } catch (e) {
      setDl({ available: false, why: daemonErrorText(e) })
    }
  }, [app])

  useEffect(() => {
    void load()
  }, [load])

  const act = async (f: () => Promise<string | void>): Promise<void> => {
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      const msg = await f()
      if (msg) setNotice(msg)
      await load()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
      setAsking(null)
    }
  }

  if (hidden) return null

  const channels = u?.channels ?? []
  // Releases come newest first: a channel can move forward to a release
  // listed above its current one (promote) or back to one below (rollback).
  // The daemon checks the versions; this only offers the sensible buttons.
  const at = (version: string | null): number => (u?.releases ?? []).findIndex((x) => x.version === version)
  const promoteTargets = (r: Release, i: number): string[] =>
    channels.filter((c) => !r.channels.includes(c.channel) && (c.version == null || at(c.version) > i)).map((c) => c.channel)
  const rollbackTargets = (r: Release, i: number): string[] =>
    channels.filter((c) => !r.channels.includes(c.channel) && c.version != null && at(c.version) >= 0 && at(c.version) < i).map((c) => c.channel)
  const from = (r: Release): string | null => r.channels[0] ?? null

  return (
    <SettingsGroup
      card
      title="Desktop releases"
      hint="Versions of this App’s desktop download, and which channel offers each one."
      state={u == null && !error ? <SettingsLoadState kind="loading" title="Loading releases…" /> : undefined}
    >
      <div className="px-4 py-3 space-y-2" data-testid="app-releases">
        {error ? (
          <p role="alert" className="type-hint text-[var(--color-status-error-soft)]">
            That didn&rsquo;t work. <span className="text-[var(--color-text-muted)]">The server said: {error.trim().replace(/[.\s]+$/, '')}.</span>
          </p>
        ) : null}
        {notice ? (
          <p role="status" className={secondary}>
            {notice}
          </p>
        ) : null}
        {u == null ? (
          <p className={muted}>Loading…</p>
        ) : u.releases.length === 0 ? (
          <p className={muted} data-testid="releases-empty">
            No desktop releases yet. This App&rsquo;s agent publishes one when it ships a desktop version
            {u.keyId ? '' : ', after it sets up the App’s update signing key'}.
          </p>
        ) : (
          <>
            <p data-testid="releases-channels" className={secondary}>
              {channels.length
                ? channels.map((c) => `${c.channel} ${c.version ?? '–'}`).join(' · ')
                : 'No channel serves a release right now.'}
            </p>
            <p className={muted}>
              Active users: {u.activeToday} today · {u.activeWeek} on the busiest day this week
              {u.counting === 'checks' ? ' (update checks; one install may check more than once)' : ''}
              {dl == null ? '' : dl.available ? ` · Downloads (30 days): ${dl.total}` : ' · Downloads: unavailable'}
            </p>
            <table className="w-full type-hint">
              <thead>
                <tr className="text-left text-[var(--color-text-muted)]">
                  <th>Version</th>
                  <th>State</th>
                  <th>Channels</th>
                  <th>Size</th>
                  <th>Downloads</th>
                  <th>Users</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {u.releases.map((r, i) => (
                  <React.Fragment key={r.id}>
                    <tr data-testid={`release-${r.version}`} className="text-[var(--color-text-primary)]">
                      <td className="font-mono">{r.version}</td>
                      <td>{r.state === 'yanked' ? 'pulled' : r.state}</td>
                      <td>{r.channels.join(', ') || '–'}</td>
                      <td>{size(r.size)}</td>
                      <td>{dl?.available ? (dl.byVersion[r.version] ?? 0) : '–'}</td>
                      <td>{r.activeUsers}</td>
                      <td className="text-right space-x-1">
                        {r.state === 'live'
                          ? promoteTargets(r, i).map((to) => (
                              <Button
                                key={`p-${to}`}
                                disabled={busy}
                                onClick={() =>
                                  void act(async () => {
                                    await promote(app, from(r), to, r.version)
                                    return `${to} now serves ${r.version}.`
                                  })
                                }
                              >
                                {`Promote to ${to}`}
                              </Button>
                            ))
                          : null}
                        {r.state === 'live'
                          ? rollbackTargets(r, i).map((ch) => (
                              <Button key={`r-${ch}`} disabled={busy} onClick={() => void act(async () => rollback(app, ch, r.version))}>
                                {`Roll ${ch} back to this`}
                              </Button>
                            ))
                          : null}
                        {r.state === 'live' ? (
                          <Button variant="danger-quiet" disabled={busy} onClick={() => setAsking(r.id)}>
                            Pull…
                          </Button>
                        ) : null}
                      </td>
                    </tr>
                    {r.error ? (
                      <tr>
                        <td colSpan={7} className={muted}>
                          {r.error}
                        </td>
                      </tr>
                    ) : null}
                    {r.files.map((f) => (
                      <tr key={`${r.id}-${f.platform}`} className={muted}>
                        <td />
                        <td colSpan={2}>{f.platform}</td>
                        <td>{size(f.size)}</td>
                        <td colSpan={3}>
                          {f.url ? (
                            <a href={f.url} target="_blank" rel="noreferrer" className="text-[var(--color-accent)]">
                              {f.file}
                            </a>
                          ) : (
                            `${f.file} (${f.upload})`
                          )}
                        </td>
                      </tr>
                    ))}
                  </React.Fragment>
                ))}
              </tbody>
            </table>
            <p className={muted}>
              A rollback can’t take people back down: Tauri installs only newer versions, so people already on a bad
              version stay on it until a higher version ships. Pull stops offering a version on every channel.
            </p>
            {u.endpoint ? <p className={muted}>Updater endpoint: {u.endpoint}</p> : null}
          </>
        )}
      </div>
      {asking ? (() => {
        const r = u?.releases.find((x) => x.id === asking)
        if (!r) return null
        return (
          <ConfirmSheet
            title={`Pull ${r.version}?`}
            message="No channel offers it any more. People who already have it keep it until a newer version ships."
            confirmLabel={`Pull ${r.version}`}
            destructive
            testId="release-pull-confirm"
            onCancel={() => setAsking(null)}
            onConfirm={() => {
              setAsking(null)
              void act(async () => pull(app, r.version))
            }}
          />
        )
      })() : null}
    </SettingsGroup>
  )
}

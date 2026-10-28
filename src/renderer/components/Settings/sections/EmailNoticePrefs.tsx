// Settings → Notifications → "Email notices" (prd-email-notifications-
// part2-v1 §4 N10, V): the person's own address for agent email notices
// (add, its confirm status, remove) and which workspaces' agents may email
// them. The same as `k2 notifications email address` and `k2 notify email
// mute` (CLI). The daemon owns every value and rule; this block renders them
// and sends the person's clicks to the server it is given.

import React, { useCallback, useEffect, useState } from 'react'
import { Toggle } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import {
  errorText,
  fetchMyEmailNotices,
  noticeAddressStatus,
  removeMyNoticeAddress,
  setMyNoticeAddress,
  setMyNoticeMute,
  type MyEmailNotices,
} from './email-notice-api'

export interface NoticeWorkspace {
  projectId: string
  name: string
}

const muted = 'text-[10px] text-[var(--color-text-muted)]'

export function EmailNoticePrefs({ scope, workspaces }: { scope: ServerScope; workspaces: NoticeWorkspace[] }): React.JSX.Element | null {
  const [mine, setMine] = useState<MyEmailNotices | null>(null)
  const [missing, setMissing] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [draft, setDraft] = useState('')
  const [sentTo, setSentTo] = useState<string | null>(null)

  const load = useCallback(async (): Promise<void> => {
    try {
      setMine(await fetchMyEmailNotices(scope))
      setLoadError(null)
    } catch (e) {
      // An older server (or Email Notifications switched off) has no route.
      if (e instanceof DaemonHttpError && e.status === 404) setMissing(true)
      else setLoadError(errorText(e))
    }
  }, [scope])

  useEffect(() => {
    void load()
  }, [load])

  const run = async (write: () => Promise<void>): Promise<boolean> => {
    setBusy(true)
    setError(null)
    try {
      await write()
      await load()
      return true
    } catch (e) {
      setError(errorText(e))
      return false
    } finally {
      setBusy(false)
    }
  }

  if (missing) return null

  const add = async (): Promise<void> => {
    const address = draft.trim()
    if (!address) return
    if (await run(() => setMyNoticeAddress(scope, address))) {
      setSentTo(address)
      setDraft('')
    }
  }

  const live = mine ? [...(mine.address ? [mine.address] : []), ...mine.pending] : []
  const mutedSet = new Set(mine?.mutedWorkspaces ?? [])

  return (
    <div data-testid="email-notices" data-settings-id="notifications.email-notices" className="space-y-2">
      <p className={`${muted} py-1`}>
        Agents on this server can email you a short notice (&ldquo;the nightly build failed&rdquo;) once you confirm an
        address and the owner turns notices on for that workspace. Each email has a link to stop them.
      </p>
      {loadError && (
        <p role="alert" className="text-[11px] text-[var(--color-status-error)]">
          Could not load your email notices: {loadError}
        </p>
      )}
      {!mine && !loadError && <p className="text-[11px] text-[var(--color-text-muted)]">Loading…</p>}
      {mine && (
        <>
          {live.map((a) => (
            <div key={a.id} data-testid={`email-notice-address-${a.state}`} className="flex items-center justify-between gap-3 py-2 border-b border-[var(--color-border)]">
              <div className="min-w-0">
                <div className="text-xs text-[var(--color-text-primary)] truncate">{a.address}</div>
                <div className={muted}>{noticeAddressStatus(a)}</div>
              </div>
              <button
                type="button"
                disabled={busy}
                aria-label={`Stop email notices to ${a.address}`}
                onClick={() => void run(() => removeMyNoticeAddress(scope, a.address))}
                className="px-2 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50"
              >
                Remove
              </button>
            </div>
          ))}
          <form
            className="flex items-center gap-2 py-2"
            onSubmit={(e) => {
              e.preventDefault()
              void add()
            }}
          >
            <input
              type="email"
              value={draft}
              disabled={busy}
              onChange={(e) => setDraft(e.target.value)}
              placeholder={mine.address ? 'Use another address…' : 'you@example.com'}
              aria-label="Your address for email notices"
              className="flex-1 min-w-0 px-2 py-1 text-xs bg-transparent border border-[var(--color-border)] text-[var(--color-text-primary)]"
            />
            <button
              type="submit"
              disabled={busy || draft.trim() === ''}
              className="px-2 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50"
            >
              Send confirm link
            </button>
          </form>
          {sentTo && (
            <p data-testid="email-notice-sent" className={muted}>
              We sent a confirm link to {sentTo}. Open it and press Confirm; notices start after that.
            </p>
          )}
          {workspaces.length > 0 && (
            <div data-testid="email-notice-workspaces">
              {workspaces.map((w) => {
                const on = !mutedSet.has(w.projectId)
                return (
                  <div key={w.projectId} data-testid={`email-notice-ws-${w.projectId}`} className="flex items-center justify-between gap-4 py-2 border-b border-[var(--color-border)]">
                    <div className="min-w-0">
                      <div className="text-xs text-[var(--color-text-primary)] truncate">{w.name}</div>
                      <div className={muted}>{on ? 'May email you' : 'Off for you'}</div>
                    </div>
                    <Toggle
                      aria-label={`Email notices from ${w.name}`}
                      checked={on}
                      disabled={busy}
                      onChange={(next) => void run(() => setMyNoticeMute(scope, w.projectId, !next))}
                    />
                  </div>
                )
              })}
            </div>
          )}
        </>
      )}
      {error && (
        <p role="alert" data-testid="email-notice-error" className="text-[11px] text-[var(--color-status-error)]">
          {error}
        </p>
      )}
    </div>
  )
}

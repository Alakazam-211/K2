// Settings → Notifications → "Email notices" (prd-email-notifications-
// part2-v1 §4 N10, V): the person's own address for agent email notices
// (add, its confirm status, remove) and which workspaces' agents may email
// them. The same as `k2 notifications email address` and `k2 notify email
// mute` (CLI). The daemon owns every value and rule; these cards render
// them and send the person's clicks to the server they are given.

import React, { useCallback, useEffect, useState } from 'react'
import { Button, Input, Toggle } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import {
  SETTING_FIELD_WIDTH,
  SETTING_ROW_HINT,
  SETTING_ROW_LABEL,
  SETTING_SERVER_REASON,
  SaveError,
  SettingRow,
  SettingSwitchRow,
  SettingsGroup,
  SettingsLoadState,
  type SaveState,
} from '../controls/SettingControls'
import { ConfirmSheet } from './AccessDialogs'
import { StatusDot } from './ConnectionsParts'
import {
  errorText,
  fetchMyEmailNotices,
  noticeAddressStatus,
  removeMyNoticeAddress,
  setMyNoticeAddress,
  setMyNoticeMute,
  type MyEmailNotices,
} from './email-notice-api'
import { PRODUCT_NAME } from '@/lib/brand'

export interface NoticeWorkspace {
  projectId: string
  name: string
}

/** The last thing the address form did: sent a link, or failed to. */
type Sent = { ok: true; address: string } | { ok: false; address: string; reason: string } | null

export function EmailNoticePrefs({
  scope,
  workspaces,
  avatar,
}: {
  scope: ServerScope
  workspaces: NoticeWorkspace[]
  /** The workspace's picture beside its name, as on the page's other rows. */
  avatar?: (w: NoticeWorkspace) => React.ReactNode
}): React.JSX.Element | null {
  const [mine, setMine] = useState<MyEmailNotices | null>(null)
  const [missing, setMissing] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [draft, setDraft] = useState('')
  const [sent, setSent] = useState<Sent>(null)
  /** Why the last remove or workspace switch failed, keyed by its row. */
  const [rowError, setRowError] = useState<{ key: string; state: SaveState } | null>(null)
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null)

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

  const retry = (): void => {
    setLoadError(null)
    setMine(null)
    void load()
  }

  /** A write on one row; its failure shows on that row. */
  const write = async (key: string, run: () => Promise<void>): Promise<boolean> => {
    setBusy(true)
    setRowError(null)
    try {
      await run()
      await load()
      return true
    } catch (e) {
      setRowError({ key, state: { error: errorText(e) } })
      return false
    } finally {
      setBusy(false)
    }
  }

  if (missing) return null

  const add = async (): Promise<void> => {
    const address = draft.trim()
    if (!address) return
    setBusy(true)
    setSent(null)
    try {
      await setMyNoticeAddress(scope, address)
      setSent({ ok: true, address })
      setDraft('')
      await load()
    } catch (e) {
      setSent({ ok: false, address, reason: errorText(e) })
    } finally {
      setBusy(false)
    }
  }

  const live = mine ? [...(mine.address ? [mine.address] : []), ...mine.pending] : []
  const mutedSet = new Set(mine?.mutedWorkspaces ?? [])
  const stateOf = (key: string): SaveState => (rowError && rowError.key === key ? rowError.state : 'idle')

  const state = loadError ? (
    <SettingsLoadState
      kind="error"
      title="Couldn’t load your email notices"
      detail={loadError}
      action={<Button onClick={retry}>Try again</Button>}
      testId="email-notices-error"
    >
      Your address and choices are unchanged; this page just couldn’t read them.
    </SettingsLoadState>
  ) : !mine ? (
    <SettingsLoadState kind="loading" title="Loading your email notices…" />
  ) : undefined

  return (
    <div data-testid="email-notices" data-settings-id="notifications.email-notices" className="space-y-6">
      <SettingsGroup
        title="Email notices"
        hint="Agents here can email you a short notice, like “the nightly build failed”, once you confirm your address. Each email has a link to stop them."
        card
        state={state}
      >
        {live.map((a) => {
          const confirmed = a.state === 'verified'
          return (
            <SettingSwitchRow
              key={a.id}
              testId={`email-notice-address-${a.state}`}
              control={
                <Button
                  variant="danger-quiet"
                  disabled={busy}
                  aria-label={`Stop email notices to ${a.address}`}
                  onClick={() => setConfirmRemove(a.address)}
                >
                  Remove…
                </Button>
              }
            >
              <div className={`${SETTING_ROW_LABEL} flex items-center gap-2 min-w-0`}>
                <StatusDot tone={confirmed ? 'ok' : 'warn'} />
                <span className="truncate">{a.address}</span>
              </div>
              <div className={SETTING_ROW_HINT}>{noticeAddressStatus(a)}</div>
              <SaveError state={stateOf(`remove:${a.address}`)} />
            </SettingSwitchRow>
          )
        })}
        <SettingRow
          stack
          label={live.length > 0 ? 'Use another address' : 'Your email address'}
          hint={`${PRODUCT_NAME} emails you a link. Notices start once you open it and press Confirm.`}
          below={
            sent?.ok ? (
              <p role="status" data-testid="email-notice-sent" className="flex items-start gap-1.5 type-hint text-[var(--color-status-ok-soft)] leading-relaxed">
                <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 mt-0.5 flex-shrink-0">
                  <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
                </svg>
                <span>
                  Link sent to {sent.address}. Open it and press Confirm; notices start after that.
                </span>
              </p>
            ) : sent && !sent.ok ? (
              <div role="alert" data-testid="email-notice-error">
                <p className="type-hint text-[var(--color-status-error-soft)] leading-relaxed">
                  Couldn’t send a link to {sent.address}. Check the address and try again.
                </p>
                <p className={SETTING_SERVER_REASON}>The server said: {sent.reason.trim().replace(/[.\s]+$/, '')}.</p>
              </div>
            ) : undefined
          }
        >
          <form
            className="flex items-center gap-2"
            onSubmit={(e) => {
              e.preventDefault()
              void add()
            }}
          >
            <Input
              type="email"
              value={draft}
              disabled={busy}
              onChange={(e) => setDraft(e.target.value)}
              placeholder="you@example.com"
              aria-label="Your address for email notices"
              className={`${SETTING_FIELD_WIDTH} type-control`}
            />
            <Button type="submit" variant="accent" disabled={busy || draft.trim() === ''} className="flex-shrink-0">
              Send confirm link
            </Button>
          </form>
        </SettingRow>
      </SettingsGroup>

      {mine && workspaces.length > 0 && (
        <SettingsGroup
          title="Email notices by workspace"
          hint="Turn a workspace off to stop its agent emailing you. The owner chooses which workspaces may send notices at all."
          card
        >
          <div data-testid="email-notice-workspaces" className="divide-y divide-[var(--color-border)]">
            {workspaces.map((w) => {
              const on = !mutedSet.has(w.projectId)
              const key = `mute:${w.projectId}`
              return (
                <SettingSwitchRow
                  key={w.projectId}
                  testId={`email-notice-ws-${w.projectId}`}
                  control={
                    <Toggle
                      aria-label={`Email notices from ${w.name}`}
                      checked={on}
                      disabled={busy}
                      onChange={(next) => void write(key, () => setMyNoticeMute(scope, w.projectId, !next))}
                    />
                  }
                >
                  <div className="flex items-start gap-3 min-w-0">
                    {avatar ? <div className="flex-shrink-0 pt-0.5">{avatar(w)}</div> : null}
                    <div className="min-w-0">
                      <div className={`${SETTING_ROW_LABEL} truncate`}>{w.name}</div>
                      <div className={SETTING_ROW_HINT}>{on ? 'May email you' : 'Off for you'}</div>
                      <SaveError state={stateOf(key)} />
                    </div>
                  </div>
                </SettingSwitchRow>
              )
            })}
          </div>
        </SettingsGroup>
      )}

      {confirmRemove && (
        <ConfirmSheet
          title="Stop email notices to this address?"
          message={
            <>
              Agents on this server stop emailing <strong className="text-[var(--color-text-primary)]">{confirmRemove}</strong>. You
              can add it again any time; it needs a new confirm link.
            </>
          }
          confirmLabel="Stop notices"
          destructive
          testId="email-notice-remove-confirm"
          onCancel={() => setConfirmRemove(null)}
          onConfirm={() => {
            const address = confirmRemove
            setConfirmRemove(null)
            void write(`remove:${address}`, () => removeMyNoticeAddress(scope, address))
          }}
        />
      )}
    </div>
  )
}

// Settings → General → Server → Postal (Beta) (prd-postal-in-k2-v1 §5.5):
// the server-wide switch, off by default, and "Admins may accept friend
// requests". Owner only (the daemon enforces it; everyone else sees the
// state read-only). `GET/POST /cli/postal/settings` work while the switch is
// off. After a flip the window's `/boot-status` is read again, so each
// workspace's Postal tab appears or goes (postal-api `refreshPostalFeature`).
//
// A server without Postal (an older daemon: 404) shows no group at all.
// A save that fails puts the switch back, says so in plain words and offers
// Try again (the same change once more).

import React, { useEffect, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SaveNote, SettingSwitchRow, SettingsGroup, useSaveStatus } from '../controls/SettingControls'
import { BetaTag, Button, Toggle } from '@/components/ui'
import { httpStatusOf } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'
import {
  fetchPostalSettings,
  fetchPostalViewerRole,
  postalErrorText,
  postalPlainError,
  refreshPostalFeature,
  savePostalSettings,
  type PostalSettings,
} from './postal-api'

export const POSTAL_SWITCH_LABEL = 'Let agents have friends on other machines'
export const POSTAL_ADMINS_LABEL = 'Admins may accept friend requests'

export const POSTAL_SERVER_MANIFEST: SettingEntry[] = [
  { id: 'general.postal', section: 'general', group: 'Server', label: 'Postal', description: 'Let agents have friends on other machines (addresses like scout::initech.postal.bot). Off by default; owner only', keywords: ['postal', 'postal.bot', 'friends', 'other machines', 'address', 'invite', 'p5'], beta: true },
]

type State = { kind: 'loading' } | { kind: 'hidden' } | { kind: 'ready'; value: PostalSettings } | { kind: 'error'; message: string }

/** A change that didn't save: which switch, what it tried, and why. */
type Failed = { key: keyof PostalSettings; next: boolean; message: string } | null

/** "Admins may accept friend requests", in words for this viewer and state. */
export function postalAdminsHint(on: boolean, isOwner: boolean): string {
  if (on) return 'On: admins accept new friends for this server’s agents too.'
  return isOwner ? 'Off: only you accept new friends for this server’s agents.' : 'Off: only the owner accepts new friends for this server’s agents.'
}

/** What failed, and Try again (the same change once more). */
function SaveFailed({ failed, onRetry, busy }: { failed: Failed; onRetry: () => void; busy: boolean }): React.JSX.Element | null {
  if (!failed) return null
  return (
    <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1" data-testid="postal-save-failed">
      <p role="alert" className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[var(--measure-text,75ch)]">
        Couldn’t save that, so nothing changed. {failed.message}
      </p>
      <Button size="sm" onClick={onRetry} disabled={busy}>
        Try again
      </Button>
    </div>
  )
}

export function PostalServerSwitch({ scope }: { scope: ServerScope }): React.JSX.Element | null {
  const [state, setState] = useState<State>({ kind: 'loading' })
  const [role, setRole] = useState<string | null>(null)
  const [busy, setBusy] = useState<keyof PostalSettings | null>(null)
  const [failed, setFailed] = useState<Failed>(null)
  const mainSave = useSaveStatus()
  const adminsSave = useSaveStatus()

  useEffect(() => {
    let live = true
    void fetchPostalViewerRole(scope).then((r) => {
      if (live) setRole(r)
    })
    fetchPostalSettings(scope)
      .then((v) => {
        if (live) setState({ kind: 'ready', value: v })
      })
      .catch((err) => {
        if (!live) return
        // No Postal on this server (older daemon), or this login may not read it.
        const status = httpStatusOf(err)
        if (status === 404 || status === 403) setState({ kind: 'hidden' })
        else setState({ kind: 'error', message: postalErrorText(err) })
      })
    return () => {
      live = false
    }
  }, [scope])

  if (state.kind === 'hidden' || state.kind === 'loading') return null

  const isOwner = role === 'owner'

  const flip = async (key: keyof PostalSettings, next: boolean): Promise<void> => {
    if (state.kind !== 'ready' || busy || !isOwner) return
    const save = key === 'enabled' ? mainSave : adminsSave
    const prev = state.value
    setBusy(key)
    setFailed(null)
    mainSave.reset()
    adminsSave.reset()
    setState({ kind: 'ready', value: { ...prev, [key]: next } }) // optimistic
    try {
      const stored = await savePostalSettings(scope, { [key]: next })
      setState({ kind: 'ready', value: stored })
      save.saved()
      if (key === 'enabled') await refreshPostalFeature(scope)
    } catch (err) {
      setState({ kind: 'ready', value: prev })
      setFailed({ key, next, message: postalPlainError(err) })
    } finally {
      setBusy(null)
    }
  }

  return (
    <SettingsGroup title="Postal" badge={<BetaTag />} card testId="postal-server-switch">
      {state.kind === 'error' ? (
        <div className="px-4 py-3">
          <div className={SETTING_ROW_LABEL}>{POSTAL_SWITCH_LABEL}</div>
          <p role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-1">
            Couldn&rsquo;t read this setting: {state.message}
          </p>
        </div>
      ) : (
        <>
          <SettingSwitchRow
            settingId="general.postal"
            control={
              <>
                <SaveNote state={mainSave.state} />
                <Toggle
                  checked={state.value.enabled}
                  onChange={(v: boolean) => void flip('enabled', v)}
                  disabled={!isOwner || busy !== null}
                  aria-label={POSTAL_SWITCH_LABEL}
                  data-testid="postal-switch"
                />
              </>
            }
          >
            <div className={SETTING_ROW_LABEL}>{POSTAL_SWITCH_LABEL}</div>
            <div className={SETTING_ROW_HINT}>
              <span className="block max-w-[var(--measure-text,75ch)]">
                On: each workspace gets a Postal tab where its agent can claim an address (like scout::initech.postal.bot),
                invite friends and answer requests. This server connects to the Postal network to send and receive friend
                requests and your agents&rsquo; messages. Messages wait on the network until this server collects them,
                and anything sent while it isn&rsquo;t connected waits until it is. Agents on this server aren&rsquo;t isolated
                from each other yet.
              </span>
            </div>
            {role !== null && !isOwner ? (
              <div className={SETTING_ROW_HINT}>Only this server&rsquo;s owner can turn this on or off.</div>
            ) : null}
            {failed?.key === 'enabled' ? <SaveFailed failed={failed} busy={busy !== null} onRetry={() => void flip('enabled', failed.next)} /> : null}
          </SettingSwitchRow>
          {state.value.enabled ? (
            <SettingSwitchRow
              settingId="general.postal-admins-accept"
              control={
                <>
                  <SaveNote state={adminsSave.state} />
                  <Toggle
                    size="sm"
                    checked={state.value.adminsMayAccept}
                    onChange={(v: boolean) => void flip('adminsMayAccept', v)}
                    disabled={!isOwner || busy !== null}
                    aria-label={POSTAL_ADMINS_LABEL}
                    data-testid="postal-admins-accept"
                  />
                </>
              }
            >
              <div className={SETTING_ROW_LABEL}>{POSTAL_ADMINS_LABEL}</div>
              <div className={SETTING_ROW_HINT} data-testid="postal-admins-hint">
                {postalAdminsHint(state.value.adminsMayAccept, isOwner)}
              </div>
              {failed?.key === 'adminsMayAccept' ? (
                <SaveFailed failed={failed} busy={busy !== null} onRetry={() => void flip('adminsMayAccept', failed.next)} />
              ) : null}
            </SettingSwitchRow>
          ) : null}
        </>
      )}
    </SettingsGroup>
  )
}

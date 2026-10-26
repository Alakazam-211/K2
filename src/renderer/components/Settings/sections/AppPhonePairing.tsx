// Settings → Apps → <App> → Phones (prd-app-phone-pairing-v1 §8.6, 0.46.5):
// whether people can pair the App's own phone app by QR from its web page;
// the iOS and Android apps that open the codes; the hosts K2 serves their
// link files on; paired phones; Turn off; Check; and the Setup sheet (the
// owner, with a fresh vault confirm). Thin: the daemon owns every rule and
// answers in words (app-pairing-api.ts). No QR here: the QR is the App's,
// on its own page, for its own users.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import { SettingsGroup } from '../controls/SettingControls'
import { vaultApi } from './vault-api'
import {
  APP_PAIRING_CONFIRM_ACTION,
  checkPairing,
  checkWord,
  fetchPairing,
  lines,
  offWords,
  parseAndroidLines,
  setupPairing,
  turnPairingOff,
  type HostCheck,
  type PairingStatus,
} from './app-pairing-api'

const muted = 'text-[10px] text-[var(--color-text-muted)]'
const secondary = 'text-[10px] text-[var(--color-text-secondary)]'
const field =
  'w-full px-2 py-1 text-[10px] bg-transparent border border-[var(--color-border)] text-[var(--color-text-primary)] font-mono'

const NO_CONFIRM_HINT =
  'No vault password and no Owner Connect login on this box: setting the phone apps needs one (k2 vault password set).'

type Confirm = { step: 'idle' } | { step: 'starting' } | { step: 'mac' } | { step: 'password'; method: string }

export function AppPhonePairing({ app }: { app: string }): React.JSX.Element {
  const [status, setStatus] = useState<PairingStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [checks, setChecks] = useState<HostCheck[] | null>(null)
  const [editing, setEditing] = useState(false)
  const [confirm, setConfirm] = useState<Confirm>({ step: 'idle' })
  const [ios, setIos] = useState('')
  const [android, setAndroid] = useState('')
  const [origins, setOrigins] = useState('')
  const [paths, setPaths] = useState('')
  const [storeIos, setStoreIos] = useState('')
  const [storeAndroid, setStoreAndroid] = useState('')
  const pwRef = useRef<HTMLInputElement>(null)

  const load = useCallback(async () => {
    try {
      setStatus(await fetchPairing(app))
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }, [app])

  useEffect(() => {
    void load()
  }, [load])

  const openSetup = (): void => {
    const s = status
    setIos((s?.ios ?? []).join('\n'))
    setAndroid((s?.android ?? []).map((a) => [a.package, ...a.sha256].join(' ')).join('\n'))
    setOrigins((s?.nativeOrigins ?? []).join('\n'))
    setPaths((s?.linkPaths ?? []).join('\n'))
    setStoreIos(s?.storeIos ?? '')
    setStoreAndroid(s?.storeAndroid ?? '')
    setError(null)
    setNotice(null)
    setEditing(true)
  }

  const finish = async (method: string, password?: string): Promise<void> => {
    const { token } = await vaultApi.confirmFinish({
      action: APP_PAIRING_CONFIRM_ACTION,
      secretId: app,
      method,
      ...(password !== undefined ? { password } : {}),
    })
    const next = await setupPairing(
      app,
      {
        ios: lines(ios),
        android: parseAndroidLines(android),
        nativeOrigins: lines(origins),
        linkPaths: lines(paths),
        storeIos,
        storeAndroid,
      },
      token,
    )
    setConfirm({ step: 'idle' })
    setEditing(false)
    setStatus(next)
    setNotice(next.warnings.length ? next.warnings.join(' ') : 'Phone pairing is set up.')
  }

  const save = async (): Promise<void> => {
    setError(null)
    setConfirm({ step: 'starting' })
    try {
      const { methods } = await vaultApi.confirmStart(APP_PAIRING_CONFIRM_ACTION, app)
      if (methods.includes('local-auth')) {
        setConfirm({ step: 'mac' })
        await finish('local-auth')
        return
      }
      const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
      if (pw) {
        setConfirm({ step: 'password', method: pw })
        return
      }
      setConfirm({ step: 'idle' })
      setError(NO_CONFIRM_HINT)
    } catch (e) {
      setConfirm({ step: 'idle' })
      setError(daemonErrorText(e))
    }
  }

  const submitPassword = async (method: string): Promise<void> => {
    const el = pwRef.current
    if (!el) return
    const pw = el.value
    el.value = ''
    try {
      await finish(method, pw)
    } catch (e) {
      setConfirm({ step: 'idle' })
      setError(daemonErrorText(e))
    }
  }

  const act = async (f: () => Promise<void>): Promise<void> => {
    setBusy(true)
    setError(null)
    setNotice(null)
    try {
      await f()
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingsGroup title="Phones">
      <div data-settings-id="skin-access.phones" className="space-y-2" aria-label="Phone pairing">
        {error ? (
          <p data-testid="phones-error" aria-live="polite" className="text-[10px] text-[var(--color-status-error-soft)]">
            {error}
          </p>
        ) : null}
        {notice ? (
          <p data-testid="phones-notice" aria-live="polite" className={secondary}>
            {notice}
          </p>
        ) : null}
        <p className={muted}>
          People signed in to this App on the web can sign its own phone app in by QR, with no password on the phone.
        </p>
        {status == null ? (
          <p className={muted}>Loading phone pairing…</p>
        ) : (
          <>
            <p data-testid="phones-state" className="text-[10px] text-[var(--color-text-primary)]">
              Phone pairing {status.on ? 'on' : `off: ${offWords(status)}`}
            </p>
            {status.ios.map((id) => (
              <p key={`ios-${id}`} className={secondary}>
                iOS · <span className="font-mono">{id}</span>
              </p>
            ))}
            {status.android.map((a) => (
              <p key={`and-${a.package}`} className={secondary}>
                Android · <span className="font-mono">{a.package}</span> ({a.sha256.length} fingerprint
                {a.sha256.length === 1 ? '' : 's'})
              </p>
            ))}
            {status.hosts.length === 0 ? (
              <p className={muted}>No public host: the App needs K2 Connect and its own label.</p>
            ) : (
              status.hosts.map((h) => {
                const c = (checks ?? status.lastCheck ?? []).find((x) => x.host === h.host)
                return (
                  <p key={h.host} className={secondary}>
                    <span className="font-mono">{h.host}</span>
                    {h.pairs ? '' : ' · not pairing'}
                    {c
                      ? ` · iOS file ${checkWord(c.aasa)}, Android file ${checkWord(c.assetlinks)}, Apple's copy ${checkWord(c.appleCdn)}`
                      : ''}
                  </p>
                )
              })
            )}
            {status.storeIos ? <p className={secondary}>App Store · {status.storeIos}</p> : null}
            {status.storeAndroid ? <p className={secondary}>Google Play · {status.storeAndroid}</p> : null}
            <p data-testid="phones-counts" className={muted}>
              Paired phones now: {status.pairedPhones} · Pairings in the last day: {status.pairingsLastDay}
              {status.staySignedInDays === 0 ? ' · Stay signed in is off, so phones can’t pair' : ''}
            </p>
            {!editing ? (
              <div className="flex gap-2">
                <Button disabled={busy} onClick={openSetup}>
                  {status.ios.length || status.android.length ? 'Change phone apps…' : 'Set up…'}
                </Button>
                {status.on ? (
                  <Button disabled={busy} onClick={() => void act(async () => setStatus(await turnPairingOff(app)))}>
                    Turn off
                  </Button>
                ) : null}
                {status.on ? (
                  <Button disabled={busy} onClick={() => void act(async () => setChecks(await checkPairing(app)))}>
                    Check
                  </Button>
                ) : null}
              </div>
            ) : (
              <div className="space-y-1" aria-label="Phone pairing setup">
                <label className={secondary}>
                  iOS app ids (TEAMID.com.you.app), one per line
                  <textarea aria-label="iOS app ids" className={field} rows={2} value={ios} onChange={(e) => setIos(e.target.value)} />
                </label>
                <label className={secondary}>
                  Android apps: package then SHA-256 fingerprints, one app per line
                  <textarea aria-label="Android apps" className={field} rows={2} value={android} onChange={(e) => setAndroid(e.target.value)} />
                </label>
                <label className={secondary}>
                  Webview origins (tauri://localhost, capacitor://localhost), if any
                  <textarea aria-label="Webview origins" className={field} rows={1} value={origins} onChange={(e) => setOrigins(e.target.value)} />
                </label>
                <label className={secondary}>
                  The App&apos;s other universal-link paths (/jobs/*), if any
                  <textarea aria-label="Link paths" className={field} rows={1} value={paths} onChange={(e) => setPaths(e.target.value)} />
                </label>
                <label className={secondary}>
                  App Store link
                  <input aria-label="App Store link" className={field} value={storeIos} onChange={(e) => setStoreIos(e.target.value)} />
                </label>
                <label className={secondary}>
                  Google Play link
                  <input aria-label="Google Play link" className={field} value={storeAndroid} onChange={(e) => setStoreAndroid(e.target.value)} />
                </label>
                <p className={muted}>
                  While pairing is on, K2 answers this App&apos;s iOS and Android link files on its hosts. In your app, list
                  each host exactly, never a wildcard. Saving asks you to confirm (Touch ID, your login or the vault
                  password).
                </p>
                {confirm.step === 'password' ? (
                  <div className="flex gap-2 items-center">
                    <input
                      ref={pwRef}
                      type="password"
                      aria-label={confirm.method === 'vault-password' ? 'Vault password' : 'Your password'}
                      className={field}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') void submitPassword(confirm.method)
                      }}
                    />
                    <Button onClick={() => void submitPassword(confirm.method)}>Confirm</Button>
                  </div>
                ) : null}
                <div className="flex gap-2">
                  <Button disabled={confirm.step !== 'idle'} onClick={() => void save()}>
                    {confirm.step === 'mac' ? 'Confirm with Touch ID…' : 'Save'}
                  </Button>
                  <Button
                    onClick={() => {
                      setEditing(false)
                      setConfirm({ step: 'idle' })
                    }}
                  >
                    Cancel
                  </Button>
                </div>
              </div>
            )}
          </>
        )}
      </div>
    </SettingsGroup>
  )
}

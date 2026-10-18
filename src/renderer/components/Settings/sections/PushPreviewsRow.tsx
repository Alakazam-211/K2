// Settings → Notifications → Message previews (0.46.1).
//
// The owner's server-wide `pushMessagePreviews` switch (default on). On, each
// phone's own preview choice decides whether a Thread push shows the start of
// the agent's message; off, every phone gets the generic body. The daemon owns
// the value (`/cli/settings`, Admin floor in `REMOTE_ACCESS_KEYS`); this row
// renders it and sends the click.
//
// Shown only when the focused server's `/cli/settings/get` carries the key, so
// it appears once the daemon side lands and stays hidden on older servers. The
// caller (NotificationsSection) mounts it for the Owner and Admins only.

import React, { useEffect, useState } from 'react'

import { Toggle } from '@/components/ui'
import { SettingsGroup } from '../controls/SettingControls'
import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'

export const PUSH_PREVIEWS_KEY = 'pushMessagePreviews'
export const PUSH_PREVIEWS_LABEL = 'Show message text in push notifications'
export const PUSH_PREVIEWS_HINT =
  "The text passes through K2's push relay and Apple or Google to reach your phone. When off, every phone on this server gets “New message in Thread” instead."

/** The switch's value, or `null` when this server doesn't have the key (an
 *  older daemon): the row hides. */
export function pushPreviewsValue(settings: Record<string, unknown> | null): boolean | null {
  const v = settings?.[PUSH_PREVIEWS_KEY]
  return typeof v === 'boolean' ? v : null
}

export function PushPreviewsRow(): React.JSX.Element | null {
  const [on, setOn] = useState<boolean | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let cancelled = false
    settingsGet().then(
      (s) => {
        if (!cancelled) setOn(pushPreviewsValue(s as unknown as Record<string, unknown>))
      },
      (e: unknown) => {
        // Unknown means hidden: never show a switch whose value we can't read.
        console.warn('[notifications] settings read failed:', e)
      },
    )
    return () => {
      cancelled = true
    }
  }, [])

  if (on === null) return null

  const change = async (next: boolean): Promise<void> => {
    if (busy) return
    const prev = on
    setOn(next) // optimistic
    setBusy(true)
    setError(null)
    try {
      const s = await settingsUpdate({ [PUSH_PREVIEWS_KEY]: next })
      const v = pushPreviewsValue(s as unknown as Record<string, unknown>)
      setOn(v ?? next)
    } catch (e) {
      setOn(prev)
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingsGroup title="Message previews">
      <div
        className="py-2 border-b border-[var(--color-border)]"
        data-settings-id="notifications.previews"
        data-testid="notifications-push-previews"
      >
        <div className="flex items-center justify-between gap-4">
          <span className="text-xs text-[var(--color-text-primary)]">{PUSH_PREVIEWS_LABEL}</span>
          <Toggle
            aria-label={PUSH_PREVIEWS_LABEL}
            checked={on}
            disabled={busy}
            onChange={(v: boolean) => void change(v)}
          />
        </div>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">{PUSH_PREVIEWS_HINT}</p>
        {error && (
          <p role="alert" className="text-[10px] text-[var(--color-status-error)] mt-1">
            {error}
          </p>
        )}
      </div>
    </SettingsGroup>
  )
}

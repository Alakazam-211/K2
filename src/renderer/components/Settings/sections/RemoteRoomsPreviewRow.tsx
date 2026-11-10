import { SettingRow } from '../controls/SettingControls'
import React from 'react'
import { Toggle } from '@/components/ui'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'

// ── Open other servers' agents in Home ─────────────────────────────────
// Home (prd-home-multi-server-client MS55; prd-home-seamless-0432 Z1–Z4).
// On (the default on every OS from 0.45.5): a Home row on another server opens
// that server's room in Home without switching. Off: it switches this
// window's server. Graduated out of Experimental in 0.46.3 (Rosson
// 2026-10-21): it lives in Workspace settings (Workspaces / Agents, 0.46.5), desktop only. Per computer,
// never the daemon: localStorage `k2.homeRemoteRooms.v1` (what each value
// means, and the one-time graduation reset: lib/remote-rooms-preview).
export const REMOTE_ROOMS_LABEL = "Open other servers' agents in Home"

export function RemoteRoomsPreviewRow(): React.JSX.Element {
  const enabled = useRemoteRoomsPreviewStore((s) => s.enabled)
  const setEnabled = useRemoteRoomsPreviewStore((s) => s.setEnabled)
  return (
    <SettingRow
      settingId="general.remote-rooms-preview"
      label={REMOTE_ROOMS_LABEL}
      hint="Clicking an agent on Home that lives on another server opens it right here. Off: it switches this window to that server."
    >
      <Toggle
        checked={enabled}
        onChange={(next) => setEnabled(next)}
        aria-label={REMOTE_ROOMS_LABEL}
      />
    </SettingRow>
  )
}

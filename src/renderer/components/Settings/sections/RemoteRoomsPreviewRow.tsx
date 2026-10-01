import React from 'react'
import { Toggle } from '@/components/ui'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'

// ── Remote rooms (preview) ─────────────────────────────────────────────
// Home M4 (prd-home-multi-server-client MS55, answer Q3). Off: a Home row on
// another server switches this window's server, as today. On: it opens that
// server's room in Home without switching. Per computer, never the daemon:
// localStorage `k2.homeRemoteRooms.v1`.
export function RemoteRoomsPreviewRow(): React.JSX.Element {
  const enabled = useRemoteRoomsPreviewStore((s) => s.enabled)
  const setEnabled = useRemoteRoomsPreviewStore((s) => s.setEnabled)
  return (
    <div
      className="flex items-center justify-between py-2"
      data-settings-id="general.remote-rooms-preview"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">
          Remote rooms (preview)
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          Open a Home agent that lives on another server right here, without
          switching this window&apos;s server. Off: clicking it switches
          servers, as before.
        </p>
      </div>
      <Toggle
        checked={enabled}
        onChange={(next) => setEnabled(next)}
        aria-label="Remote rooms (preview)"
      />
    </div>
  )
}

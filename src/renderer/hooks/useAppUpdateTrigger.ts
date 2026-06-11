// Mounts the Shape A `app:update-trigger` listener (0.39.35 — unified
// remote update). When this machine is a "bundled-app" host and a remote
// Owner/Admin triggers an update, the local daemon emits an
// `app:update-trigger` Tauri event carrying the job_id; we drive the app's
// OWN Tauri updater and relay phases back to the local daemon (see
// `lib/app-update-driver.ts`).
//
// Mount ONCE at app startup (App-level). Idempotent within a window: the
// effect tears its listener down on unmount.

import { useEffect } from 'react'
import { listen } from '@tauri-apps/api/event'
import {
  APP_UPDATE_TRIGGER_EVENT,
  handleAppUpdateTrigger,
} from '@/lib/app-update-driver'

export function useAppUpdateTrigger(): void {
  useEffect(() => {
    const unlistenPromise = listen(APP_UPDATE_TRIGGER_EVENT, (event) => {
      void handleAppUpdateTrigger(event.payload)
    })
    return () => {
      void unlistenPromise.then((unlisten) => unlisten())
    }
  }, [])
}

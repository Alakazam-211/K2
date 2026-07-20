/**
 * Web shim for `@tauri-apps/plugin-notification`.
 * Uses the browser Notification API when available; otherwise no-ops.
 */

export async function isPermissionGranted(): Promise<boolean> {
  if (typeof Notification === 'undefined') return false
  return Notification.permission === 'granted'
}

export async function requestPermission(): Promise<
  'granted' | 'denied' | 'default'
> {
  if (typeof Notification === 'undefined') return 'denied'
  try {
    return await Notification.requestPermission()
  } catch {
    return 'denied'
  }
}

export function sendNotification(options: {
  title: string
  body?: string
  icon?: string
}): void {
  if (typeof Notification === 'undefined') return
  if (Notification.permission !== 'granted') return
  try {
    new Notification(options.title, {
      body: options.body,
      icon: options.icon,
    })
  } catch (err) {
    console.warn('[web-shim] Notification failed:', err)
  }
}

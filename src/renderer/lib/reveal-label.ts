import { desktopOsFromNavigator } from '@/lib/desktop-chrome'

/**
 * Context-menu label for revealing a path in the host file manager.
 * Prefers client OS via UA (daemon may differ on remote; open still runs
 * on the daemon — label is best-effort UX).
 */
export function revealInFileManagerLabel(): string {
  if (typeof navigator === 'undefined') return 'Reveal in file manager'
  const ua = navigator.userAgent
  // iOS/iPadOS report MacIntel platform but aren't Finder; Files drawer
  // is desktop-first so Mac UA → Finder is correct for Tauri clients.
  if (/Mac|Darwin/i.test(ua) && !/iPhone|iPad|iPod/i.test(ua)) {
    return 'Open in Finder'
  }
  if (/Win/i.test(ua)) return 'Open in Explorer'
  if (/Linux|X11|CrOS/i.test(ua)) return 'Show in Files'
  return 'Reveal in file manager'
}

/** Where a deleted file goes, by this desktop's name: Windows' Recycle Bin, else Trash. */
export function trashName(): string {
  if (typeof navigator === 'undefined') return 'Trash'
  return desktopOsFromNavigator(navigator) === 'windows' ? 'Recycle Bin' : 'Trash'
}

// Words that name this computer and how its owner confirms, per desktop.
//
// Windows says "this PC", and a K2 confirm there is a password: the
// daemon offers Touch ID (`local-auth`) only on a Mac; Linux and Windows
// confirm with the Connect login password or the vault password
// (`vault_routes.rs` `methods`). macOS and Linux keep today's words.

import { desktopOsFromNavigator, type DesktopOs } from '@/lib/desktop-chrome'

export function wordsOs(): DesktopOs {
  return desktopOsFromNavigator(typeof navigator === 'undefined' ? null : navigator)
}

/** "This PC" on Windows, "This Mac" elsewhere (as before). */
export function thisMachine(os: DesktopOs = wordsOs()): string {
  return os === 'windows' ? 'This PC' : 'This Mac'
}

/** "this PC" on Windows, "this Mac" elsewhere (as before). */
export function thisMachineLower(os: DesktopOs = wordsOs()): string {
  return os === 'windows' ? 'this PC' : 'this Mac'
}

/** "Touch ID or your password" on a Mac and Linux (as before); "your password" on Windows. */
export function touchIdOrPassword(os: DesktopOs = wordsOs()): string {
  return os === 'windows' ? 'your password' : 'Touch ID or your password'
}

/** "Touch ID, your login or the vault password" (as before); Windows: "your login or the vault password". */
export function confirmWays(os: DesktopOs = wordsOs()): string {
  return os === 'windows' ? 'your login or the vault password' : 'Touch ID, your login or the vault password'
}

/** "Touch ID or your vault password" (as before); Windows: "your vault password". */
export function touchIdOrVaultPassword(os: DesktopOs = wordsOs()): string {
  return os === 'windows' ? 'your vault password' : 'Touch ID or your vault password'
}

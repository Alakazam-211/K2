// Option-click (macOS) / Alt-click (Windows, Linux) on a file in a chat
// surface opens it in the background (Rosson, 0.46.1 RC smoke 2026-10-18:
// "If the user does an Option+click, open the tab but don't navigate to it
// ... and show it as a tooltip on hover. For all files and paths.").
//
// One place for the gesture and its words, so every Thread file link and
// file chip reads the same.

import { isMacPlatform } from '@/lib/desktop-chrome'

/** How a file open should land. */
export interface FileOpenOptions {
  /** Add (or find) the tab without showing it: the view, the workspace and
   *  the focus stay where they are. */
  background?: boolean
}

/** Is this click (or key press) the background-open gesture? */
export function isBackgroundOpenEvent(e: { altKey?: boolean }): boolean {
  return e.altKey === true
}

/** The open options for a click: `{ background: true }` on Option/Alt. */
export function fileOpenOptionsFor(e: { altKey?: boolean }): FileOpenOptions | undefined {
  return isBackgroundOpenEvent(e) ? { background: true } : undefined
}

/** `⌥-click` on a Mac, `Alt-click` elsewhere. */
export function backgroundClickLabel(isMac: boolean = isMacPlatform()): string {
  return isMac ? '⌥-click' : 'Alt-click'
}

/** The hover hint under a file's path. */
export function fileOpenHint(isMac: boolean = isMacPlatform()): string {
  return `Click to open · ${backgroundClickLabel(isMac)} to open in background`
}

/** The tooltip: the path (with its line) on the first line, the hint under. */
export function fileOpenTooltip(where: string, isMac: boolean = isMacPlatform()): string {
  return where ? `${where}\n${fileOpenHint(isMac)}` : fileOpenHint(isMac)
}

/** The spoken form: "Option-click" / "Alt-click to open in background". */
export function fileOpenAriaHint(isMac: boolean = isMacPlatform()): string {
  return `${isMac ? 'Option-click' : 'Alt-click'} to open in background`
}

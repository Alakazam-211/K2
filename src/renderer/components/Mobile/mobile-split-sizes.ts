// Mobile tab Thread | device split width, per WINDOW and per room
// (prd-mobile-dev-v1 M-A7): the Design tab's rules under its own prefix.
// Never daemon state: a view preference of this screen.

import { getWindowLabel } from '@/lib/window-chrome'
import { createSplitSizes } from '@/components/Design/design-split-sizes'

export const MOBILE_SPLIT_KEY_PREFIX = 'k2.windowChrome.mobile.'

/** `[threadFlex, deviceFlex]` before any drag: a phone is narrow. */
export const DEFAULT_MOBILE_SPLIT: readonly [number, number] = Object.freeze([45, 55]) as readonly [number, number]

const mobileSplit = createSplitSizes(MOBILE_SPLIT_KEY_PREFIX, DEFAULT_MOBILE_SPLIT)

export function mobileSplitKey(label: string): string {
  return mobileSplit.key(label)
}

export function readMobileSplit(roomKey: string | null, label = getWindowLabel()): [number, number] {
  return mobileSplit.read(roomKey, label)
}

export function writeMobileSplit(roomKey: string, split: readonly number[], label = getWindowLabel()): void {
  mobileSplit.write(roomKey, split, label)
}

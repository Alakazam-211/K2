// When the scene's picture is the file's own (the docs preview route draws
// it) while everything else is still the sample's, the banner says so,
// without contradicting itself: the picture source reports which it is,
// and the scene's sample tab words its banner by it (LiveShellSample).

import { createContext } from 'react'
import { PRODUCT_NAME } from '@/lib/brand'

/** Told whether the picture is the file's own (true) or the sample's stand-in (false). */
export const PictureRealContext = createContext<((real: boolean) => void) | null>(null)

/** The banner while the picture is the file's: the same banner and player, honest words. */
export const REAL_PICTURE_BANNER = {
  title: 'Picture from this file',
  line: `People, holds and changes are ${PRODUCT_NAME}’s sample until live editing arrives.`,
} as const

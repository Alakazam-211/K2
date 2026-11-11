// The Kessel config the app gives every terminal: the active palette's
// terminal colors and the font picked in Settings → Terminal → Font.
// App.tsx mounts it above the whole app tree (and above Home's rooms).

import React, { useMemo } from 'react'
import { KesselConfigProvider } from '@/kessel/config-context'
import { defaultKesselConfig, terminalFontStack } from '@/kessel/config'
import { useStyleStore } from '@/stores/style'
import { useSettingsStore } from '@/stores/settings'
import { toKesselColors } from '@/lib/style-resolve'
import { getPalette } from '@/styles.generated'

/** Style System P3 — the terminal color bridge. Derives the active
 *  palette's terminal colors (ANSI-16 + fg/bg/cursor/selection) from the
 *  style store and mounts the (previously never-mounted)
 *  KesselConfigProvider above the ENTIRE app tree, so every
 *  TerminalPane — main layout, focus mode, pinned-chat retainer kept
 *  alive under the Settings overlay — reads them via useKesselConfig().
 *  A palette switch changes `overrides` identity → the provider
 *  re-merges → the painter effects (which depend on config.colors.*)
 *  re-run on live terminals. The charcoal default's terminal block is
 *  value-identical to defaultKesselConfig.colors, so default boots
 *  render exactly as before. Runtime OSC-4 from TUIs still wins (it
 *  layers over the config palette downstream). */
export function StyledKesselProvider({ children }: { children: React.ReactNode }): React.JSX.Element {
  const styleId = useStyleStore((s) => s.styleId)
  const resolvedPaletteId = useStyleStore((s) => s.resolvedPaletteId)
  // Settings → Terminal → Font: the default font keeps the default config.
  const terminalFont = useSettingsStore((s) => s.terminal?.fontFamily)
  const overrides = useMemo(() => {
    const palette = getPalette(styleId, resolvedPaletteId)
    const family = terminalFontStack(terminalFont)
    const font = family === defaultKesselConfig.font.family ? undefined : { family }
    if (!palette && !font) return undefined
    return { ...(palette ? { colors: toKesselColors(palette.terminal) } : {}), ...(font ? { font } : {}) }
  }, [styleId, resolvedPaletteId, terminalFont])
  return <KesselConfigProvider overrides={overrides}>{children}</KesselConfigProvider>
}

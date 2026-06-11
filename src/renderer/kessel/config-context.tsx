// Kessel — React context for KesselConfig.
//
// Exposes the current configuration to every component inside the
// Kessel pane without prop-drilling. No provider = default config
// (behavior identical to pre-config Kessel).
//
// Usage:
//   <KesselConfigProvider value={myConfig}>
//     <SessionStreamView .../>
//   </KesselConfigProvider>
//
// Inside any descendant:
//   const config = useKesselConfig()
//   const fontSize = config.font.size

import React, { createContext, useContext, useMemo } from 'react'

import {
  type KesselConfig,
  type KesselConfigOverrides,
  defaultKesselConfig,
  mergeKesselConfig,
} from './config'

const KesselConfigContext = createContext<KesselConfig>(defaultKesselConfig)

export interface KesselConfigProviderProps {
  /** Full config. Use this when you've already merged overrides
   *  against a non-default baseline. */
  value?: KesselConfig
  /** Overrides on top of `defaultKesselConfig`. Shortcut so callers
   *  can pass e.g. `{ font: { size: 16 } }` without building the
   *  full config tree. */
  overrides?: KesselConfigOverrides
  children: React.ReactNode
}

/** Provider that memoizes the merged config so descendants don't
 *  rerender when the parent rerenders with an equivalent override
 *  object. */
export function KesselConfigProvider({
  value,
  overrides,
  children,
}: KesselConfigProviderProps): React.JSX.Element {
  const merged = useMemo(
    () => value ?? mergeKesselConfig(overrides),
    [value, overrides],
  )
  return (
    <KesselConfigContext.Provider value={merged}>
      {children}
    </KesselConfigContext.Provider>
  )
}

/** Read the current Kessel config. Returns `defaultKesselConfig`
 *  when no provider is above the caller. */
export function useKesselConfig(): KesselConfig {
  return useContext(KesselConfigContext)
}

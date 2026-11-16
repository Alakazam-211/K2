// The file a live view is watching, for the views that ask the daemon about
// it themselves (the scene's picture asks the docs preview route whether
// the daemon draws this file). Given by the live entry on a file tab; none
// elsewhere (the views' tests and samples), where a view uses its sample.

import { createContext, useContext } from 'react'

export interface LiveFile {
  /** The file's path, as the daemon's docs routes take it. */
  path: string
}

export const LiveFileContext = createContext<LiveFile | null>(null)

export const useLiveFile = (): LiveFile | null => useContext(LiveFileContext)

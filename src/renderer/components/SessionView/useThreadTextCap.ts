// The reading column's width, one place: "Thread text width" (Settings →
// Workspaces / Agents → Workspace settings, `threadTextWidth.ts`). The Thread's
// message column and the markdown editor's Write and Read column both read it
// here, so they stay in step: change the setting and both move.

import { useSettingsStore } from '@/stores/settings'
import { threadTextCapPx } from './threadTextWidth'

/** The column's limit in px, or null for Full width (no limit). */
export function useThreadTextCapPx(): number | null {
  return useSettingsStore((s) => threadTextCapPx(s.threadTextWidth))
}

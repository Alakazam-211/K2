// Turns a `DropText` (lib/drop-payload.ts) into text to insert, or shows
// the refusal as a warning toast and gives null: the caller then inserts
// nothing.

import type { DropText } from '@/lib/drop-payload'
import { useToastStore } from '@/stores/toast'

export function dropTextOrToast(r: DropText): string | null {
  if (r.ok) return r.text
  useToastStore.getState().addToast(r.refusal, 'warning')
  return null
}

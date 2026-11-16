// The models K2 offers per harness, and the rows of the model picker
// (0.46.13): the Thread's `/model` menu and Settings → Providers / Models
// "Default model". One list, so the workspace default, the server default
// and a chat's switch offer the same models.

/** The CLI binary a preset command runs (first token, path and `.exe` stripped). */
export function binaryTokenFromCommand(command: string): string {
  const first = command.trim().split(/\s+/)[0] ?? ''
  const base = first.split(/[/\\]/).pop() ?? first
  return base.replace(/\.exe$/i, '')
}

/** The server-default key (`defaultModels`) for a CLI binary, if it has one. */
export function defaultModelHarness(bin: string): string | null {
  const b = bin === 'agent' ? 'cursor-agent' : bin
  return DEFAULT_MODEL_HARNESSES.includes(b) ? b : null
}

/** Harnesses a server default model can be set for (the daemon's list). */
export const DEFAULT_MODEL_HARNESSES: readonly string[] = ['claude', 'codex', 'grok', 'gemini', 'cursor-agent', 'pi']

/** The models K2 suggests for a harness's CLI binary. */
export function modelSuggestionsForAgent(command: string): string[] {
  switch (binaryTokenFromCommand(command)) {
    case 'claude':
      return ['opus', 'sonnet', 'haiku']
    case 'codex':
      return ['gpt-5.3-codex', 'o3', 'gpt-4.1']
    case 'grok':
      return ['grok-4', 'grok-3-mini']
    case 'gemini':
      return ['gemini-2.5-pro', 'gemini-2.5-flash']
    case 'cursor-agent':
    case 'agent':
      return ['composer-1']
    default:
      return []
  }
}

/** A model's name for people ("Opus 5.5"); an id without one shows as itself. */
const LABELS: Record<string, { name: string; note?: string }> = {
  opus: { name: 'Opus 5.5', note: 'Most capable' },
  sonnet: { name: 'Sonnet 5.5', note: 'Balanced' },
  haiku: { name: 'Haiku 5.5', note: 'Fastest' },
}

export function modelLabel(id: string): string {
  return LABELS[id]?.name ?? id
}

export function modelNote(id: string): string | undefined {
  return LABELS[id]?.note
}

/** One picker row. `value` '' is the server default (Thread) or "Harness
 *  default" (Settings): no model of its own. */
export interface ModelRow {
  value: string
  label: string
  secondary?: string
}

/** The picker's rows. `harness` is the CLI binary; `serverDefault` names the
 *  first row (`thread` mode: "Server default (Opus 5.5)"; `setting` mode:
 *  "Harness default"); `query` filters by id and name, case-insensitive. A
 *  current model that isn't a suggestion still gets its row. */
export function modelPickerRows(input: {
  harness: string
  mode: 'thread' | 'setting'
  serverDefault?: string | null
  current?: string | null
  query?: string
}): ModelRow[] {
  const ids = [...modelSuggestionsForAgent(input.harness)]
  const cur = input.current?.trim()
  if (cur && !ids.includes(cur)) ids.push(cur)
  const first: ModelRow =
    input.mode === 'thread'
      ? {
          value: '',
          label: input.serverDefault ? `Server default (${modelLabel(input.serverDefault)})` : 'Server default (harness picks)',
          secondary: 'Set in Settings → Providers / Models',
        }
      : { value: '', label: 'Harness default', secondary: 'The harness picks' }
  const rows: ModelRow[] = [first, ...ids.map((id) => ({ value: id, label: modelLabel(id), secondary: modelNote(id) }))]
  const q = (input.query ?? '').trim().toLowerCase()
  if (!q) return rows
  return rows.filter((r) => r.label.toLowerCase().includes(q) || r.value.toLowerCase().includes(q))
}

/** Which row is checked: the current model's row; the server default's
 *  (Thread) or "Harness default" (Settings) when none is set or it is the
 *  default. */
export function checkedModelRow(rows: readonly ModelRow[], current: string | null | undefined, serverDefault?: string | null): string {
  const cur = current?.trim() ?? ''
  if (!cur || (serverDefault && cur === serverDefault)) return ''
  return rows.some((r) => r.value === cur) ? cur : ''
}

/** The composer's model chip shows only when the chat isn't on the server
 *  default (and K2 knows what it's on). */
export function showModelChip(current: string | null | undefined, serverDefault: string | null | undefined): boolean {
  const cur = current?.trim()
  if (!cur) return false
  return cur !== (serverDefault?.trim() ?? '')
}

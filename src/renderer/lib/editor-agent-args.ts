/** Launch argv for an AI File Editor session (catalog, theme, persona).
 *
 *  Claude gets `--append-system-prompt`. Grok takes a positional initial
 *  prompt and often defaults to `[ui] screen_mode = "minimal"` (empty
 *  scrollback + a tiny pinned prompt) — `--fullscreen` forces the normal
 *  TUI so the editor pane is not a blank grid. */

export function commandBaseName(command: string): string {
  const trimmed = command.trim()
  const slash = Math.max(trimmed.lastIndexOf('/'), trimmed.lastIndexOf('\\'))
  return (slash >= 0 ? trimmed.slice(slash + 1) : trimmed).toLowerCase()
}

export function buildEditorAgentArgs(opts: {
  command: string
  baseArgs: string[]
  systemBrief: string
  userMessage: string
}): string[] {
  const name = commandBaseName(opts.command)
  const brief = `${opts.systemBrief}\n\n${opts.userMessage}`
  if (name === 'claude') {
    return [
      ...opts.baseArgs,
      '--append-system-prompt',
      opts.systemBrief,
      opts.userMessage,
    ]
  }
  if (name === 'grok') {
    const args = opts.baseArgs.filter(
      (a) => a !== '--minimal' && a !== '--fullscreen',
    )
    return [...args, '--fullscreen', brief]
  }
  return [...opts.baseArgs, brief]
}

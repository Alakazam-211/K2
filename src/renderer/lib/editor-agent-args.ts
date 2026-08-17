/** Launch argv for an AI File Editor session (catalog, theme, persona).
 *
 *  Claude gets `--append-system-prompt`. Grok takes a positional initial
 *  prompt. Grok's default `[ui] screen_mode = "minimal"` plus alt-screen
 *  looks like a blank Kessel pane (`--fullscreen` still uses alt-screen).
 *  `--no-alt-screen` draws on the primary screen Kessel actually shows. */

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
      (a) =>
        a !== '--minimal' &&
        a !== '--fullscreen' &&
        a !== '--no-alt-screen',
    )
    return [...args, '--no-alt-screen', '--fullscreen', brief]
  }
  return [...opts.baseArgs, brief]
}

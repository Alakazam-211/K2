# LLM Provider Icons

The SVG files in this directory are vendored from **Lobe Icons**:

- Source: https://github.com/lobehub/lobe-icons
- Package: `@lobehub/icons-static-svg`
- License: MIT (Copyright © LobeHub)

| File | Source filename |
|---|---|
| `claude.svg` | `claude-color.svg` |
| `codex.svg` | `codex-color.svg` |
| `copilot.svg` | `copilot-color.svg` |
| `gemini.svg` | `gemini-color.svg` |
| `cursor.svg` | `cursor.svg` |
| `goose.svg` | `goose.svg` |
| `ollama.svg` | replaced with the Simple Icons version (see below) |
| `opencode.svg` | `opencode.svg` |

The `pi.svg` mark is sourced from https://pi.dev/logo.svg (Pi coding
agent's official logo). Hand-converted from `fill="#fff"` to
`fill="currentColor"` so it inherits the wrapper's CSS color the same
way the Lobe monochrome marks do.

The `interpreter.svg` mark is sourced from
https://www.openinterpreter.com/icon.svg (Open Interpreter's official
logo). The original used a `prefers-color-scheme: dark` CSS rule to
flip black/white; converted to inline `fill="currentColor"` so it
follows our app theme rather than the OS theme.

The `ollama.svg` mark replaces Lobe's Ollama variant (which was a
stylized take, not the recognizable squarish llama mark) with the
Simple Icons version (https://simpleicons.org/icons/ollama, CC0).
Hand-converted from `fill="#000000"` to `fill="currentColor"` and
adjusted to `1em` sizing for parity with the rest of the set.

The brand marks themselves are trademarks of their respective owners
(Anthropic, OpenAI, Google, GitHub/Microsoft, etc.). The MIT license
covers the SVG renderings — usage of the underlying logos is governed
by each company's brand-use policy and is permissible here as
"this app integrates with X" attribution.

To refresh, install `@lobehub/icons-static-svg` and copy the matching
files from `node_modules/@lobehub/icons-static-svg/icons/`.

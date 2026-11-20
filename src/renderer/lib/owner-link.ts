// The one-use owner link (open chat as its owner). plugin-opener's openUrl
// would run `open <link>` or `xdg-open <link>`, and every local account can
// read a program's arguments. The app's `open_owner_link` opens it through a
// private redirect page instead, so only a file path is ever on a command
// line. Every caller with an owner link uses this, never openUrl.
import { invoke } from '@tauri-apps/api/core'

export async function openOwnerLink(link: string): Promise<void> {
  await invoke('open_owner_link', { link })
}

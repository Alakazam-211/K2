// Secrets that arrive as files (runtime §5.2): the App pass at
// K2_PASS_FILE (/run/k2/pass in a wall), which K2 rotates every hour, and
// the App key at $K2_SECRETS_DIR/K2_APP_KEY. Never an env var, never
// logged. The file is read on every use (it is a few bytes on tmpfs), so
// a rotation is picked up by the very next call.

import { readFileSync, statSync } from 'node:fs'

export class SecretFile {
  readonly path: string

  constructor(path: string) {
    this.path = path
  }

  /** Does the file exist now? */
  exists(): boolean {
    try {
      return statSync(this.path).isFile()
    } catch {
      return false
    }
  }

  /** The current value. Throws when the file can't be read or is empty. */
  read(): string {
    const text = readFileSync(this.path, 'utf8').trim()
    if (!text) throw new Error(`${this.path} is empty`)
    return text
  }
}

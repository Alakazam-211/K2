// `GET /cli/fs/read-file`'s text, checked: `{content: string}` or a loud
// error. A 200 that isn't that shape (an empty body, a proxy's HTML page,
// a server too old or too new) used to reach a viewer as `undefined`, and
// an HTML tile drew the word "undefined" instead of the page.

import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliGet } from '@/lib/daemon-cli'

/** The server answered, but not with a file's text. */
export const NOT_FILE_TEXT = 'The server didn’t send this file’s text. Try again, or reopen the file.'

/** A read's body as the file's text, or throws (never `undefined`). */
export function fileTextOf(body: unknown): string {
  const content = body && typeof body === 'object' ? (body as { content?: unknown }).content : undefined
  if (typeof content !== 'string') throw new Error(NOT_FILE_TEXT)
  return content
}

/** Read `path` on `scope`'s server as text. */
export async function readFileText(scope: ServerScope, path: string): Promise<string> {
  return fileTextOf(await daemonCliGet<unknown>(scope, 'fs/read-file', { path }))
}

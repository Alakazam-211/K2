// The Help explorer's catalog is generated from cli/k2 (`--schema`,
// `study`) and k2-node's USAGE. This fails when the CLI moved and the
// file did not: run `node scripts/gen-cli-explorer-catalog.mjs`.

import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import catalog from '@shared/cli-catalog.generated.json'

const ROOT = resolve(__dirname, '../../../..')

describe('cli-catalog.generated.json', () => {
  it('matches what cli/k2 and k2-node print today', () => {
    const out = execFileSync(process.execPath, [resolve(ROOT, 'scripts/gen-cli-explorer-catalog.mjs'), '--check'], {
      cwd: ROOT,
      encoding: 'utf8',
      timeout: 60_000,
    })
    expect(out).toContain('is current')
  }, 70_000)

  it('carries the k2 verbs, the study pages (no leftover aliases) and k2-node', () => {
    expect(catalog.k2.commands.length).toBeGreaterThan(300)
    const ids = catalog.study.map((p) => p.id)
    for (const id of ['what', 'errors', 'people', 'ticket-brief', 'compute', 'vault']) expect(ids).toContain(id)
    expect(ids).not.toContain('skins')
    expect(ids).not.toContain('llm-accounts')
    for (const p of catalog.study) {
      expect([p.id, p.body.startsWith(`k2 study ${p.id}`)]).toEqual([p.id, true])
      expect(p.summary.length).toBeGreaterThan(0)
    }
    expect(catalog.k2Node.commands.map((c) => c.noun)).toContain('enroll')
    expect(catalog.k2Node.footer).toContain('k2 compute')
    expect(Object.keys(catalog.k2.help)).toContain('compute')
  })
})

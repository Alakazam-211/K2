// The Postal tab's address check follows the daemon's label rule (k2-core
// `postal::is_label`, the Postal plane's rule): 1 to 63 of a-z 0-9 -, no
// hyphen first or last, no two in a row, not all digits, not reserved, not
// starting with turn-. The reserved list here, the daemon's and the CLI's
// are one list: this test reads the other two and fails if they drift.
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { isPostalAddress, isPostalLabel, POSTAL_RESERVED_LABELS } from './postal-api'

const repo = join(dirname(fileURLToPath(import.meta.url)), '../../../../..')
const read = (rel: string): string => readFileSync(join(repo, rel), 'utf8')

describe('isPostalLabel', () => {
  it('takes one- and two-character labels, z3 among them, up to 63', () => {
    for (const good of ['z3', 'a', 'ab', 'k2n-lab', 'a1-b2', '0a', 'acme-box', 'a'.repeat(63)]) {
      expect(isPostalLabel(good), good).toBe(true)
    }
  })

  it('refuses what the plane refuses', () => {
    for (const bad of ['', '-a', 'a-', 'a.b', 'a--b', '123', '7', 'turn-x', 'a_b', 'A3', 'a'.repeat(64)]) {
      expect(isPostalLabel(bad), bad).toBe(false)
    }
  })

  it('refuses each reserved name', () => {
    expect(POSTAL_RESERVED_LABELS).toHaveLength(33)
    for (const name of POSTAL_RESERVED_LABELS) expect(isPostalLabel(name), name).toBe(false)
  })

  it('checks the label inside a full address', () => {
    expect(isPostalAddress('scout::z3.postal.bot')).toBe(true)
    expect(isPostalAddress(' Scout::Z3.Postal.Bot ')).toBe(true)
    for (const bad of ['scout::acme.postal.bot', 'scout::123.postal.bot', 'scout::a--b.postal.bot', 'scout::turn-x.postal.bot', 'scout::a.b.postal.bot']) {
      expect(isPostalAddress(bad), bad).toBe(false)
    }
  })

  it('has the same reserved list as the daemon and the CLI', () => {
    const core = read('crates/k2-core/src/postal/mod.rs')
    const block = /pub const RESERVED_LABELS: &\[&str\] = &\[([\s\S]*?)\];/.exec(core)
    expect(block, 'the daemon list').not.toBeNull()
    const daemon = [...(block as RegExpExecArray)[1].matchAll(/"([^"]+)"/g)].map((m) => m[1])
    const bash = /^ +(www\|[a-z0-9|-]+)\) return 1 ;;$/m.exec(read('cli/src/remote-target.sh'))
    expect(bash, 'the CLI list').not.toBeNull()
    const cli = (bash as RegExpExecArray)[1].split('|')
    expect(daemon).toEqual([...POSTAL_RESERVED_LABELS])
    expect(cli).toEqual([...POSTAL_RESERVED_LABELS])
  })
})

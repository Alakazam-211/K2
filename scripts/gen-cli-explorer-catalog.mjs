#!/usr/bin/env node
// Builds the Help explorer's CLI catalog from the CLIs themselves, so the
// renderer never hand-forks their text:
//
//   k2 --schema          every agent-facing verb (usage + description)
//   k2 study [--json]    the bounded teaching pages (ids, titles, summaries)
//   k2 study <id> --json each page's body
//   crates/k2-node/src/main.rs  the k2-node USAGE text (no build needed)
//
// All of those print without a daemon. The CLI runs with a clean env and a
// throwaway HOME so it never reads a real ~/.k2 or talks to a daemon.
//
//   node scripts/gen-cli-explorer-catalog.mjs          write the file
//   node scripts/gen-cli-explorer-catalog.mjs --check  exit 1 if stale
//
// Output: src/shared/cli-catalog.generated.json

import { execFile } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const run = promisify(execFile)
const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const K2 = join(ROOT, 'cli/k2')
const NODE_MAIN = join(ROOT, 'crates/k2-node/src/main.rs')
export const OUT = join(ROOT, 'src/shared/cli-catalog.generated.json')

/** Nouns `k2 --schema` does not list yet. Their `--help` is captured when it
 *  prints without a daemon; the rest fall back to `k2 help <noun>`. */
const HELP_ONLY_NOUNS = ['compute', 'sidecar', 'vault', 'job', 'whoami', 'hooks', 'root-helper', 'onboarding', 'commit', 'git-tools']

/** Leftover aliases `k2 study` keeps for old links; the explorer skips them. */
const STUDY_ALIASES = new Set(['skins', 'llm-accounts'])

/** `k2 <noun> --help` if it prints without a daemon, else null. */
async function nounHelp(home, noun) {
  try {
    const out = (await k2(home, [noun, '--help'])).trimEnd()
    return out ? out : null
  } catch {
    // Exits non-zero (needs a daemon): the explorer says `k2 help <noun>`.
    return null
  }
}

async function k2(home, args) {
  const { stdout, stderr } = await run('bash', [K2, ...args], {
    cwd: ROOT,
    env: { PATH: '/usr/bin:/bin:/usr/sbin:/sbin', HOME: home, LANG: 'C.UTF-8' },
    maxBuffer: 16 * 1024 * 1024,
  })
  if (stderr.trim()) throw new Error(`k2 ${args.join(' ')} wrote to stderr:\n${stderr}`)
  return stdout
}

function parseStudySummaries(text) {
  const out = {}
  for (const line of text.split('\n')) {
    const m = /^ {2}([a-z][a-z-]*) {2,}(\S.*)$/.exec(line)
    if (m) out[m[1]] = m[2].trim()
  }
  return out
}

/** k2-node's USAGE: one row per `k2-node …` line, continuation lines folded in. */
export function parseNodeUsage(src) {
  const m = /const USAGE: &str = r#"([\s\S]*?)"#;/.exec(src)
  if (!m) throw new Error(`no USAGE in ${NODE_MAIN}`)
  const usage = m[1]
  const commands = []
  let section = 'Commands'
  for (const raw of usage.split('\n').slice(1)) {
    const line = raw.replace(/\s+$/, '')
    if (!line.trim()) {
      section = 'Run and manage'
      continue
    }
    const cmd = /^ {2}((?:sudo )?k2-node) (\S+)(.*)$/.exec(line)
    if (cmd) {
      const sub = cmd[2]
      const rest = cmd[3]
      // `usage   (note)` or `usage   description` after 3+ spaces.
      const split = /^(.*?\S) {3,}(\S.*)$/.exec(`${cmd[1]} ${sub}${rest}`)
      const full = split ? split[1] : `${cmd[1]} ${sub}${rest}`
      let note = split ? split[2] : ''
      note = note.replace(/^\((.*)\)$/, '$1')
      commands.push({ noun: sub, usage: full.trim(), note, section })
      continue
    }
    if (/^\S/.test(line)) {
      const head = line.replace(/:$/, '').replace(/ \(.*\)$/, '').replace(/:.*$/, '')
      if (/^Agents never run this/.test(line)) continue
      section = head
      continue
    }
    const prev = commands[commands.length - 1]
    if (!prev) continue
    const cont = line.trim()
    if (/^(Over SSH|ssh |\(the password)/.test(cont)) continue
    if (/^\(.*\)$/.test(cont)) prev.note = prev.note ? `${prev.note} ${cont.slice(1, -1)}` : cont.slice(1, -1)
    else prev.usage = `${prev.usage} ${cont}`
  }
  const tagline = usage.split('\n')[0].replace(/^k2-node — /, '')
  const footer = /Agents never run this[^\n]*/.exec(usage)?.[0] ?? ''
  return { tagline, footer, commands }
}

export async function buildCatalog() {
  const home = mkdtempSync(join(tmpdir(), 'k2-explorer-gen-'))
  try {
    const [schemaText, studyJson, studyText] = await Promise.all([
      k2(home, ['--schema']),
      k2(home, ['study', '--json']),
      k2(home, ['study']),
    ])
    const schema = JSON.parse(schemaText)
    const commands = schema.commands.map((c) => ({
      name: c.name,
      usage: c.usage,
      description: c.description,
    }))
    const summaries = parseStudySummaries(studyText)
    const topics = JSON.parse(studyJson).topics.filter((t) => !STUDY_ALIASES.has(t.id))
    const pages = await Promise.all(
      topics.map(async (t) => {
        const page = JSON.parse(await k2(home, ['study', t.id, '--json']))
        if (page.id !== t.id) throw new Error(`k2 study ${t.id} returned id ${page.id}`)
        const summary = summaries[t.id]
        if (!summary) throw new Error(`k2 study catalog has no summary line for ${t.id}`)
        return { id: t.id, title: t.title, summary, body: page.body }
      }),
    )
    const help = {}
    for (const [noun, text] of await Promise.all(
      HELP_ONLY_NOUNS.map(async (n) => [n, await nounHelp(home, n)]),
    )) {
      if (text) help[noun] = text
    }
    const node = parseNodeUsage(readFileSync(NODE_MAIN, 'utf8'))
    return {
      note: 'Generated by scripts/gen-cli-explorer-catalog.mjs from cli/k2 (--schema, study) and crates/k2-node/src/main.rs. Do not edit.',
      k2: { description: schema.description, commands, help },
      study: pages,
      k2Node: node,
    }
  } finally {
    rmSync(home, { recursive: true, force: true })
  }
}

export function serialize(catalog) {
  return `${JSON.stringify(catalog, null, 1)}\n`
}

const isMain = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)
if (isMain) {
  const text = serialize(await buildCatalog())
  if (process.argv.includes('--check')) {
    const current = readFileSync(OUT, 'utf8')
    if (current !== text) {
      console.error(`${OUT} is stale. Run: node scripts/gen-cli-explorer-catalog.mjs`)
      process.exit(1)
    }
    console.log('src/shared/cli-catalog.generated.json is current')
  } else {
    writeFileSync(OUT, text)
    console.log(`wrote ${OUT} (${text.length} bytes)`)
  }
}

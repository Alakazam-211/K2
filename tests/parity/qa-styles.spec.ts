// Style System QA capture — NON-DEFAULT configurations.
//
// Unlike capture.spec.ts (strict before/after parity on the default
// charcoal/dark look), this spec exists so a human (or agent) can LOOK
// at the app under the new Paper palette and Spacious gaps and hunt for
// visual defects. It is NOT a pixel-diff harness — shots land in
//   tests/parity/shots/qa-<config>/<screen>.png
// and are reviewed by eye.
//
// Style seeding is localStorage-only (per-client view state):
//   1. localStorage (`k2.style` / `k2.palette` / `k2.palette.<scheme>` /
//      `k2.scheme` / `k2.gaps`) via addInitScript, so index.html's
//      pre-paint bootstrap stamps the <html> data-* attributes exactly
//      like a real relaunch would.
//   2. `k2.style.migrated` = `1` so the one-shot upgrade migration never
//      consults a daemon style echo (style is never daemon-canonical after
//      migration). Post-connect must hold from localStorage alone.
// Each capture asserts the <html> attributes still match the config, so
// a revert fails loudly instead of producing default-look screenshots.
//
// Run: bunx playwright test --config tests/parity/qa-styles.config.ts
import { test, expect, type Page } from '@playwright/test'
import * as fs from 'fs'
import * as path from 'path'
import {
  FREEZE_CSS,
  installMockDaemon,
  installTauriShim,
  connectToMockDaemon,
} from './mock-daemon'

interface QaConfig {
  /** Shots directory suffix: tests/parity/shots/qa-<name>/ */
  name: string
  style: string
  palette: string
  scheme: 'dark' | 'light'
  /** '' = the style's base density (attribute absent). */
  gaps: string
}

const CONFIGS: QaConfig[] = [
  { name: 'paper-light', style: 'square', palette: 'paper', scheme: 'light', gaps: '' },
  { name: 'charcoal-spacious', style: 'square', palette: 'charcoal', scheme: 'dark', gaps: 'spacious' },
  { name: 'paper-spacious', style: 'square', palette: 'paper', scheme: 'light', gaps: 'spacious' },
  { name: 'glass-obsidian', style: 'glass', palette: 'obsidian', scheme: 'dark', gaps: '' },
  { name: 'bezel-graphite', style: 'bezel', palette: 'graphite', scheme: 'dark', gaps: '' },
  { name: 'glass-veil', style: 'glass', palette: 'veil', scheme: 'light', gaps: '' },
  { name: 'bezel-porcelain', style: 'bezel', palette: 'porcelain', scheme: 'light', gaps: '' },
]

/** Settle + capture into the config's qa shots dir, then verify the
 *  style attributes were not reverted mid-render (fail loudly — a
 *  reverted shot would review the WRONG configuration). */
async function capture(page: Page, cfg: QaConfig, name: string): Promise<void> {
  await expect(
    page.getByText('Something went wrong rendering K2'),
    `error boundary visible on screen '${name}' (${cfg.name})`,
  ).toHaveCount(0)
  await page.evaluate(async () => {
    await document.fonts.ready
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))
  })
  await page.waitForTimeout(500)
  const attrs = await page.evaluate(() => {
    const h = document.documentElement
    return {
      style: h.getAttribute('data-style'),
      palette: h.getAttribute('data-palette'),
      scheme: h.getAttribute('data-scheme'),
      gaps: h.getAttribute('data-gaps'),
    }
  })
  expect(attrs, `style attributes drifted on '${name}' (${cfg.name})`).toEqual({
    style: cfg.style,
    palette: cfg.palette,
    scheme: cfg.scheme,
    gaps: cfg.gaps || null,
  })
  const shotsDir = path.join(__dirname, 'shots', `qa-${cfg.name}`)
  fs.mkdirSync(shotsDir, { recursive: true })
  await page.screenshot({
    path: path.join(shotsDir, `${name}.png`),
    animations: 'disabled',
    caret: 'hide',
  })
}

/** Boot the renderer with the config seeded pre-paint in localStorage
 *  (client SSOT). Daemon style echo is intentionally NOT required —
 *  post-connect holds from the mirror + migrated flag alone. */
async function boot(page: Page, cfg: QaConfig): Promise<void> {
  await page.addInitScript((c: QaConfig) => {
    localStorage.setItem('k2.style', c.style)
    localStorage.setItem('k2.palette', c.palette)
    localStorage.setItem('k2.palette.' + c.scheme, c.palette)
    localStorage.setItem('k2.scheme', c.scheme)
    if (c.gaps) localStorage.setItem('k2.gaps', c.gaps)
    // Skip one-shot daemon→local migration; seed is the authority.
    localStorage.setItem('k2.style.migrated', '1')
  }, cfg)
  await installTauriShim(page)
  await installMockDaemon(page)
  await page.goto('/', { waitUntil: 'domcontentloaded' })
  await page.addStyleTag({ content: FREEZE_CSS })
}

async function bootApp(page: Page, cfg: QaConfig): Promise<void> {
  await boot(page, cfg)
  await expect(page.getByText('Connecting…')).toBeVisible({ timeout: 10_000 })
  await connectToMockDaemon(page)
  await expect(page.getByText('No workspaces yet')).toBeVisible({ timeout: 15_000 })
}

async function openSettings(page: Page, section: string): Promise<void> {
  await page.evaluate(async (s) => {
    const m = await import('/stores/settings.ts')
    m.useSettingsStore.getState().openSettings(s)
  }, section)
  await expect(page.getByText('Settings', { exact: true }).first()).toBeVisible()
}

for (const cfg of CONFIGS) {
  test.describe(`qa-${cfg.name}`, () => {
    test('01-connect-gate', async ({ page }) => {
      await boot(page, cfg)
      await expect(page.getByText('Connecting…')).toBeVisible({ timeout: 10_000 })
      await capture(page, cfg, '01-connect-gate')
    })

    test('02-app-home', async ({ page }) => {
      await bootApp(page, cfg)
      await expect(page.getByText('Add a workspace to get started')).toBeVisible()
      await capture(page, cfg, '02-app-home')
    })

    test('03-projects-page', async ({ page }) => {
      await bootApp(page, cfg)
      await page.evaluate(async () => {
        const m = await import('/stores/page-view.ts')
        m.usePageViewStore.getState().setPage('projects')
      })
      await expect(page.getByText('Create your first project')).toBeVisible()
      await capture(page, cfg, '03-projects-page')
    })

    test('04-feedback-page', async ({ page }) => {
      await bootApp(page, cfg)
      await page.evaluate(async () => {
        const m = await import('/stores/page-view.ts')
        m.usePageViewStore.getState().setPage('feedback')
      })
      await expect(page.getByText('Feedback', { exact: true }).first()).toBeVisible()
      await capture(page, cfg, '04-feedback-page')
    })

    test('05-settings-general', async ({ page }) => {
      await bootApp(page, cfg)
      await openSettings(page, 'general')
      await capture(page, cfg, '05-settings-general')
    })

    test('06-settings-styles', async ({ page }) => {
      await bootApp(page, cfg)
      await openSettings(page, 'styles')
      await expect(page.getByText('Palette', { exact: true })).toBeVisible()
      await capture(page, cfg, '06-settings-styles')
    })
  })
}

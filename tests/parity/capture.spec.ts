// Screenshot PARITY capture — Style System P1 / Checkpoint A
// (prd-style-system-v1 §6).
//
// Captures a fixed matrix of deterministic screens into
//   tests/parity/shots/<PARITY_LABEL>/<screen>.png
// Run once on main (PARITY_LABEL=before), once on feat/style-system
// (PARITY_LABEL=after), then `bun run parity:diff`. See README.md.
//
// Every test FAILS LOUDLY: required env var, required marker text per
// screen, and an explicit no-error-boundary assertion before each shot.
import { test, expect, type Page } from '@playwright/test'
import * as fs from 'fs'
import * as path from 'path'
import {
  FREEZE_CSS,
  installMockDaemon,
  installTauriShim,
  connectToMockDaemon,
} from './mock-daemon'

const label = process.env.PARITY_LABEL
if (!label || !/^[A-Za-z0-9._-]+$/.test(label)) {
  throw new Error(
    `PARITY_LABEL env var is required (e.g. PARITY_LABEL=before bun run parity:capture); ` +
      `got: ${JSON.stringify(label)}`,
  )
}
const shotsDir = path.join(__dirname, 'shots', label)
fs.mkdirSync(shotsDir, { recursive: true })

/** Settle + capture. Fonts must be loaded and two frames rendered; the
 *  error boundary must NOT be showing (a crashed view diffing equal on
 *  both branches would be a false PASS). */
async function capture(page: Page, name: string): Promise<void> {
  await expect(
    page.getByText('Something went wrong rendering K2'),
    `error boundary visible on screen '${name}'`,
  ).toHaveCount(0)
  await page.evaluate(async () => {
    await document.fonts.ready
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))
  })
  // Small fixed settle for late microtask renders (store hydration).
  await page.waitForTimeout(500)
  await page.screenshot({
    path: path.join(shotsDir, `${name}.png`),
    animations: 'disabled',
    caret: 'hide',
  })
}

/** Boot the renderer with the Tauri shim + freeze CSS (no daemon yet). */
async function boot(page: Page): Promise<void> {
  await installTauriShim(page)
  await installMockDaemon(page)
  await page.goto('/', { waitUntil: 'domcontentloaded' })
  await page.addStyleTag({ content: FREEZE_CSS })
}

/** Boot AND mount the full App against the mocked remote daemon. */
async function bootApp(page: Page): Promise<void> {
  await boot(page)
  // The gate renders its pre-mount overlay first; then we steer it onto
  // the mocked host, whose canned /boot-status + whoami get accepted.
  await expect(page.getByText('Connecting…')).toBeVisible({ timeout: 10_000 })
  await connectToMockDaemon(page)
  // Sidebar empty-state proves App (not the gate overlay) is on screen.
  await expect(page.getByText('No workspaces yet')).toBeVisible({ timeout: 15_000 })
}

/** Open a Settings section via the live settings store (same live-module
 *  trick as connectToMockDaemon — deterministic, no menu coordinates). */
async function openSettings(page: Page, section: string): Promise<void> {
  await page.evaluate(async (s) => {
    const m = await import('/stores/settings.ts')
    m.useSettingsStore.getState().openSettings(s)
  }, section)
  await expect(page.getByText('Settings', { exact: true }).first()).toBeVisible()
}

test('01-connect-gate: pre-daemon connecting overlay', async ({ page }) => {
  await boot(page)
  // No daemon anywhere (local reports not_installed; no host selected):
  // the gate parks on its full-screen overlay. Capture well before the
  // 10s mark where the overlay adds its Reload escape-hatch button.
  await expect(page.getByText('Connecting…')).toBeVisible({ timeout: 10_000 })
  await capture(page, '01-connect-gate')
})

test('02-app-home: mounted app shell, Agents page, empty workspace list', async ({ page }) => {
  await bootApp(page)
  await expect(page.getByText('Add a workspace to get started')).toBeVisible()
  await capture(page, '02-app-home')
})

test('03-projects-page: Projects overlay page, empty state', async ({ page }) => {
  await bootApp(page)
  await page.evaluate(async () => {
    const m = await import('/stores/page-view.ts')
    m.usePageViewStore.getState().setPage('projects')
  })
  await expect(page.getByText('Create your first project')).toBeVisible()
  await capture(page, '03-projects-page')
})

test('04-feedback-page: Feedback overlay page, empty state', async ({ page }) => {
  await bootApp(page)
  await page.evaluate(async () => {
    const m = await import('/stores/page-view.ts')
    m.usePageViewStore.getState().setPage('feedback')
  })
  await expect(page.getByText('Feedback', { exact: true }).first()).toBeVisible()
  await capture(page, '04-feedback-page')
})

test('05-settings-general', async ({ page }) => {
  await bootApp(page)
  await openSettings(page, 'general')
  await capture(page, '05-settings-general')
})

test('06-settings-terminal', async ({ page }) => {
  await bootApp(page)
  await openSettings(page, 'terminal')
  await capture(page, '06-settings-terminal')
})

test('07-settings-keybindings', async ({ page }) => {
  await bootApp(page)
  await openSettings(page, 'keybindings')
  await capture(page, '07-settings-keybindings')
})

test('08-settings-connections', async ({ page }) => {
  await bootApp(page)
  await openSettings(page, 'connections')
  await capture(page, '08-settings-connections')
})

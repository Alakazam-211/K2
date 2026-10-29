// @k2/server: K2's server SDK for App code (prd-k2-app-runtime-v1 S23).
//
//   import { fromEnv } from '@k2/server'
//   const k2 = fromEnv()
//   const s = await k2.login({ username, password })
//   if ('mustChangePassword' in s) return redirect(s.changeUrl)  // someone else set it
//   const { pass } = s
//   await k2.asGuest(pass).thread.send({ addr: 'sales', text: 'Hi' })
//
// The verb table and every answer type are generated from K2's verb
// catalog (`./generated/verbs.ts`); `k2 study app-api` is the page.
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { K2App } from "./client.js";
import { K2Error } from "./errors.js";
export { K2App } from "./client.js";
export { coerceTurn, describe as describeTurn, reduceTurn } from "../../activity/src/core.js";
export { K2Error, fromResponse } from "./errors.js";
export * from "./generated/verbs.js";
export { qrSvg } from "./pairing.js";
/** Where a wall mounts the App pass (runtime §5.2). */
export const DEFAULT_PASS_FILE = '/run/k2/pass';
/** The App key's file name under K2_SECRETS_DIR (S45). */
export const APP_KEY_FILE = 'K2_APP_KEY';
/**
 * A client from the App's environment:
 * - `K2_GATEWAY_URL` (else `http://$K2_HOST:$K2_PORT`): where K2 is;
 * - `K2_PASS_FILE` (else `/run/k2/pass` when it exists): the App pass,
 *   read again whenever K2 rotates it;
 * - `K2_APP_ID` (the service id logins name) and `K2_APP_NAME`;
 * - `$K2_SECRETS_DIR/K2_APP_KEY`, when granted: users and rooms.
 *
 * `extra` overrides any of them (and supplies fetch/WebSocket in tests).
 */
export function fromEnv(env = process.env, extra = {}) {
    const host = env.K2_HOST?.trim();
    const port = env.K2_PORT?.trim();
    const gatewayUrl = env.K2_GATEWAY_URL?.trim() || (host && port ? `http://${host}:${port}` : '');
    if (!gatewayUrl && !extra.gatewayUrl) {
        throw new K2Error({
            code: 'usage',
            hint: 'K2_GATEWAY_URL is not set: K2 sets it for every App and --cmd service (or pass gatewayUrl)',
        });
    }
    const named = env.K2_PASS_FILE?.trim();
    const passFile = named || (existsSync(DEFAULT_PASS_FILE) ? DEFAULT_PASS_FILE : null);
    const secrets = env.K2_SECRETS_DIR?.trim();
    const appKeyFile = secrets ? join(secrets, APP_KEY_FILE) : null;
    return new K2App({
        gatewayUrl,
        passFile,
        appId: env.K2_APP_ID?.trim() || null,
        appName: env.K2_APP_NAME?.trim() || null,
        appKeyFile,
        ...extra,
    });
}

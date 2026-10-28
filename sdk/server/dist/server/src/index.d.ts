import { K2App, type K2Config } from './client.ts';
export { K2App, type K2Actor, type K2Config, type RequestLike, type SubscribeOptions } from './client.ts';
export type { ActivityState, Turn, WatchActivityOptions, WatchActivityTarget } from './activity.ts';
export { coerceTurn, describe as describeTurn, reduceTurn } from '../../activity/src/core.ts';
export type { AppKeyOk, AppKeyRoomsSet, AppKeyUser, AppKeyUserCreated, AppKeyUsers, AppKeyWhoami } from './client.ts';
export { K2Error, fromResponse } from './errors.ts';
export * from './generated/verbs.ts';
export { qrSvg, type QrSvgOptions } from './pairing.ts';
/** Where a wall mounts the App pass (runtime §5.2). */
export declare const DEFAULT_PASS_FILE = "/run/k2/pass";
/** The App key's file name under K2_SECRETS_DIR (S45). */
export declare const APP_KEY_FILE = "K2_APP_KEY";
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
export declare function fromEnv(env?: Record<string, string | undefined>, extra?: Partial<K2Config>): K2App;

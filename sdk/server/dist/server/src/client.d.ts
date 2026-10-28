import { K2Error } from './errors.ts';
import { type AppCallVerb, type AppSubscribeVerb, type AppSurfaces, type AppVerbs, type AppGuestLogin } from './generated/verbs.ts';
import { type WebSocketCtor } from './socket.ts';
import { type ActivityState, type WatchActivityOptions, type WatchActivityTarget } from './activity.ts';
export interface K2Config {
    /** Where K2 is: K2_GATEWAY_URL (in a wall http://127.0.0.1:8730). */
    gatewayUrl: string;
    /** The App pass file (K2_PASS_FILE); null when this App has none. */
    passFile?: string | null;
    /** This App's service id (K2_APP_ID): logins name it. */
    appId?: string | null;
    /** This App's slug (K2_APP_NAME). */
    appName?: string | null;
    /** The App key file ($K2_SECRETS_DIR/K2_APP_KEY); null when not granted. */
    appKeyFile?: string | null;
    fetch?: typeof fetch;
    WebSocket?: WebSocketCtor;
}
/** `subscribe`'s options (0.46.5). */
export interface SubscribeOptions {
    /** The socket is open (first time and after every reconnect): catch up
     *  here, after the gap, never on the close-time `resync` error. */
    onOpen?: () => void;
}
/** The surfaces plus `call` / `subscribe` by name, bound to one actor. */
export interface K2Actor extends Omit<AppSurfaces, 'thread'> {
    /** Every Thread verb, plus `watchActivity` (0.46.5): the live Thread
     *  turn of one agent (`{addr}`) or of rooms (`{rooms}`), kept right
     *  (snapshot on every open, frames, resets, a 60 s self-check). */
    readonly thread: AppSurfaces['thread'] & {
        watchActivity(target: WatchActivityTarget, cb: (states: Map<string, ActivityState>) => void, opts?: WatchActivityOptions): () => void;
    };
    call<V extends AppCallVerb>(verb: V, args?: AppVerbs[V]['args']): Promise<AppVerbs[V]['value']>;
    subscribe<V extends AppSubscribeVerb>(verb: V, args: AppVerbs[V]['args'], onFrame: (frame: AppVerbs[V]['value']) => void, onError?: (error: K2Error) => void, opts?: SubscribeOptions): () => void;
}
/** Something with request headers (Node's IncomingMessage, a Fetch Request). */
export interface RequestLike {
    headers: Record<string, string | string[] | undefined> | {
        get(name: string): string | null;
    };
}
export declare class K2App {
    readonly gatewayUrl: string;
    readonly appId: string | null;
    readonly appName: string | null;
    private readonly passFile;
    private readonly keyFile;
    private readonly fetchImpl;
    private readonly WebSocketImpl;
    private readonly guest;
    private readonly defaultActor;
    private readonly appActor;
    constructor(cfg: K2Config);
    /** Calls as the App itself (its pass file). Written out on purpose. */
    asApp(): K2Actor;
    /** Calls as one guest (their session pass, from `login`). */
    asGuest(pass: string): K2Actor;
    /** Run `fn` with `pass` as the default actor for every call inside it. */
    withGuest<T>(pass: string, fn: () => T): T;
    /** The guest bound to the current call chain, if any. */
    currentGuest(): string | null;
    /**
     * The guest pass a request carries: `Authorization: Bearer k2skn_…`, or
     * the `cookie` named (default `k2_skin_session`). Null when none.
     */
    guestFrom(req: RequestLike, cookie?: string): string | null;
    /**
     * Connect/Express-style middleware: the request's guest (guestFrom) is
     * the default actor for everything the handler calls.
     */
    middleware(cookie?: string): (req: RequestLike, res: unknown, next: (err?: unknown) => void) => void;
    /**
     * Log a guest in through this App (`guests.login`; `app` is this App's
     * id). Through a wall's link the App pass opens the door; the answer's
     * `pass` is the guest's session pass.
     */
    login(args: {
        username: string;
        password: string;
    }): Promise<AppGuestLogin>;
    readonly users: {
        list: () => Promise<AppKeyUsers>;
        create: (body: {
            username: string;
            email?: string;
            password?: string;
            rooms?: string[];
            role?: string;
        }) => Promise<AppKeyUserCreated>;
        setRooms: (body: {
            username: string;
            rooms: string[];
            applyTokens?: boolean;
        }) => Promise<AppKeyRoomsSet>;
        disable: (body: {
            username: string;
        }) => Promise<AppKeyOk>;
        enable: (body: {
            username: string;
        }) => Promise<AppKeyOk>;
        remove: (body: {
            username: string;
        }) => Promise<AppKeyOk>;
        unlock: (body: {
            username: string;
        }) => Promise<AppKeyOk>;
        passwordReset: (body: {
            username: string;
        }) => Promise<AppKeyOk>;
    };
    readonly rooms: {
        /** A user's rooms in this App ([] takes every room away). */
        set: (body: {
            username: string;
            rooms: string[];
            applyTokens?: boolean;
        }) => Promise<AppKeyRoomsSet>;
    };
    /** The App key's own view: this App, its scopes and ceiling. */
    keyWhoami(): Promise<AppKeyWhoami>;
    private appKey;
    private actor;
    private row;
    /** The pass a call presents, or null (a login on a --cmd service: no pass needed). */
    private passFor;
    private callAs;
    private subscribeAs;
}
export interface AppKeyUser {
    readonly username: string;
    readonly email: string | null;
    readonly fullName: string | null;
    readonly hasPassword: boolean;
    readonly enabled: boolean;
    readonly roleId: string | null;
    readonly rooms: string[];
    readonly roomHandles: string[];
    readonly app: string;
}
export interface AppKeyUsers {
    readonly app: string;
    readonly users: AppKeyUser[];
}
export interface AppKeyUserCreated {
    readonly ok: boolean;
    readonly user: AppKeyUser;
}
export interface AppKeyRoomsSet {
    readonly ok: boolean;
    readonly username: string;
    readonly app: string;
    readonly rooms: string[];
    readonly roomHandles: string[];
}
export interface AppKeyOk {
    readonly ok: boolean;
    readonly username: string;
    readonly [key: string]: unknown;
}
export interface AppKeyWhoami {
    readonly app: string;
    readonly keyId: string;
    readonly scopes: string[];
    readonly rooms: string[];
    readonly roomHandles: string[];
    readonly roleIds: string[];
}
export interface K2App extends Omit<AppSurfaces, 'thread'> {
    readonly thread: K2Actor['thread'];
}

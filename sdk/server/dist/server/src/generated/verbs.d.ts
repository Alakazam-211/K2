import type { K2Error } from '../errors.ts';
/** The catalog version this table was generated from. */
export declare const CATALOG_VERSION = 35;
/** A JSON Schema from the catalog's subset, reduced for projection. */
export type Schema = {
    readonly [key: string]: unknown;
};
/** One App verb: its transport, names and the answer's shape. */
export interface AppVerbRow {
    readonly cap: string | null;
    readonly effect: 'read' | 'write';
    readonly kind: 'call' | 'subscribe';
    readonly feature: string;
    /** both = helper and link; helper = a guest's own browser session only; link = server-side only. */
    readonly door: 'both' | 'helper' | 'link';
    readonly http: {
        readonly method: 'GET' | 'POST';
        readonly route: string;
        /** Catalog argument name → wire name. */
        readonly params: Readonly<Record<string, string>>;
        /** POST arguments sent in the query string (the rest go in the JSON body). */
        readonly query: readonly string[];
    } | null;
    /** `query`: fixed query parameters the SDK adds to the socket URL (e.g. the ear's `ear=1` opt-in). */
    readonly socket: {
        readonly path: string;
        readonly param: string;
        readonly frame: string;
        readonly refetch: string | null;
        readonly query: Readonly<Record<string, string>>;
    } | null;
    /** Catalog response field → live wire field, at any depth. */
    readonly responseNames: Readonly<Record<string, string>>;
    /** The closed answer (or frame) schema the SDK projects onto. */
    readonly response: Schema;
}
/** Every App verb, in catalog order. */
export declare const APP_VERBS: {
    readonly 'me.get': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/me";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly agents: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly can: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly activity: {
                                        readonly type: "boolean";
                                    };
                                    readonly chat: {
                                        readonly type: "boolean";
                                    };
                                    readonly download: {
                                        readonly type: "boolean";
                                    };
                                    readonly files: {
                                        readonly type: "string";
                                    };
                                    readonly heartbeats: {
                                        readonly type: "string";
                                    };
                                    readonly identity: {
                                        readonly type: "boolean";
                                    };
                                    readonly members: {
                                        readonly type: "boolean";
                                    };
                                    readonly send: {
                                        readonly type: "boolean";
                                    };
                                    readonly store: {
                                        readonly type: "string";
                                    };
                                    readonly tickets: {
                                        readonly type: "string";
                                    };
                                    readonly upload: {
                                        readonly type: "boolean";
                                    };
                                    readonly wiki: {
                                        readonly type: "boolean";
                                    };
                                };
                                readonly type: "object";
                            };
                            readonly caps: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                            readonly color: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly displayName: {
                                readonly type: "string";
                            };
                            readonly folders: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly download: {
                                        readonly items: {
                                            readonly type: "string";
                                        };
                                        readonly type: "array";
                                    };
                                    readonly read: {
                                        readonly items: {
                                            readonly type: "string";
                                        };
                                        readonly type: "array";
                                    };
                                    readonly upload: {
                                        readonly items: {
                                            readonly type: "string";
                                        };
                                        readonly type: "array";
                                    };
                                    readonly uploadDir: {
                                        readonly type: "string";
                                    };
                                    readonly write: {
                                        readonly items: {
                                            readonly type: "string";
                                        };
                                        readonly type: "array";
                                    };
                                };
                                readonly type: "object";
                            };
                            readonly handle: {
                                readonly type: "string";
                            };
                            readonly image: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly muted: {
                                readonly type: "boolean";
                            };
                            readonly room: {
                                readonly type: "string";
                            };
                            readonly ticketsWaiting: {
                                readonly type: "integer";
                            };
                            readonly unread: {
                                readonly type: "integer";
                            };
                            readonly waiting: {
                                readonly type: "integer";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly app: {
                    readonly type: "string";
                };
                readonly catalogVersion: {
                    readonly type: "integer";
                };
                readonly features: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly instanceId: {
                    readonly type: "string";
                };
                readonly kind: {
                    readonly enum: readonly ["guest", "app", "token"];
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly principalId: {
                    readonly type: readonly ["string", "null"];
                };
                readonly publicHost: {
                    readonly type: readonly ["string", "null"];
                };
                readonly rooms: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly caps: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                            readonly folders: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly download: {
                                        readonly items: {
                                            readonly type: "string";
                                        };
                                        readonly type: "array";
                                    };
                                    readonly read: {
                                        readonly items: {
                                            readonly type: "string";
                                        };
                                        readonly type: "array";
                                    };
                                    readonly upload: {
                                        readonly items: {
                                            readonly type: "string";
                                        };
                                        readonly type: "array";
                                    };
                                    readonly uploadDir: {
                                        readonly type: "string";
                                    };
                                    readonly write: {
                                        readonly items: {
                                            readonly type: "string";
                                        };
                                        readonly type: "array";
                                    };
                                };
                                readonly type: "object";
                            };
                            readonly handle: {
                                readonly type: "string";
                            };
                            readonly room: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly serverId: {
                    readonly type: readonly ["string", "null"];
                };
                readonly username: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'roomAgents.list': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/agents";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly agents: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly color: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly displayName: {
                                readonly type: "string";
                            };
                            readonly handle: {
                                readonly type: "string";
                            };
                            readonly image: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly muted: {
                                readonly type: "boolean";
                            };
                            readonly projectId: {
                                readonly type: "string";
                            };
                            readonly ticketsWaiting: {
                                readonly type: "integer";
                            };
                            readonly unread: {
                                readonly type: "integer";
                            };
                            readonly waiting: {
                                readonly type: "integer";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'guests.login': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/login";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly pass: "token";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly caps: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly device: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly id: {
                            readonly type: "string";
                        };
                        readonly name: {
                            readonly type: "string";
                        };
                    };
                    readonly type: "object";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly pass: {
                    readonly type: "string";
                };
                readonly passExpiresAt: {
                    readonly type: "integer";
                };
                readonly principalId: {
                    readonly type: "string";
                };
                readonly refresh: {
                    readonly type: "string";
                };
                readonly refreshExpiresAt: {
                    readonly type: "integer";
                };
                readonly role: {
                    readonly type: readonly ["string", "null"];
                };
                readonly roomAccess: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly caps: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                            readonly handle: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly roomHandles: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly rooms: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly username: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'guests.rooms': {
        readonly cap: "guests:rooms";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "guests-rooms-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/guests/rooms";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly changed: {
                    readonly type: "boolean";
                };
                readonly enabled: {
                    readonly type: "boolean";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly role: {
                    readonly type: readonly ["string", "null"];
                };
                readonly roomHandles: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly rooms: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly username: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'guests.setRooms': {
        readonly cap: "guests:rooms";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "guests-rooms-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/guests/rooms";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly changed: {
                    readonly type: "boolean";
                };
                readonly enabled: {
                    readonly type: "boolean";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly role: {
                    readonly type: readonly ["string", "null"];
                };
                readonly roomHandles: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly rooms: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly username: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'app.rooms': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "guests-rooms-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/app-rooms";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly ceiling: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly grantRooms: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly displayName: {
                                readonly type: "string";
                            };
                            readonly handle: {
                                readonly type: "string";
                            };
                            readonly room: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly granted: {
                    readonly type: "boolean";
                };
                readonly grantedAt: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly ownRoom: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly displayName: {
                            readonly type: "string";
                        };
                        readonly handle: {
                            readonly type: "string";
                        };
                        readonly room: {
                            readonly type: "string";
                        };
                    };
                    readonly type: readonly ["object", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'guests.list': {
        readonly cap: "guests:rooms";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "guests-rooms-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/guests/list";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly guests: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly agent: {
                                readonly type: "boolean";
                            };
                            readonly enabled: {
                                readonly type: "boolean";
                            };
                            readonly role: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly roomHandles: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                            readonly rooms: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                            readonly username: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly nextCursor: {
                    readonly type: readonly ["string", "null"];
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'password.change': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/password/change";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly stayed: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'wallet.list': {
        readonly cap: "wallet:self";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-wallet-v1";
        readonly door: "helper";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/wallet/list";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly items: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly addedVia: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly createdAt: {
                                readonly type: "integer";
                            };
                            readonly dormantSince: {
                                readonly type: readonly ["integer", "null"];
                            };
                            readonly expiresAt: {
                                readonly type: readonly ["integer", "null"];
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly inUse: {
                                readonly type: "boolean";
                            };
                            readonly label: {
                                readonly type: "string";
                            };
                            readonly lastUsedAt: {
                                readonly type: readonly ["integer", "null"];
                            };
                            readonly plan: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly provider: {
                                readonly type: "string";
                            };
                            readonly sealed: {
                                readonly type: "boolean";
                            };
                            readonly state: {
                                readonly type: "string";
                            };
                            readonly type: {
                                readonly enum: readonly ["llm_subscription", "llm_api_key"];
                            };
                            readonly usedBy: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly providers: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly apiKey: {
                            readonly items: {
                                readonly type: "string";
                            };
                            readonly type: "array";
                        };
                        readonly subscription: {
                            readonly items: {
                                readonly type: "string";
                            };
                            readonly type: "array";
                        };
                    };
                    readonly type: "object";
                };
                readonly uses: {
                    readonly additionalProperties: {
                        readonly type: "string";
                    };
                    readonly type: "object";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'wallet.status': {
        readonly cap: "wallet:self";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-wallet-v1";
        readonly door: "helper";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/wallet/status";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly item: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly addedVia: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly createdAt: {
                            readonly type: "integer";
                        };
                        readonly dormantSince: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly expiresAt: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly id: {
                            readonly type: "string";
                        };
                        readonly inUse: {
                            readonly type: "boolean";
                        };
                        readonly label: {
                            readonly type: "string";
                        };
                        readonly lastUsedAt: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly plan: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly provider: {
                            readonly type: "string";
                        };
                        readonly sealed: {
                            readonly type: "boolean";
                        };
                        readonly state: {
                            readonly type: "string";
                        };
                        readonly type: {
                            readonly enum: readonly ["llm_subscription", "llm_api_key"];
                        };
                        readonly usedBy: {
                            readonly items: {
                                readonly type: "string";
                            };
                            readonly type: "array";
                        };
                    };
                    readonly type: "object";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'wallet.add': {
        readonly cap: "wallet:self";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-wallet-v1";
        readonly door: "helper";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/wallet/add";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly item: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly addedVia: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly createdAt: {
                            readonly type: "integer";
                        };
                        readonly dormantSince: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly expiresAt: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly id: {
                            readonly type: "string";
                        };
                        readonly inUse: {
                            readonly type: "boolean";
                        };
                        readonly label: {
                            readonly type: "string";
                        };
                        readonly lastUsedAt: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly plan: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly provider: {
                            readonly type: "string";
                        };
                        readonly sealed: {
                            readonly type: "boolean";
                        };
                        readonly state: {
                            readonly type: "string";
                        };
                        readonly type: {
                            readonly enum: readonly ["llm_subscription", "llm_api_key"];
                        };
                        readonly usedBy: {
                            readonly items: {
                                readonly type: "string";
                            };
                            readonly type: "array";
                        };
                    };
                    readonly type: "object";
                };
                readonly login: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly code: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly done: {
                            readonly type: "boolean";
                        };
                        readonly error: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly itemId: {
                            readonly type: "string";
                        };
                        readonly label: {
                            readonly type: "string";
                        };
                        readonly loginId: {
                            readonly type: "string";
                        };
                        readonly needsPaste: {
                            readonly type: "boolean";
                        };
                        readonly provider: {
                            readonly type: "string";
                        };
                        readonly startedAt: {
                            readonly type: "integer";
                        };
                        readonly state: {
                            readonly type: "string";
                        };
                        readonly url: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly urlHost: {
                            readonly type: readonly ["string", "null"];
                        };
                    };
                    readonly type: readonly ["object", "null"];
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'wallet.login': {
        readonly cap: "wallet:self";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-wallet-v1";
        readonly door: "helper";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/wallet/login";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly item: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly addedVia: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly createdAt: {
                            readonly type: "integer";
                        };
                        readonly dormantSince: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly expiresAt: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly id: {
                            readonly type: "string";
                        };
                        readonly inUse: {
                            readonly type: "boolean";
                        };
                        readonly label: {
                            readonly type: "string";
                        };
                        readonly lastUsedAt: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly plan: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly provider: {
                            readonly type: "string";
                        };
                        readonly sealed: {
                            readonly type: "boolean";
                        };
                        readonly state: {
                            readonly type: "string";
                        };
                        readonly type: {
                            readonly enum: readonly ["llm_subscription", "llm_api_key"];
                        };
                        readonly usedBy: {
                            readonly items: {
                                readonly type: "string";
                            };
                            readonly type: "array";
                        };
                    };
                    readonly type: "object";
                };
                readonly login: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly code: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly done: {
                            readonly type: "boolean";
                        };
                        readonly error: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly itemId: {
                            readonly type: "string";
                        };
                        readonly label: {
                            readonly type: "string";
                        };
                        readonly loginId: {
                            readonly type: "string";
                        };
                        readonly needsPaste: {
                            readonly type: "boolean";
                        };
                        readonly provider: {
                            readonly type: "string";
                        };
                        readonly startedAt: {
                            readonly type: "integer";
                        };
                        readonly state: {
                            readonly type: "string";
                        };
                        readonly url: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly urlHost: {
                            readonly type: readonly ["string", "null"];
                        };
                    };
                    readonly type: readonly ["object", "null"];
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'wallet.loginStatus': {
        readonly cap: "wallet:self";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-wallet-v1";
        readonly door: "helper";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/wallet/login/status";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly login: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly code: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly done: {
                            readonly type: "boolean";
                        };
                        readonly error: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly itemId: {
                            readonly type: "string";
                        };
                        readonly label: {
                            readonly type: "string";
                        };
                        readonly loginId: {
                            readonly type: "string";
                        };
                        readonly needsPaste: {
                            readonly type: "boolean";
                        };
                        readonly provider: {
                            readonly type: "string";
                        };
                        readonly startedAt: {
                            readonly type: "integer";
                        };
                        readonly state: {
                            readonly type: "string";
                        };
                        readonly url: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly urlHost: {
                            readonly type: readonly ["string", "null"];
                        };
                    };
                    readonly type: "object";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'wallet.loginInput': {
        readonly cap: "wallet:self";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-wallet-v1";
        readonly door: "helper";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/wallet/login/input";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'wallet.loginCancel': {
        readonly cap: "wallet:self";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-wallet-v1";
        readonly door: "helper";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/wallet/login/cancel";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly login: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly code: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly done: {
                            readonly type: "boolean";
                        };
                        readonly error: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly itemId: {
                            readonly type: "string";
                        };
                        readonly label: {
                            readonly type: "string";
                        };
                        readonly loginId: {
                            readonly type: "string";
                        };
                        readonly needsPaste: {
                            readonly type: "boolean";
                        };
                        readonly provider: {
                            readonly type: "string";
                        };
                        readonly startedAt: {
                            readonly type: "integer";
                        };
                        readonly state: {
                            readonly type: "string";
                        };
                        readonly url: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly urlHost: {
                            readonly type: readonly ["string", "null"];
                        };
                    };
                    readonly type: "object";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'wallet.use': {
        readonly cap: "wallet:self";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-wallet-v1";
        readonly door: "helper";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/wallet/use";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly id: {
                    readonly type: readonly ["string", "null"];
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly provider: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'wallet.remove': {
        readonly cap: "wallet:self";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-wallet-v1";
        readonly door: "helper";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/wallet/remove";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly note: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly removed: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'guests.logout': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-mobile-access-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/logout";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'guests.refresh': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-mobile-access-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/refresh";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly pass: "token";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly device: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly id: {
                            readonly type: "string";
                        };
                        readonly name: {
                            readonly type: "string";
                        };
                    };
                    readonly type: "object";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly pass: {
                    readonly type: "string";
                };
                readonly passExpiresAt: {
                    readonly type: "integer";
                };
                readonly principalId: {
                    readonly type: "string";
                };
                readonly refresh: {
                    readonly type: "string";
                };
                readonly refreshExpiresAt: {
                    readonly type: "integer";
                };
                readonly username: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'guests.devices': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-mobile-access-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/devices";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly devices: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly createdAt: {
                                readonly type: "integer";
                            };
                            readonly current: {
                                readonly type: "boolean";
                            };
                            readonly deviceId: {
                                readonly type: "string";
                            };
                            readonly expiresAt: {
                                readonly type: "integer";
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly lastUsedAt: {
                                readonly type: "integer";
                            };
                            readonly name: {
                                readonly type: "string";
                            };
                            readonly via: {
                                readonly enum: readonly ["pairing", "password"];
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'guests.revokeDevice': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-mobile-access-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/devices/revoke";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'profile.get': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-user-profile-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/profile";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly agentLogin: {
                    readonly type: "boolean";
                };
                readonly app: {
                    readonly type: "string";
                };
                readonly can: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly editImage: {
                            readonly type: "boolean";
                        };
                        readonly editNames: {
                            readonly type: "boolean";
                        };
                    };
                    readonly type: "object";
                };
                readonly counts: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly devices: {
                            readonly type: "integer";
                        };
                        readonly pushDevices: {
                            readonly type: "integer";
                        };
                        readonly sessions: {
                            readonly type: "integer";
                        };
                    };
                    readonly type: "object";
                };
                readonly createdAt: {
                    readonly type: "integer";
                };
                readonly displayName: {
                    readonly type: readonly ["string", "null"];
                };
                readonly email: {
                    readonly type: readonly ["string", "null"];
                };
                readonly fullName: {
                    readonly type: readonly ["string", "null"];
                };
                readonly hasPassword: {
                    readonly type: "boolean";
                };
                readonly image: {
                    readonly type: readonly ["string", "null"];
                };
                readonly imageHash: {
                    readonly type: readonly ["string", "null"];
                };
                readonly lastSignInAt: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly nameSetBySelf: {
                    readonly type: "boolean";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly role: {
                    readonly type: readonly ["string", "null"];
                };
                readonly rooms: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly handle: {
                                readonly type: "string";
                            };
                            readonly name: {
                                readonly type: "string";
                            };
                            readonly room: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly test: {
                    readonly type: "boolean";
                };
                readonly username: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'profile.update': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-user-profile-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/profile";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly profile: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly agentLogin: {
                            readonly type: "boolean";
                        };
                        readonly app: {
                            readonly type: "string";
                        };
                        readonly can: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly editImage: {
                                    readonly type: "boolean";
                                };
                                readonly editNames: {
                                    readonly type: "boolean";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly counts: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly devices: {
                                    readonly type: "integer";
                                };
                                readonly pushDevices: {
                                    readonly type: "integer";
                                };
                                readonly sessions: {
                                    readonly type: "integer";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly createdAt: {
                            readonly type: "integer";
                        };
                        readonly displayName: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly email: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly fullName: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly hasPassword: {
                            readonly type: "boolean";
                        };
                        readonly image: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly imageHash: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly lastSignInAt: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly nameSetBySelf: {
                            readonly type: "boolean";
                        };
                        readonly ok: {
                            readonly type: "boolean";
                        };
                        readonly role: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly rooms: {
                            readonly items: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly handle: {
                                        readonly type: "string";
                                    };
                                    readonly name: {
                                        readonly type: "string";
                                    };
                                    readonly room: {
                                        readonly type: "string";
                                    };
                                };
                                readonly type: "object";
                            };
                            readonly type: "array";
                        };
                        readonly test: {
                            readonly type: "boolean";
                        };
                        readonly username: {
                            readonly type: "string";
                        };
                    };
                    readonly type: "object";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'profile.setImage': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-user-profile-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/profile/image";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly image: {
                    readonly type: "string";
                };
                readonly imageHash: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'profile.clearImage': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-user-profile-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/profile/image/clear";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'profile.sessions': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-user-profile-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/sessions";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly sessions: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly client: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly createdAt: {
                                readonly type: "integer";
                            };
                            readonly current: {
                                readonly type: "boolean";
                            };
                            readonly expiresAt: {
                                readonly type: readonly ["integer", "null"];
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly kind: {
                                readonly enum: readonly ["browser", "device"];
                            };
                            readonly lastSeenAt: {
                                readonly type: readonly ["integer", "null"];
                            };
                            readonly push: {
                                readonly type: "boolean";
                            };
                            readonly pushPlatform: {
                                readonly type: "string";
                            };
                            readonly via: {
                                readonly type: readonly ["string", "null"];
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'profile.revokeSession': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-user-profile-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/sessions/revoke";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'profile.signOutOthers': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-user-profile-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/sessions/revoke-others";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ended: {
                    readonly type: "integer";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'push.register': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "custom-app-push-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/push/register";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly channel: {
                    readonly enum: readonly ["app", "k2-companion"];
                };
                readonly configured: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly apns: {
                            readonly type: "boolean";
                        };
                        readonly fcm: {
                            readonly type: "boolean";
                        };
                    };
                    readonly type: "object";
                };
                readonly deviceId: {
                    readonly type: "string";
                };
                readonly environment: {
                    readonly enum: readonly ["production", "sandbox"];
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly lastSeenAt: {
                    readonly type: "integer";
                };
                readonly mutedTopics: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly platform: {
                    readonly enum: readonly ["apns", "fcm"];
                };
                readonly previews: {
                    readonly type: readonly ["boolean", "null"];
                };
                readonly who: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'push.unregister': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "custom-app-push-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/push/unregister";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly removed: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'push.device': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "custom-app-push-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/push/device";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly channel: {
                    readonly enum: readonly ["app", "k2-companion"];
                };
                readonly configured: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly apns: {
                            readonly type: "boolean";
                        };
                        readonly fcm: {
                            readonly type: "boolean";
                        };
                    };
                    readonly type: "object";
                };
                readonly deviceId: {
                    readonly type: "string";
                };
                readonly environment: {
                    readonly enum: readonly ["production", "sandbox"];
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly lastSeenAt: {
                    readonly type: "integer";
                };
                readonly mutedTopics: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly platform: {
                    readonly enum: readonly ["apns", "fcm"];
                };
                readonly previews: {
                    readonly type: readonly ["boolean", "null"];
                };
                readonly who: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'push.prefs': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "custom-app-push-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/push/prefs";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly channel: {
                    readonly enum: readonly ["app", "k2-companion"];
                };
                readonly configured: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly apns: {
                            readonly type: "boolean";
                        };
                        readonly fcm: {
                            readonly type: "boolean";
                        };
                    };
                    readonly type: "object";
                };
                readonly deviceId: {
                    readonly type: "string";
                };
                readonly environment: {
                    readonly enum: readonly ["production", "sandbox"];
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly lastSeenAt: {
                    readonly type: "integer";
                };
                readonly mutedTopics: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly platform: {
                    readonly enum: readonly ["apns", "fcm"];
                };
                readonly previews: {
                    readonly type: readonly ["boolean", "null"];
                };
                readonly who: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'push.send': {
        readonly cap: "push:send";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "custom-app-push-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/push/send";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly queued: {
                    readonly type: "integer";
                };
                readonly sendId: {
                    readonly type: "string";
                };
                readonly skipped: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly muted_topic: {
                            readonly type: "integer";
                        };
                        readonly no_devices: {
                            readonly type: "integer";
                        };
                        readonly not_a_guest: {
                            readonly type: "integer";
                        };
                        readonly platform_not_configured: {
                            readonly type: "integer";
                        };
                        readonly rate_limited: {
                            readonly type: "integer";
                        };
                    };
                    readonly type: "object";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'push.status': {
        readonly cap: "push:send";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "custom-app-push-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/push/status";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly at: {
                    readonly type: "integer";
                };
                readonly dead: {
                    readonly type: "integer";
                };
                readonly dropped: {
                    readonly type: "integer";
                };
                readonly failed: {
                    readonly type: "integer";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly queued: {
                    readonly type: "integer";
                };
                readonly sendId: {
                    readonly type: "string";
                };
                readonly sent: {
                    readonly type: "integer";
                };
                readonly skipped: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly muted_topic: {
                            readonly type: "integer";
                        };
                        readonly no_devices: {
                            readonly type: "integer";
                        };
                        readonly not_a_guest: {
                            readonly type: "integer";
                        };
                        readonly platform_not_configured: {
                            readonly type: "integer";
                        };
                        readonly rate_limited: {
                            readonly type: "integer";
                        };
                    };
                    readonly type: "object";
                };
                readonly wrongApp: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'push.devices': {
        readonly cap: "push:send";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "custom-app-push-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/push/devices";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly guests: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly devices: {
                                readonly type: "integer";
                            };
                            readonly guest: {
                                readonly type: "string";
                            };
                            readonly platforms: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'pairing.start': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-phone-pairing-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/pair/start";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly code: {
                    readonly type: "string";
                };
                readonly expiresAt: {
                    readonly type: "integer";
                };
                readonly expiresInSecs: {
                    readonly type: "integer";
                };
                readonly host: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly pairUrl: {
                    readonly type: "string";
                };
                readonly pairingId: {
                    readonly type: "string";
                };
                readonly qr: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly rows: {
                            readonly items: {
                                readonly type: "string";
                            };
                            readonly type: "array";
                        };
                        readonly size: {
                            readonly type: "integer";
                        };
                    };
                    readonly type: "object";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'pairing.status': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-phone-pairing-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/pair/status";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly device: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly deviceLoginId: {
                            readonly type: "string";
                        };
                        readonly name: {
                            readonly type: "string";
                        };
                        readonly pairedAt: {
                            readonly type: "integer";
                        };
                    };
                    readonly type: "object";
                };
                readonly expiresInSecs: {
                    readonly type: "integer";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly pairingId: {
                    readonly type: "string";
                };
                readonly state: {
                    readonly enum: readonly ["pending", "paired", "expired", "cancelled"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'pairing.cancel': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-phone-pairing-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/pair/cancel";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly cancelled: {
                    readonly type: "boolean";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'pairing.check': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-phone-pairing-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/pair/check";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly appName: {
                    readonly type: "string";
                };
                readonly expiresInSecs: {
                    readonly type: "integer";
                };
                readonly fullName: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly username: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'pairing.redeem': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-phone-pairing-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/pair";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly pass: "token";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly caps: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly device: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly id: {
                            readonly type: "string";
                        };
                        readonly name: {
                            readonly type: "string";
                        };
                    };
                    readonly type: "object";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly pass: {
                    readonly type: "string";
                };
                readonly passExpiresAt: {
                    readonly type: "integer";
                };
                readonly principalId: {
                    readonly type: "string";
                };
                readonly push: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly code: {
                            readonly type: "string";
                        };
                        readonly configured: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly apns: {
                                    readonly type: "boolean";
                                };
                                readonly fcm: {
                                    readonly type: "boolean";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly id: {
                            readonly type: "string";
                        };
                        readonly registered: {
                            readonly type: "boolean";
                        };
                        readonly who: {
                            readonly type: "string";
                        };
                    };
                    readonly type: "object";
                };
                readonly refresh: {
                    readonly type: "string";
                };
                readonly refreshExpiresAt: {
                    readonly type: "integer";
                };
                readonly role: {
                    readonly type: readonly ["string", "null"];
                };
                readonly roomAccess: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly caps: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                            readonly handle: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly roomHandles: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly rooms: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly username: {
                    readonly type: "string";
                };
                readonly via: {
                    readonly enum: readonly ["pairing"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'email.send': {
        readonly cap: "email:send";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "email-notifications-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/email/send";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly recipients: {
                    readonly type: "integer";
                };
                readonly status: {
                    readonly enum: readonly ["submitted", "scheduled", "held"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'email.status': {
        readonly cap: "email:send";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "email-notifications-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/email/status";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly createdAt: {
                    readonly type: "integer";
                };
                readonly error: {
                    readonly type: "string";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly recipients: {
                    readonly type: "integer";
                };
                readonly sendAt: {
                    readonly type: "integer";
                };
                readonly status: {
                    readonly enum: readonly ["queued", "scheduled", "held", "submitted", "failed", "refused", "cancelled"];
                };
                readonly updatedAt: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'email.log': {
        readonly cap: "email:send";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "email-notifications-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/email/log";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly items: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly createdAt: {
                                readonly type: "integer";
                            };
                            readonly error: {
                                readonly type: "string";
                            };
                            readonly from: {
                                readonly type: "string";
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly origin: {
                                readonly type: "string";
                            };
                            readonly purpose: {
                                readonly type: "string";
                            };
                            readonly status: {
                                readonly type: "string";
                            };
                            readonly subject: {
                                readonly type: "string";
                            };
                            readonly tags: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                            readonly to: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly next: {
                    readonly type: "integer";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'app.info': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-mobile-access-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/app-info";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly kind: {
                    readonly type: "string";
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly signIn: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly methods: {
                            readonly items: {
                                readonly type: "string";
                            };
                            readonly type: "array";
                        };
                        readonly staysSignedIn: {
                            readonly type: "boolean";
                        };
                        readonly url: {
                            readonly type: readonly ["string", "null"];
                        };
                    };
                    readonly type: "object";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'password.forgot': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-mobile-access-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/password/forgot";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'password.reset': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-mobile-access-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/password/reset";
            readonly params: {
                readonly resetToken: "token";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.page': {
        readonly cap: "thread:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/thread";
            readonly params: {
                readonly afterSeq: "after_seq";
                readonly beforeSeq: "before_seq";
                readonly sinceSeq: "since_seq";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly conversation: "conversation_id";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly addr: {
                    readonly type: "string";
                };
                readonly after_seq: {
                    readonly type: "integer";
                };
                readonly changed: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly collection: {
                                readonly type: "string";
                            };
                            readonly doc: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly attachments: {
                                        readonly items: {
                                            readonly additionalProperties: false;
                                            readonly properties: {
                                                readonly canDownload: {
                                                    readonly type: "boolean";
                                                };
                                                readonly kind: {
                                                    readonly type: "string";
                                                };
                                                readonly line: {
                                                    readonly type: "integer";
                                                };
                                                readonly lineEnd: {
                                                    readonly type: "integer";
                                                };
                                                readonly mime: {
                                                    readonly type: "string";
                                                };
                                                readonly modifiedAt: {
                                                    readonly type: "integer";
                                                };
                                                readonly name: {
                                                    readonly type: "string";
                                                };
                                                readonly size: {
                                                    readonly type: "integer";
                                                };
                                                readonly state: {
                                                    readonly type: "string";
                                                };
                                            };
                                            readonly type: "object";
                                        };
                                        readonly type: "array";
                                    };
                                    readonly author: {
                                        readonly additionalProperties: false;
                                        readonly properties: {
                                            readonly kind: {
                                                readonly type: "string";
                                            };
                                            readonly login: {
                                                readonly type: "string";
                                            };
                                            readonly name: {
                                                readonly type: "string";
                                            };
                                        };
                                        readonly type: "object";
                                    };
                                    readonly body: {
                                        readonly type: "string";
                                    };
                                    readonly choice: {
                                        readonly additionalProperties: false;
                                        readonly properties: {
                                            readonly allow_custom: {
                                                readonly type: "boolean";
                                            };
                                            readonly answer: {
                                                readonly type: "string";
                                            };
                                            readonly answeredAt: {
                                                readonly type: "integer";
                                            };
                                            readonly answeredBy: {
                                                readonly additionalProperties: false;
                                                readonly properties: {
                                                    readonly kind: {
                                                        readonly type: "string";
                                                    };
                                                    readonly login: {
                                                        readonly type: "string";
                                                    };
                                                    readonly name: {
                                                        readonly type: "string";
                                                    };
                                                };
                                                readonly type: "object";
                                            };
                                            readonly expiresAt: {
                                                readonly type: "integer";
                                            };
                                            readonly options: {
                                                readonly items: {
                                                    readonly additionalProperties: false;
                                                    readonly properties: {
                                                        readonly label: {
                                                            readonly type: "string";
                                                        };
                                                    };
                                                    readonly type: "object";
                                                };
                                                readonly type: "array";
                                            };
                                            readonly prompt: {
                                                readonly type: "string";
                                            };
                                            readonly status: {
                                                readonly type: "string";
                                            };
                                        };
                                        readonly type: "object";
                                    };
                                    readonly created_at: {
                                        readonly type: "number";
                                    };
                                    readonly from: {
                                        readonly type: "string";
                                    };
                                    readonly id: {
                                        readonly type: "string";
                                    };
                                    readonly kind: {
                                        readonly type: "string";
                                    };
                                    readonly secret: {
                                        readonly additionalProperties: false;
                                        readonly properties: {
                                            readonly answeredAt: {
                                                readonly type: "integer";
                                            };
                                            readonly answeredBy: {
                                                readonly additionalProperties: false;
                                                readonly properties: {
                                                    readonly kind: {
                                                        readonly type: "string";
                                                    };
                                                    readonly login: {
                                                        readonly type: "string";
                                                    };
                                                    readonly name: {
                                                        readonly type: "string";
                                                    };
                                                };
                                                readonly type: "object";
                                            };
                                            readonly delivery: {
                                                readonly type: "string";
                                            };
                                            readonly expiresAt: {
                                                readonly type: "integer";
                                            };
                                            readonly hosts: {
                                                readonly items: {
                                                    readonly type: "string";
                                                };
                                                readonly type: "array";
                                            };
                                            readonly name: {
                                                readonly type: "string";
                                            };
                                            readonly prompt: {
                                                readonly type: "string";
                                            };
                                            readonly ref: {
                                                readonly type: "string";
                                            };
                                            readonly scope: {
                                                readonly type: "string";
                                            };
                                            readonly secretKind: {
                                                readonly type: "string";
                                            };
                                            readonly status: {
                                                readonly type: "string";
                                            };
                                        };
                                        readonly type: "object";
                                    };
                                    readonly to: {
                                        readonly type: "string";
                                    };
                                    readonly via: {
                                        readonly type: "string";
                                    };
                                    readonly widget: {
                                        readonly additionalProperties: false;
                                        readonly properties: {
                                            readonly garden: {
                                                readonly type: "string";
                                            };
                                            readonly widget: {
                                                readonly type: "string";
                                            };
                                        };
                                        readonly type: "object";
                                    };
                                };
                                readonly type: "object";
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly seq: {
                                readonly type: "integer";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly collection: {
                    readonly type: "string";
                };
                readonly conversation: {
                    readonly type: "string";
                };
                readonly has_more: {
                    readonly type: "boolean";
                };
                readonly items: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly collection: {
                                readonly type: "string";
                            };
                            readonly doc: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly attachments: {
                                        readonly items: {
                                            readonly additionalProperties: false;
                                            readonly properties: {
                                                readonly canDownload: {
                                                    readonly type: "boolean";
                                                };
                                                readonly kind: {
                                                    readonly type: "string";
                                                };
                                                readonly line: {
                                                    readonly type: "integer";
                                                };
                                                readonly lineEnd: {
                                                    readonly type: "integer";
                                                };
                                                readonly mime: {
                                                    readonly type: "string";
                                                };
                                                readonly modifiedAt: {
                                                    readonly type: "integer";
                                                };
                                                readonly name: {
                                                    readonly type: "string";
                                                };
                                                readonly size: {
                                                    readonly type: "integer";
                                                };
                                                readonly state: {
                                                    readonly type: "string";
                                                };
                                            };
                                            readonly type: "object";
                                        };
                                        readonly type: "array";
                                    };
                                    readonly author: {
                                        readonly additionalProperties: false;
                                        readonly properties: {
                                            readonly kind: {
                                                readonly type: "string";
                                            };
                                            readonly login: {
                                                readonly type: "string";
                                            };
                                            readonly name: {
                                                readonly type: "string";
                                            };
                                        };
                                        readonly type: "object";
                                    };
                                    readonly body: {
                                        readonly type: "string";
                                    };
                                    readonly choice: {
                                        readonly additionalProperties: false;
                                        readonly properties: {
                                            readonly allow_custom: {
                                                readonly type: "boolean";
                                            };
                                            readonly answer: {
                                                readonly type: "string";
                                            };
                                            readonly answeredAt: {
                                                readonly type: "integer";
                                            };
                                            readonly answeredBy: {
                                                readonly additionalProperties: false;
                                                readonly properties: {
                                                    readonly kind: {
                                                        readonly type: "string";
                                                    };
                                                    readonly login: {
                                                        readonly type: "string";
                                                    };
                                                    readonly name: {
                                                        readonly type: "string";
                                                    };
                                                };
                                                readonly type: "object";
                                            };
                                            readonly expiresAt: {
                                                readonly type: "integer";
                                            };
                                            readonly options: {
                                                readonly items: {
                                                    readonly additionalProperties: false;
                                                    readonly properties: {
                                                        readonly label: {
                                                            readonly type: "string";
                                                        };
                                                    };
                                                    readonly type: "object";
                                                };
                                                readonly type: "array";
                                            };
                                            readonly prompt: {
                                                readonly type: "string";
                                            };
                                            readonly status: {
                                                readonly type: "string";
                                            };
                                        };
                                        readonly type: "object";
                                    };
                                    readonly created_at: {
                                        readonly type: "number";
                                    };
                                    readonly from: {
                                        readonly type: "string";
                                    };
                                    readonly id: {
                                        readonly type: "string";
                                    };
                                    readonly kind: {
                                        readonly type: "string";
                                    };
                                    readonly secret: {
                                        readonly additionalProperties: false;
                                        readonly properties: {
                                            readonly answeredAt: {
                                                readonly type: "integer";
                                            };
                                            readonly answeredBy: {
                                                readonly additionalProperties: false;
                                                readonly properties: {
                                                    readonly kind: {
                                                        readonly type: "string";
                                                    };
                                                    readonly login: {
                                                        readonly type: "string";
                                                    };
                                                    readonly name: {
                                                        readonly type: "string";
                                                    };
                                                };
                                                readonly type: "object";
                                            };
                                            readonly delivery: {
                                                readonly type: "string";
                                            };
                                            readonly expiresAt: {
                                                readonly type: "integer";
                                            };
                                            readonly hosts: {
                                                readonly items: {
                                                    readonly type: "string";
                                                };
                                                readonly type: "array";
                                            };
                                            readonly name: {
                                                readonly type: "string";
                                            };
                                            readonly prompt: {
                                                readonly type: "string";
                                            };
                                            readonly ref: {
                                                readonly type: "string";
                                            };
                                            readonly scope: {
                                                readonly type: "string";
                                            };
                                            readonly secretKind: {
                                                readonly type: "string";
                                            };
                                            readonly status: {
                                                readonly type: "string";
                                            };
                                        };
                                        readonly type: "object";
                                    };
                                    readonly to: {
                                        readonly type: "string";
                                    };
                                    readonly via: {
                                        readonly type: "string";
                                    };
                                    readonly widget: {
                                        readonly additionalProperties: false;
                                        readonly properties: {
                                            readonly garden: {
                                                readonly type: "string";
                                            };
                                            readonly widget: {
                                                readonly type: "string";
                                            };
                                        };
                                        readonly type: "object";
                                    };
                                };
                                readonly type: "object";
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly seq: {
                                readonly type: "integer";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly pastAddresses: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly removed: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly seq: {
                                readonly type: "integer";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.latest': {
        readonly cap: "thread:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/thread/latest";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly conversation: "conversationId";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly items: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly addr: {
                                readonly type: "string";
                            };
                            readonly at: {
                                readonly type: readonly ["number", "null"];
                            };
                            readonly author: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly kind: {
                                        readonly type: "string";
                                    };
                                    readonly login: {
                                        readonly type: "string";
                                    };
                                    readonly name: {
                                        readonly type: "string";
                                    };
                                };
                                readonly type: readonly ["object", "null"];
                            };
                            readonly conversation: {
                                readonly type: "string";
                            };
                            readonly error: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly code: {
                                        readonly type: "string";
                                    };
                                    readonly hint: {
                                        readonly type: "string";
                                    };
                                };
                                readonly type: "object";
                            };
                            readonly from: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly kind: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly preview: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly seq: {
                                readonly type: readonly ["integer", "null"];
                            };
                            readonly via: {
                                readonly type: readonly ["string", "null"];
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.activity': {
        readonly cap: "activity:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "thread-activity-ear-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/thread/activity";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly conversation: "conversation_id";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly addr: {
                    readonly type: "string";
                };
                readonly conversation: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly turn: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly background: {
                            readonly type: "integer";
                        };
                        readonly childCounts: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly monitor: {
                                    readonly type: "integer";
                                };
                                readonly shell: {
                                    readonly type: "integer";
                                };
                                readonly subagent: {
                                    readonly type: "integer";
                                };
                                readonly task: {
                                    readonly type: "integer";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly end: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly reason: {
                                    readonly type: "string";
                                };
                            };
                            readonly type: readonly ["object", "null"];
                        };
                        readonly evidenceAt: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly messages: {
                            readonly type: "integer";
                        };
                        readonly openBecause: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly phase: {
                            readonly type: "string";
                        };
                        readonly phaseSince: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly rev: {
                            readonly type: "integer";
                        };
                        readonly schema: {
                            readonly type: "integer";
                        };
                        readonly serverNow: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly startedAt: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly state: {
                            readonly type: "string";
                        };
                        readonly stepKind: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly subagents: {
                            readonly type: "integer";
                        };
                        readonly subagentsDone: {
                            readonly type: "integer";
                        };
                        readonly tally: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly cmd: {
                                    readonly type: "integer";
                                };
                                readonly edit: {
                                    readonly type: "integer";
                                };
                                readonly read: {
                                    readonly type: "integer";
                                };
                                readonly search: {
                                    readonly type: "integer";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly turnId: {
                            readonly type: "string";
                        };
                        readonly waitingOn: {
                            readonly type: readonly ["string", "null"];
                        };
                    };
                    readonly type: readonly ["object", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.activityRooms': {
        readonly cap: "activity:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "thread-activity-rooms-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/thread/activity/rooms";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly turns: {
                    readonly type: "json";
                };
                readonly unknown: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.send': {
        readonly cap: "thread:post";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/thread/post";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly conversation: "conversation_id";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly addr: {
                    readonly type: "string";
                };
                readonly author: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly kind: {
                            readonly type: "string";
                        };
                        readonly login: {
                            readonly type: "string";
                        };
                        readonly name: {
                            readonly type: "string";
                        };
                    };
                    readonly type: "object";
                };
                readonly body: {
                    readonly type: readonly ["string", "null"];
                };
                readonly conversation: {
                    readonly type: "string";
                };
                readonly from: {
                    readonly type: "string";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly kind: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly seq: {
                    readonly type: "integer";
                };
                readonly to: {
                    readonly type: readonly ["string", "null"];
                };
                readonly via: {
                    readonly type: readonly ["string", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.answerCard': {
        readonly cap: "thread:post";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/thread/answer";
            readonly params: {
                readonly cardId: "id";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly conversation: "conversation_id";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly addr: {
                    readonly type: "string";
                };
                readonly agentCanSee: {
                    readonly type: "boolean";
                };
                readonly answer: {
                    readonly type: "string";
                };
                readonly conversation: {
                    readonly type: "string";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly kind: {
                    readonly type: "string";
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly ref: {
                    readonly type: "string";
                };
                readonly seq: {
                    readonly type: "integer";
                };
                readonly status: {
                    readonly type: "string";
                };
                readonly trimmed: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.voidCard': {
        readonly cap: "thread:post";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/thread/void";
            readonly params: {
                readonly cardId: "id";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly conversation: "conversation_id";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly addr: {
                    readonly type: "string";
                };
                readonly conversation: {
                    readonly type: "string";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly kind: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly seq: {
                    readonly type: "integer";
                };
                readonly status: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.events': {
        readonly cap: "thread:read";
        readonly effect: "read";
        readonly kind: "subscribe";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: null;
        readonly socket: {
            readonly path: "/cli/overlay/events";
            readonly param: "conversation";
            readonly frame: "thread";
            readonly refetch: "thread.page";
            readonly query: {};
        };
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly collection: {
                    readonly type: "string";
                };
                readonly doc: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly attachments: {
                            readonly items: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly canDownload: {
                                        readonly type: "boolean";
                                    };
                                    readonly kind: {
                                        readonly type: "string";
                                    };
                                    readonly line: {
                                        readonly type: "integer";
                                    };
                                    readonly lineEnd: {
                                        readonly type: "integer";
                                    };
                                    readonly mime: {
                                        readonly type: "string";
                                    };
                                    readonly modifiedAt: {
                                        readonly type: "integer";
                                    };
                                    readonly name: {
                                        readonly type: "string";
                                    };
                                    readonly size: {
                                        readonly type: "integer";
                                    };
                                    readonly state: {
                                        readonly type: "string";
                                    };
                                };
                                readonly type: "object";
                            };
                            readonly type: "array";
                        };
                        readonly author: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly kind: {
                                    readonly type: "string";
                                };
                                readonly login: {
                                    readonly type: "string";
                                };
                                readonly name: {
                                    readonly type: "string";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly body: {
                            readonly type: "string";
                        };
                        readonly choice: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly allow_custom: {
                                    readonly type: "boolean";
                                };
                                readonly answer: {
                                    readonly type: "string";
                                };
                                readonly answeredAt: {
                                    readonly type: "integer";
                                };
                                readonly answeredBy: {
                                    readonly additionalProperties: false;
                                    readonly properties: {
                                        readonly kind: {
                                            readonly type: "string";
                                        };
                                        readonly login: {
                                            readonly type: "string";
                                        };
                                        readonly name: {
                                            readonly type: "string";
                                        };
                                    };
                                    readonly type: "object";
                                };
                                readonly expiresAt: {
                                    readonly type: "integer";
                                };
                                readonly options: {
                                    readonly items: {
                                        readonly additionalProperties: false;
                                        readonly properties: {
                                            readonly label: {
                                                readonly type: "string";
                                            };
                                        };
                                        readonly type: "object";
                                    };
                                    readonly type: "array";
                                };
                                readonly prompt: {
                                    readonly type: "string";
                                };
                                readonly status: {
                                    readonly type: "string";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly created_at: {
                            readonly type: "number";
                        };
                        readonly from: {
                            readonly type: "string";
                        };
                        readonly id: {
                            readonly type: "string";
                        };
                        readonly kind: {
                            readonly type: "string";
                        };
                        readonly secret: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly answeredAt: {
                                    readonly type: "integer";
                                };
                                readonly answeredBy: {
                                    readonly additionalProperties: false;
                                    readonly properties: {
                                        readonly kind: {
                                            readonly type: "string";
                                        };
                                        readonly login: {
                                            readonly type: "string";
                                        };
                                        readonly name: {
                                            readonly type: "string";
                                        };
                                    };
                                    readonly type: "object";
                                };
                                readonly delivery: {
                                    readonly type: "string";
                                };
                                readonly expiresAt: {
                                    readonly type: "integer";
                                };
                                readonly hosts: {
                                    readonly items: {
                                        readonly type: "string";
                                    };
                                    readonly type: "array";
                                };
                                readonly name: {
                                    readonly type: "string";
                                };
                                readonly prompt: {
                                    readonly type: "string";
                                };
                                readonly ref: {
                                    readonly type: "string";
                                };
                                readonly scope: {
                                    readonly type: "string";
                                };
                                readonly secretKind: {
                                    readonly type: "string";
                                };
                                readonly status: {
                                    readonly type: "string";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly to: {
                            readonly type: "string";
                        };
                        readonly via: {
                            readonly type: "string";
                        };
                        readonly widget: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly garden: {
                                    readonly type: "string";
                                };
                                readonly widget: {
                                    readonly type: "string";
                                };
                            };
                            readonly type: "object";
                        };
                    };
                    readonly type: "object";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly seq: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.stream': {
        readonly cap: "thread:read";
        readonly effect: "read";
        readonly kind: "subscribe";
        readonly feature: "thread-stream-v1";
        readonly door: "both";
        readonly http: null;
        readonly socket: {
            readonly path: "/cli/thread/stream";
            readonly param: "rooms";
            readonly frame: "thread_message";
            readonly refetch: "thread.page";
            readonly query: {
                readonly ear: "1";
            };
        };
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly activity: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly background: {
                            readonly type: "integer";
                        };
                        readonly childCounts: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly monitor: {
                                    readonly type: "integer";
                                };
                                readonly shell: {
                                    readonly type: "integer";
                                };
                                readonly subagent: {
                                    readonly type: "integer";
                                };
                                readonly task: {
                                    readonly type: "integer";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly end: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly reason: {
                                    readonly type: "string";
                                };
                            };
                            readonly type: readonly ["object", "null"];
                        };
                        readonly evidenceAt: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly messages: {
                            readonly type: "integer";
                        };
                        readonly openBecause: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly phase: {
                            readonly type: "string";
                        };
                        readonly phaseSince: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly rev: {
                            readonly type: "integer";
                        };
                        readonly schema: {
                            readonly type: "integer";
                        };
                        readonly serverNow: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly startedAt: {
                            readonly type: readonly ["integer", "null"];
                        };
                        readonly state: {
                            readonly type: "string";
                        };
                        readonly stepKind: {
                            readonly type: readonly ["string", "null"];
                        };
                        readonly subagents: {
                            readonly type: "integer";
                        };
                        readonly subagentsDone: {
                            readonly type: "integer";
                        };
                        readonly tally: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly cmd: {
                                    readonly type: "integer";
                                };
                                readonly edit: {
                                    readonly type: "integer";
                                };
                                readonly read: {
                                    readonly type: "integer";
                                };
                                readonly search: {
                                    readonly type: "integer";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly turnId: {
                            readonly type: "string";
                        };
                        readonly waitingOn: {
                            readonly type: readonly ["string", "null"];
                        };
                    };
                    readonly type: "object";
                };
                readonly all: {
                    readonly type: "boolean";
                };
                readonly bodyBytes: {
                    readonly type: "integer";
                };
                readonly code: {
                    readonly type: "string";
                };
                readonly collection: {
                    readonly type: "string";
                };
                readonly conversation: {
                    readonly type: "string";
                };
                readonly conversations: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly doc: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly attachments: {
                            readonly items: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly canDownload: {
                                        readonly type: "boolean";
                                    };
                                    readonly kind: {
                                        readonly type: "string";
                                    };
                                    readonly line: {
                                        readonly type: "integer";
                                    };
                                    readonly lineEnd: {
                                        readonly type: "integer";
                                    };
                                    readonly mime: {
                                        readonly type: "string";
                                    };
                                    readonly modifiedAt: {
                                        readonly type: "integer";
                                    };
                                    readonly name: {
                                        readonly type: "string";
                                    };
                                    readonly size: {
                                        readonly type: "integer";
                                    };
                                    readonly state: {
                                        readonly type: "string";
                                    };
                                };
                                readonly type: "object";
                            };
                            readonly type: "array";
                        };
                        readonly author: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly kind: {
                                    readonly type: "string";
                                };
                                readonly login: {
                                    readonly type: "string";
                                };
                                readonly name: {
                                    readonly type: "string";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly body: {
                            readonly type: "string";
                        };
                        readonly choice: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly allow_custom: {
                                    readonly type: "boolean";
                                };
                                readonly answer: {
                                    readonly type: "string";
                                };
                                readonly answeredAt: {
                                    readonly type: "integer";
                                };
                                readonly answeredBy: {
                                    readonly additionalProperties: false;
                                    readonly properties: {
                                        readonly kind: {
                                            readonly type: "string";
                                        };
                                        readonly login: {
                                            readonly type: "string";
                                        };
                                        readonly name: {
                                            readonly type: "string";
                                        };
                                    };
                                    readonly type: "object";
                                };
                                readonly expiresAt: {
                                    readonly type: "integer";
                                };
                                readonly options: {
                                    readonly items: {
                                        readonly additionalProperties: false;
                                        readonly properties: {
                                            readonly label: {
                                                readonly type: "string";
                                            };
                                        };
                                        readonly type: "object";
                                    };
                                    readonly type: "array";
                                };
                                readonly prompt: {
                                    readonly type: "string";
                                };
                                readonly status: {
                                    readonly type: "string";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly created_at: {
                            readonly type: "number";
                        };
                        readonly from: {
                            readonly type: "string";
                        };
                        readonly id: {
                            readonly type: "string";
                        };
                        readonly kind: {
                            readonly type: "string";
                        };
                        readonly secret: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly answeredAt: {
                                    readonly type: "integer";
                                };
                                readonly answeredBy: {
                                    readonly additionalProperties: false;
                                    readonly properties: {
                                        readonly kind: {
                                            readonly type: "string";
                                        };
                                        readonly login: {
                                            readonly type: "string";
                                        };
                                        readonly name: {
                                            readonly type: "string";
                                        };
                                    };
                                    readonly type: "object";
                                };
                                readonly delivery: {
                                    readonly type: "string";
                                };
                                readonly expiresAt: {
                                    readonly type: "integer";
                                };
                                readonly hosts: {
                                    readonly items: {
                                        readonly type: "string";
                                    };
                                    readonly type: "array";
                                };
                                readonly name: {
                                    readonly type: "string";
                                };
                                readonly prompt: {
                                    readonly type: "string";
                                };
                                readonly ref: {
                                    readonly type: "string";
                                };
                                readonly scope: {
                                    readonly type: "string";
                                };
                                readonly secretKind: {
                                    readonly type: "string";
                                };
                                readonly status: {
                                    readonly type: "string";
                                };
                            };
                            readonly type: "object";
                        };
                        readonly to: {
                            readonly type: "string";
                        };
                        readonly via: {
                            readonly type: "string";
                        };
                        readonly widget: {
                            readonly additionalProperties: false;
                            readonly properties: {
                                readonly garden: {
                                    readonly type: "string";
                                };
                                readonly widget: {
                                    readonly type: "string";
                                };
                            };
                            readonly type: "object";
                        };
                    };
                    readonly type: "object";
                };
                readonly hint: {
                    readonly type: "string";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly kind: {
                    readonly enum: readonly ["inbox_subscribed", "thread_message", "thread_activity", "inbox_error", "inbox_reset", "activity_reset"];
                };
                readonly limit: {
                    readonly type: "integer";
                };
                readonly max: {
                    readonly type: "integer";
                };
                readonly reason: {
                    readonly type: "string";
                };
                readonly resolved: {
                    readonly additionalProperties: {
                        readonly type: "string";
                    };
                    readonly type: "object";
                };
                readonly rev: {
                    readonly type: "integer";
                };
                readonly room: {
                    readonly type: "string";
                };
                readonly rooms: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly seq: {
                    readonly type: "integer";
                };
                readonly truncated: {
                    readonly type: "boolean";
                };
                readonly turns: {
                    readonly type: "json";
                };
                readonly unknown: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.attachmentGet': {
        readonly cap: "thread:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "thread-attachments-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/thread/attachment";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly base64: {
                    readonly type: "string";
                };
                readonly eof: {
                    readonly type: "boolean";
                };
                readonly height: {
                    readonly type: "integer";
                };
                readonly kind: {
                    readonly type: "string";
                };
                readonly len: {
                    readonly type: "integer";
                };
                readonly line: {
                    readonly type: "integer";
                };
                readonly lineEnd: {
                    readonly type: "integer";
                };
                readonly mime: {
                    readonly type: "string";
                };
                readonly modifiedAt: {
                    readonly type: "integer";
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly offset: {
                    readonly type: "integer";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly sanitized: {
                    readonly type: "boolean";
                };
                readonly size: {
                    readonly type: "integer";
                };
                readonly state: {
                    readonly type: "string";
                };
                readonly text: {
                    readonly type: "string";
                };
                readonly truncated: {
                    readonly type: "boolean";
                };
                readonly width: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.catchup': {
        readonly cap: "thread:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "inbox-catchup-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/thread/catchup";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly conversation: "conversation_id";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly has_more: {
                    readonly type: "boolean";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly threads: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly addr: {
                                readonly type: "string";
                            };
                            readonly after_seq: {
                                readonly type: "integer";
                            };
                            readonly changed: {
                                readonly items: {
                                    readonly additionalProperties: false;
                                    readonly properties: {
                                        readonly collection: {
                                            readonly type: "string";
                                        };
                                        readonly doc: {
                                            readonly additionalProperties: false;
                                            readonly properties: {
                                                readonly attachments: {
                                                    readonly items: {
                                                        readonly additionalProperties: false;
                                                        readonly properties: {
                                                            readonly canDownload: {
                                                                readonly type: "boolean";
                                                            };
                                                            readonly kind: {
                                                                readonly type: "string";
                                                            };
                                                            readonly line: {
                                                                readonly type: "integer";
                                                            };
                                                            readonly lineEnd: {
                                                                readonly type: "integer";
                                                            };
                                                            readonly mime: {
                                                                readonly type: "string";
                                                            };
                                                            readonly modifiedAt: {
                                                                readonly type: "integer";
                                                            };
                                                            readonly name: {
                                                                readonly type: "string";
                                                            };
                                                            readonly size: {
                                                                readonly type: "integer";
                                                            };
                                                            readonly state: {
                                                                readonly type: "string";
                                                            };
                                                        };
                                                        readonly type: "object";
                                                    };
                                                    readonly type: "array";
                                                };
                                                readonly author: {
                                                    readonly additionalProperties: false;
                                                    readonly properties: {
                                                        readonly kind: {
                                                            readonly type: "string";
                                                        };
                                                        readonly login: {
                                                            readonly type: "string";
                                                        };
                                                        readonly name: {
                                                            readonly type: "string";
                                                        };
                                                    };
                                                    readonly type: "object";
                                                };
                                                readonly body: {
                                                    readonly type: "string";
                                                };
                                                readonly choice: {
                                                    readonly additionalProperties: false;
                                                    readonly properties: {
                                                        readonly allow_custom: {
                                                            readonly type: "boolean";
                                                        };
                                                        readonly answer: {
                                                            readonly type: "string";
                                                        };
                                                        readonly answeredAt: {
                                                            readonly type: "integer";
                                                        };
                                                        readonly answeredBy: {
                                                            readonly additionalProperties: false;
                                                            readonly properties: {
                                                                readonly kind: {
                                                                    readonly type: "string";
                                                                };
                                                                readonly login: {
                                                                    readonly type: "string";
                                                                };
                                                                readonly name: {
                                                                    readonly type: "string";
                                                                };
                                                            };
                                                            readonly type: "object";
                                                        };
                                                        readonly expiresAt: {
                                                            readonly type: "integer";
                                                        };
                                                        readonly options: {
                                                            readonly items: {
                                                                readonly additionalProperties: false;
                                                                readonly properties: {
                                                                    readonly label: {
                                                                        readonly type: "string";
                                                                    };
                                                                };
                                                                readonly type: "object";
                                                            };
                                                            readonly type: "array";
                                                        };
                                                        readonly prompt: {
                                                            readonly type: "string";
                                                        };
                                                        readonly status: {
                                                            readonly type: "string";
                                                        };
                                                    };
                                                    readonly type: "object";
                                                };
                                                readonly created_at: {
                                                    readonly type: "number";
                                                };
                                                readonly from: {
                                                    readonly type: "string";
                                                };
                                                readonly id: {
                                                    readonly type: "string";
                                                };
                                                readonly kind: {
                                                    readonly type: "string";
                                                };
                                                readonly secret: {
                                                    readonly additionalProperties: false;
                                                    readonly properties: {
                                                        readonly answeredAt: {
                                                            readonly type: "integer";
                                                        };
                                                        readonly answeredBy: {
                                                            readonly additionalProperties: false;
                                                            readonly properties: {
                                                                readonly kind: {
                                                                    readonly type: "string";
                                                                };
                                                                readonly login: {
                                                                    readonly type: "string";
                                                                };
                                                                readonly name: {
                                                                    readonly type: "string";
                                                                };
                                                            };
                                                            readonly type: "object";
                                                        };
                                                        readonly delivery: {
                                                            readonly type: "string";
                                                        };
                                                        readonly expiresAt: {
                                                            readonly type: "integer";
                                                        };
                                                        readonly hosts: {
                                                            readonly items: {
                                                                readonly type: "string";
                                                            };
                                                            readonly type: "array";
                                                        };
                                                        readonly name: {
                                                            readonly type: "string";
                                                        };
                                                        readonly prompt: {
                                                            readonly type: "string";
                                                        };
                                                        readonly ref: {
                                                            readonly type: "string";
                                                        };
                                                        readonly scope: {
                                                            readonly type: "string";
                                                        };
                                                        readonly secretKind: {
                                                            readonly type: "string";
                                                        };
                                                        readonly status: {
                                                            readonly type: "string";
                                                        };
                                                    };
                                                    readonly type: "object";
                                                };
                                                readonly to: {
                                                    readonly type: "string";
                                                };
                                                readonly via: {
                                                    readonly type: "string";
                                                };
                                                readonly widget: {
                                                    readonly additionalProperties: false;
                                                    readonly properties: {
                                                        readonly garden: {
                                                            readonly type: "string";
                                                        };
                                                        readonly widget: {
                                                            readonly type: "string";
                                                        };
                                                    };
                                                    readonly type: "object";
                                                };
                                            };
                                            readonly type: "object";
                                        };
                                        readonly id: {
                                            readonly type: "string";
                                        };
                                        readonly seq: {
                                            readonly type: "integer";
                                        };
                                    };
                                    readonly type: "object";
                                };
                                readonly type: "array";
                            };
                            readonly conversation: {
                                readonly type: "string";
                            };
                            readonly deferred: {
                                readonly type: "boolean";
                            };
                            readonly error: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly code: {
                                        readonly type: "string";
                                    };
                                    readonly hint: {
                                        readonly type: "string";
                                    };
                                };
                                readonly type: "object";
                            };
                            readonly has_more: {
                                readonly type: "boolean";
                            };
                            readonly items: {
                                readonly items: {
                                    readonly additionalProperties: false;
                                    readonly properties: {
                                        readonly collection: {
                                            readonly type: "string";
                                        };
                                        readonly doc: {
                                            readonly additionalProperties: false;
                                            readonly properties: {
                                                readonly attachments: {
                                                    readonly items: {
                                                        readonly additionalProperties: false;
                                                        readonly properties: {
                                                            readonly canDownload: {
                                                                readonly type: "boolean";
                                                            };
                                                            readonly kind: {
                                                                readonly type: "string";
                                                            };
                                                            readonly line: {
                                                                readonly type: "integer";
                                                            };
                                                            readonly lineEnd: {
                                                                readonly type: "integer";
                                                            };
                                                            readonly mime: {
                                                                readonly type: "string";
                                                            };
                                                            readonly modifiedAt: {
                                                                readonly type: "integer";
                                                            };
                                                            readonly name: {
                                                                readonly type: "string";
                                                            };
                                                            readonly size: {
                                                                readonly type: "integer";
                                                            };
                                                            readonly state: {
                                                                readonly type: "string";
                                                            };
                                                        };
                                                        readonly type: "object";
                                                    };
                                                    readonly type: "array";
                                                };
                                                readonly author: {
                                                    readonly additionalProperties: false;
                                                    readonly properties: {
                                                        readonly kind: {
                                                            readonly type: "string";
                                                        };
                                                        readonly login: {
                                                            readonly type: "string";
                                                        };
                                                        readonly name: {
                                                            readonly type: "string";
                                                        };
                                                    };
                                                    readonly type: "object";
                                                };
                                                readonly body: {
                                                    readonly type: "string";
                                                };
                                                readonly choice: {
                                                    readonly additionalProperties: false;
                                                    readonly properties: {
                                                        readonly allow_custom: {
                                                            readonly type: "boolean";
                                                        };
                                                        readonly answer: {
                                                            readonly type: "string";
                                                        };
                                                        readonly answeredAt: {
                                                            readonly type: "integer";
                                                        };
                                                        readonly answeredBy: {
                                                            readonly additionalProperties: false;
                                                            readonly properties: {
                                                                readonly kind: {
                                                                    readonly type: "string";
                                                                };
                                                                readonly login: {
                                                                    readonly type: "string";
                                                                };
                                                                readonly name: {
                                                                    readonly type: "string";
                                                                };
                                                            };
                                                            readonly type: "object";
                                                        };
                                                        readonly expiresAt: {
                                                            readonly type: "integer";
                                                        };
                                                        readonly options: {
                                                            readonly items: {
                                                                readonly additionalProperties: false;
                                                                readonly properties: {
                                                                    readonly label: {
                                                                        readonly type: "string";
                                                                    };
                                                                };
                                                                readonly type: "object";
                                                            };
                                                            readonly type: "array";
                                                        };
                                                        readonly prompt: {
                                                            readonly type: "string";
                                                        };
                                                        readonly status: {
                                                            readonly type: "string";
                                                        };
                                                    };
                                                    readonly type: "object";
                                                };
                                                readonly created_at: {
                                                    readonly type: "number";
                                                };
                                                readonly from: {
                                                    readonly type: "string";
                                                };
                                                readonly id: {
                                                    readonly type: "string";
                                                };
                                                readonly kind: {
                                                    readonly type: "string";
                                                };
                                                readonly secret: {
                                                    readonly additionalProperties: false;
                                                    readonly properties: {
                                                        readonly answeredAt: {
                                                            readonly type: "integer";
                                                        };
                                                        readonly answeredBy: {
                                                            readonly additionalProperties: false;
                                                            readonly properties: {
                                                                readonly kind: {
                                                                    readonly type: "string";
                                                                };
                                                                readonly login: {
                                                                    readonly type: "string";
                                                                };
                                                                readonly name: {
                                                                    readonly type: "string";
                                                                };
                                                            };
                                                            readonly type: "object";
                                                        };
                                                        readonly delivery: {
                                                            readonly type: "string";
                                                        };
                                                        readonly expiresAt: {
                                                            readonly type: "integer";
                                                        };
                                                        readonly hosts: {
                                                            readonly items: {
                                                                readonly type: "string";
                                                            };
                                                            readonly type: "array";
                                                        };
                                                        readonly name: {
                                                            readonly type: "string";
                                                        };
                                                        readonly prompt: {
                                                            readonly type: "string";
                                                        };
                                                        readonly ref: {
                                                            readonly type: "string";
                                                        };
                                                        readonly scope: {
                                                            readonly type: "string";
                                                        };
                                                        readonly secretKind: {
                                                            readonly type: "string";
                                                        };
                                                        readonly status: {
                                                            readonly type: "string";
                                                        };
                                                    };
                                                    readonly type: "object";
                                                };
                                                readonly to: {
                                                    readonly type: "string";
                                                };
                                                readonly via: {
                                                    readonly type: "string";
                                                };
                                                readonly widget: {
                                                    readonly additionalProperties: false;
                                                    readonly properties: {
                                                        readonly garden: {
                                                            readonly type: "string";
                                                        };
                                                        readonly widget: {
                                                            readonly type: "string";
                                                        };
                                                    };
                                                    readonly type: "object";
                                                };
                                            };
                                            readonly type: "object";
                                        };
                                        readonly id: {
                                            readonly type: "string";
                                        };
                                        readonly seq: {
                                            readonly type: "integer";
                                        };
                                    };
                                    readonly type: "object";
                                };
                                readonly type: "array";
                            };
                            readonly removed: {
                                readonly items: {
                                    readonly additionalProperties: false;
                                    readonly properties: {
                                        readonly id: {
                                            readonly type: "string";
                                        };
                                        readonly seq: {
                                            readonly type: "integer";
                                        };
                                    };
                                    readonly type: "object";
                                };
                                readonly type: "array";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.setRead': {
        readonly cap: "thread:read";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-thread-parity-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/thread/read";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly addr: {
                    readonly type: "string";
                };
                readonly changed: {
                    readonly type: "boolean";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly lastReadSeq: {
                    readonly type: "integer";
                };
                readonly lastSeq: {
                    readonly type: "integer";
                };
                readonly muted: {
                    readonly type: "boolean";
                };
                readonly mutedUntil: {
                    readonly type: "integer";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly total: {
                    readonly type: "integer";
                };
                readonly unread: {
                    readonly type: "integer";
                };
                readonly waiting: {
                    readonly type: "integer";
                };
                readonly waitingTotal: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.markAllRead': {
        readonly cap: "thread:read";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "thread-read-all-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/thread/read-all";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly addr: {
                    readonly type: "string";
                };
                readonly changed: {
                    readonly type: "boolean";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly lastReadSeq: {
                    readonly type: "integer";
                };
                readonly lastSeq: {
                    readonly type: "integer";
                };
                readonly muted: {
                    readonly type: "boolean";
                };
                readonly mutedUntil: {
                    readonly type: "integer";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly total: {
                    readonly type: "integer";
                };
                readonly unread: {
                    readonly type: "integer";
                };
                readonly waiting: {
                    readonly type: "integer";
                };
                readonly waitingTotal: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.unread': {
        readonly cap: "thread:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-thread-parity-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/thread/unread";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly badgeKey: {
                    readonly type: "string";
                };
                readonly byConversation: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly addr: {
                                readonly type: "string";
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly lastReadSeq: {
                                readonly type: "integer";
                            };
                            readonly lastSeq: {
                                readonly type: "integer";
                            };
                            readonly muted: {
                                readonly type: "boolean";
                            };
                            readonly mutedUntil: {
                                readonly type: "integer";
                            };
                            readonly unread: {
                                readonly type: "integer";
                            };
                            readonly waiting: {
                                readonly type: "integer";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly muted: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly addr: {
                                readonly type: "string";
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly mutedUntil: {
                                readonly type: "integer";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly total: {
                    readonly type: "integer";
                };
                readonly waitingTotal: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.mute': {
        readonly cap: "thread:read";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-read-state-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/thread/mute";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly addr: {
                    readonly type: "string";
                };
                readonly changed: {
                    readonly type: "boolean";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly lastReadSeq: {
                    readonly type: "integer";
                };
                readonly lastSeq: {
                    readonly type: "integer";
                };
                readonly muted: {
                    readonly type: "boolean";
                };
                readonly mutedUntil: {
                    readonly type: "integer";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly total: {
                    readonly type: "integer";
                };
                readonly unread: {
                    readonly type: "integer";
                };
                readonly waiting: {
                    readonly type: "integer";
                };
                readonly waitingTotal: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.resolveLinks': {
        readonly cap: "files:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-thread-parity-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/fs/resolve-links";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly links: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly input: {
                                readonly type: "string";
                            };
                            readonly kind: {
                                readonly type: "string";
                            };
                            readonly linkable: {
                                readonly type: "boolean";
                            };
                            readonly rel: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'thread.ensurePinned': {
        readonly cap: "thread:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/workspace/ensure-pinned-chat";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly freshSpawn: {
                    readonly type: "boolean";
                };
                readonly pendingSessionDiscovery: {
                    readonly type: "boolean";
                };
                readonly provider: {
                    readonly type: "string";
                };
                readonly resumedExisting: {
                    readonly type: "boolean";
                };
                readonly reused: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.list': {
        readonly cap: "files:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/fs/read-dir";
            readonly params: {
                readonly relPath: "path";
                readonly showHidden: "show_hidden";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly relPath: "path";
        };
        readonly response: {
            readonly items: {
                readonly additionalProperties: false;
                readonly properties: {
                    readonly isDirectory: {
                        readonly type: "boolean";
                    };
                    readonly modifiedAt: {
                        readonly type: "number";
                    };
                    readonly name: {
                        readonly type: "string";
                    };
                    readonly relPath: {
                        readonly type: "string";
                    };
                    readonly size: {
                        readonly type: "integer";
                    };
                };
                readonly type: "object";
            };
            readonly type: "array";
        };
    };
    readonly 'files.read': {
        readonly cap: "files:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/fs/read-file";
            readonly params: {
                readonly relPath: "path";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly relPath: "path";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly content: {
                    readonly type: "string";
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly relPath: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.readBinary': {
        readonly cap: "files:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/fs/read-binary";
            readonly params: {
                readonly relPath: "path";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly base64: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.readRange': {
        readonly cap: "files:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/fs/read-range";
            readonly params: {
                readonly relPath: "path";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly base64: {
                    readonly type: "string";
                };
                readonly eof: {
                    readonly type: "boolean";
                };
                readonly len: {
                    readonly type: "integer";
                };
                readonly size: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.stat': {
        readonly cap: "files:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "files-path-scope-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/fs/stat";
            readonly params: {
                readonly relPath: "path";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly relPath: "path";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ext: {
                    readonly type: "string";
                };
                readonly kind: {
                    readonly enum: readonly ["file", "directory", "other"];
                };
                readonly mime: {
                    readonly type: "string";
                };
                readonly modifiedAt: {
                    readonly type: "number";
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly relPath: {
                    readonly type: "string";
                };
                readonly size: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.downloads': {
        readonly cap: "files:download";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "files-download-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/fs/downloads";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly items: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly mime: {
                                readonly type: "string";
                            };
                            readonly modifiedAt: {
                                readonly type: "integer";
                            };
                            readonly name: {
                                readonly type: "string";
                            };
                            readonly relPath: {
                                readonly type: "string";
                            };
                            readonly size: {
                                readonly type: "integer";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly truncated: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.download': {
        readonly cap: "files:download";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "files-download-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/fs/download";
            readonly params: {
                readonly relPath: "path";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly base64: {
                    readonly type: "string";
                };
                readonly eof: {
                    readonly type: "boolean";
                };
                readonly len: {
                    readonly type: "integer";
                };
                readonly mime: {
                    readonly type: "string";
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly offset: {
                    readonly type: "integer";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly size: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.write': {
        readonly cap: "files:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/fs/write-file";
            readonly params: {
                readonly relPath: "path";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly success: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.uploadBinary': {
        readonly cap: "files:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/fs/upload-binary";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly relPath: "path";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly relPath: {
                    readonly type: "string";
                };
                readonly size: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.uploadChunk': {
        readonly cap: "files:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/fs/upload-chunk";
            readonly params: {
                readonly isLast: "is_last";
                readonly totalBytes: "total_bytes";
                readonly uploadId: "upload_id";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly relPath: "path";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly done: {
                    readonly type: "boolean";
                };
                readonly received: {
                    readonly type: "integer";
                };
                readonly relPath: {
                    readonly type: "string";
                };
                readonly size: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.create': {
        readonly cap: "files:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/fs/create";
            readonly params: {
                readonly isDirectory: "is_directory";
                readonly relPath: "path";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly success: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.copy': {
        readonly cap: "files:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/fs/copy";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly success: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.move': {
        readonly cap: "files:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/fs/move";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly success: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.rename': {
        readonly cap: "files:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/fs/rename";
            readonly params: {
                readonly newName: "new_name";
                readonly relPath: "path";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {
            readonly relPath: "path";
        };
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly relPath: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'files.events': {
        readonly cap: "files:read";
        readonly effect: "read";
        readonly kind: "subscribe";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: null;
        readonly socket: {
            readonly path: "/cli/fs/events";
            readonly param: "workspace";
            readonly frame: "fs_changed";
            readonly refetch: "files.list";
            readonly query: {};
        };
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly kind: {
                    readonly enum: readonly ["fs_changed", "workspace_resources_changed"];
                };
                readonly paths: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly workspace: {
                    readonly type: "string";
                };
                readonly workspaceId: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'resources.list': {
        readonly cap: "files:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/workspace/resources";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly docs: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly addedAt: {
                                readonly type: "integer";
                            };
                            readonly fileName: {
                                readonly type: "string";
                            };
                            readonly filePath: {
                                readonly type: "string";
                            };
                            readonly missing: {
                                readonly type: "boolean";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly workspaceId: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'resources.add': {
        readonly cap: "files:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/workspace/resources/add";
            readonly params: {
                readonly relPath: "path";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly fileName: {
                    readonly type: "string";
                };
                readonly filePath: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly workspaceId: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'resources.remove': {
        readonly cap: "files:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/workspace/resources/remove";
            readonly params: {
                readonly relPath: "path";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly filePath: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly workspaceId: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'tickets.list': {
        readonly cap: "tickets:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/feedback/list";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly items: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly agentName: {
                                readonly type: "string";
                            };
                            readonly answer: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly answeredAt: {
                                readonly type: readonly ["integer", "null"];
                            };
                            readonly assignees: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                            readonly body: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly briefBytes: {
                                readonly type: readonly ["integer", "null"];
                            };
                            readonly commentCount: {
                                readonly type: "integer";
                            };
                            readonly createdAt: {
                                readonly type: "integer";
                            };
                            readonly hasBrief: {
                                readonly type: "boolean";
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly kind: {
                                readonly type: "string";
                            };
                            readonly options: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: readonly ["array", "null"];
                            };
                            readonly priority: {
                                readonly type: "integer";
                            };
                            readonly projectId: {
                                readonly type: "string";
                            };
                            readonly status: {
                                readonly type: "string";
                            };
                            readonly title: {
                                readonly type: "string";
                            };
                            readonly updatedAt: {
                                readonly type: "integer";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'tickets.show': {
        readonly cap: "tickets:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/feedback/show";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly agentName: {
                    readonly type: "string";
                };
                readonly answer: {
                    readonly type: readonly ["string", "null"];
                };
                readonly answeredAt: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly assignees: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly body: {
                    readonly type: readonly ["string", "null"];
                };
                readonly brief: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly bytes: {
                            readonly type: "integer";
                        };
                        readonly createdAt: {
                            readonly type: "integer";
                        };
                        readonly html: {
                            readonly type: "string";
                        };
                        readonly sanitizer: {
                            readonly type: "string";
                        };
                        readonly sha256: {
                            readonly type: "string";
                        };
                        readonly text: {
                            readonly type: "string";
                        };
                    };
                    readonly type: readonly ["object", "null"];
                };
                readonly briefBytes: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly commentCount: {
                    readonly type: "integer";
                };
                readonly comments: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly at: {
                                readonly type: "integer";
                            };
                            readonly attachments: {
                                readonly items: {
                                    readonly additionalProperties: false;
                                    readonly properties: {
                                        readonly canDownload: {
                                            readonly type: "boolean";
                                        };
                                        readonly kind: {
                                            readonly type: "string";
                                        };
                                        readonly line: {
                                            readonly type: "integer";
                                        };
                                        readonly lineEnd: {
                                            readonly type: "integer";
                                        };
                                        readonly mime: {
                                            readonly type: "string";
                                        };
                                        readonly modifiedAt: {
                                            readonly type: "integer";
                                        };
                                        readonly name: {
                                            readonly type: "string";
                                        };
                                        readonly size: {
                                            readonly type: "integer";
                                        };
                                        readonly state: {
                                            readonly type: "string";
                                        };
                                    };
                                    readonly type: "object";
                                };
                                readonly type: "array";
                            };
                            readonly author: {
                                readonly type: "string";
                            };
                            readonly body: {
                                readonly type: "string";
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly createdAt: {
                    readonly type: "integer";
                };
                readonly fileCount: {
                    readonly type: "integer";
                };
                readonly hasBrief: {
                    readonly type: "boolean";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly kind: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly options: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: readonly ["array", "null"];
                };
                readonly priority: {
                    readonly type: "integer";
                };
                readonly projectId: {
                    readonly type: "string";
                };
                readonly status: {
                    readonly type: "string";
                };
                readonly title: {
                    readonly type: "string";
                };
                readonly updatedAt: {
                    readonly type: "integer";
                };
                readonly workspace: {
                    readonly type: readonly ["string", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'tickets.attachment': {
        readonly cap: "tickets:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "ticket-attachments-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/feedback/attachment";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly base64: {
                    readonly type: "string";
                };
                readonly eof: {
                    readonly type: "boolean";
                };
                readonly height: {
                    readonly type: "integer";
                };
                readonly kind: {
                    readonly type: "string";
                };
                readonly len: {
                    readonly type: "integer";
                };
                readonly line: {
                    readonly type: "integer";
                };
                readonly lineEnd: {
                    readonly type: "integer";
                };
                readonly mime: {
                    readonly type: "string";
                };
                readonly modifiedAt: {
                    readonly type: "integer";
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly offset: {
                    readonly type: "integer";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly sanitized: {
                    readonly type: "boolean";
                };
                readonly size: {
                    readonly type: "integer";
                };
                readonly state: {
                    readonly type: "string";
                };
                readonly text: {
                    readonly type: "string";
                };
                readonly truncated: {
                    readonly type: "boolean";
                };
                readonly width: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'tickets.create': {
        readonly cap: "tickets:post";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/feedback/create";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly agentName: {
                    readonly type: "string";
                };
                readonly answer: {
                    readonly type: readonly ["string", "null"];
                };
                readonly answeredAt: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly assignees: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly attachments: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly canDownload: {
                                readonly type: "boolean";
                            };
                            readonly kind: {
                                readonly type: "string";
                            };
                            readonly line: {
                                readonly type: "integer";
                            };
                            readonly lineEnd: {
                                readonly type: "integer";
                            };
                            readonly mime: {
                                readonly type: "string";
                            };
                            readonly modifiedAt: {
                                readonly type: "integer";
                            };
                            readonly name: {
                                readonly type: "string";
                            };
                            readonly size: {
                                readonly type: "integer";
                            };
                            readonly state: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly body: {
                    readonly type: readonly ["string", "null"];
                };
                readonly briefBytes: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly commentCount: {
                    readonly type: "integer";
                };
                readonly createdAt: {
                    readonly type: "integer";
                };
                readonly fileCount: {
                    readonly type: "integer";
                };
                readonly hasBrief: {
                    readonly type: "boolean";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly kind: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly options: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: readonly ["array", "null"];
                };
                readonly priority: {
                    readonly type: "integer";
                };
                readonly projectId: {
                    readonly type: "string";
                };
                readonly status: {
                    readonly type: "string";
                };
                readonly title: {
                    readonly type: "string";
                };
                readonly updatedAt: {
                    readonly type: "integer";
                };
                readonly warnings: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly code: {
                                readonly type: "string";
                            };
                            readonly hint: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly workspace: {
                    readonly type: readonly ["string", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'tickets.comment': {
        readonly cap: "tickets:post";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/feedback/comment";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly answered: {
                    readonly type: "boolean";
                };
                readonly attachments: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly canDownload: {
                                readonly type: "boolean";
                            };
                            readonly kind: {
                                readonly type: "string";
                            };
                            readonly line: {
                                readonly type: "integer";
                            };
                            readonly lineEnd: {
                                readonly type: "integer";
                            };
                            readonly mime: {
                                readonly type: "string";
                            };
                            readonly modifiedAt: {
                                readonly type: "integer";
                            };
                            readonly name: {
                                readonly type: "string";
                            };
                            readonly size: {
                                readonly type: "integer";
                            };
                            readonly state: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly author: {
                    readonly type: "string";
                };
                readonly commentId: {
                    readonly type: "string";
                };
                readonly delivered: {
                    readonly type: "boolean";
                };
                readonly deliveryReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly status: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'tickets.answer': {
        readonly cap: "tickets:post";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/feedback/answer";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly answer: {
                    readonly type: readonly ["string", "null"];
                };
                readonly answeredAt: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly attachments: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly canDownload: {
                                readonly type: "boolean";
                            };
                            readonly kind: {
                                readonly type: "string";
                            };
                            readonly line: {
                                readonly type: "integer";
                            };
                            readonly lineEnd: {
                                readonly type: "integer";
                            };
                            readonly mime: {
                                readonly type: "string";
                            };
                            readonly modifiedAt: {
                                readonly type: "integer";
                            };
                            readonly name: {
                                readonly type: "string";
                            };
                            readonly size: {
                                readonly type: "integer";
                            };
                            readonly state: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly commentId: {
                    readonly type: "string";
                };
                readonly delivered: {
                    readonly type: "boolean";
                };
                readonly deliveryReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly status: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'tickets.resolve': {
        readonly cap: "tickets:post";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/feedback/resolve";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly status: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'tickets.assign': {
        readonly cap: "tickets:post";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/feedback/assign";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly assignees: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly warnings: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly code: {
                                readonly type: "string";
                            };
                            readonly hint: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'wiki.index': {
        readonly cap: "wiki:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/wiki/index";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly generatedAt: {
                    readonly type: "string";
                };
                readonly links: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly missing: {
                                readonly type: "boolean";
                            };
                            readonly source: {
                                readonly type: "string";
                            };
                            readonly target: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly nodes: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly aliases: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                            readonly exists: {
                                readonly type: "boolean";
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly tags: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                            readonly title: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly noteCount: {
                    readonly type: "integer";
                };
                readonly scope: {
                    readonly type: "string";
                };
                readonly wikiRel: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'wiki.note': {
        readonly cap: "wiki:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/wiki/note";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly aliases: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly body: {
                    readonly type: "string";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly tags: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly title: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'storeDocs.collections': {
        readonly cap: "store:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/store/list";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly collections: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'storeDocs.get': {
        readonly cap: "store:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/store/get";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly collection: {
                    readonly type: "string";
                };
                readonly doc: {
                    readonly type: "json";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'storeDocs.query': {
        readonly cap: "store:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/store/query";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly collection: {
                    readonly type: "string";
                };
                readonly docs: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly doc: {
                                readonly type: "json";
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'db.tables': {
        readonly cap: "store:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/db/tables";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly tables: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'db.rows': {
        readonly cap: "store:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/db/rows";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly row: {
                    readonly additionalProperties: {
                        readonly type: "json";
                    };
                    readonly type: "object";
                };
                readonly rows: {
                    readonly items: {
                        readonly additionalProperties: {
                            readonly type: "json";
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly table: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'db.insert': {
        readonly cap: "store:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/db/rows";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly row: {
                    readonly additionalProperties: {
                        readonly type: "json";
                    };
                    readonly type: "object";
                };
                readonly table: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'db.update': {
        readonly cap: "store:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/db/rows/update";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly row: {
                    readonly additionalProperties: {
                        readonly type: "json";
                    };
                    readonly type: "object";
                };
                readonly table: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'db.delete': {
        readonly cap: "store:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/db/rows/delete";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly row: {
                    readonly additionalProperties: {
                        readonly type: "json";
                    };
                    readonly type: "object";
                };
                readonly table: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'db.query': {
        readonly cap: "store:read";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/db/query";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly columns: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly rowCount: {
                    readonly type: "integer";
                };
                readonly rows: {
                    readonly items: {
                        readonly additionalProperties: {
                            readonly type: "json";
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'heartbeats.list': {
        readonly cap: "heartbeats:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/heartbeat/list";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly items: {
                readonly additionalProperties: false;
                readonly properties: {
                    readonly createdAt: {
                        readonly type: "integer";
                    };
                    readonly delivery: {
                        readonly type: "string";
                    };
                    readonly disabledReason: {
                        readonly type: readonly ["string", "null"];
                    };
                    readonly enabled: {
                        readonly type: "boolean";
                    };
                    readonly firing: {
                        readonly type: "boolean";
                    };
                    readonly frequency: {
                        readonly type: "string";
                    };
                    readonly lastFired: {
                        readonly type: readonly ["string", "null"];
                    };
                    readonly name: {
                        readonly type: "string";
                    };
                    readonly nextFireAt: {
                        readonly type: readonly ["string", "null"];
                    };
                    readonly scheduleError: {
                        readonly type: readonly ["string", "null"];
                    };
                    readonly spec: {
                        readonly additionalProperties: {
                            readonly type: "json";
                        };
                        readonly type: readonly ["object", "null"];
                    };
                    readonly waitDetail: {
                        readonly type: readonly ["string", "null"];
                    };
                    readonly waitReason: {
                        readonly type: readonly ["string", "null"];
                    };
                    readonly waitSince: {
                        readonly type: readonly ["string", "null"];
                    };
                };
                readonly type: "object";
            };
            readonly type: "array";
        };
    };
    readonly 'heartbeats.show': {
        readonly cap: "heartbeats:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/heartbeat/show";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly createdAt: {
                    readonly type: "integer";
                };
                readonly delivery: {
                    readonly type: "string";
                };
                readonly disabledReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly enabled: {
                    readonly type: "boolean";
                };
                readonly firing: {
                    readonly type: "boolean";
                };
                readonly frequency: {
                    readonly type: "string";
                };
                readonly instructions: {
                    readonly type: "string";
                };
                readonly instructionsHidden: {
                    readonly type: "boolean";
                };
                readonly lastFired: {
                    readonly type: readonly ["string", "null"];
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly nextFireAt: {
                    readonly type: readonly ["string", "null"];
                };
                readonly scheduleError: {
                    readonly type: readonly ["string", "null"];
                };
                readonly spec: {
                    readonly additionalProperties: {
                        readonly type: "json";
                    };
                    readonly type: readonly ["object", "null"];
                };
                readonly waitDetail: {
                    readonly type: readonly ["string", "null"];
                };
                readonly waitReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly waitSince: {
                    readonly type: readonly ["string", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'heartbeats.status': {
        readonly cap: "heartbeats:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/heartbeat/status";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly items: {
                readonly additionalProperties: false;
                readonly properties: {
                    readonly actor: {
                        readonly type: readonly ["string", "null"];
                    };
                    readonly decision: {
                        readonly type: "string";
                    };
                    readonly durationMs: {
                        readonly type: readonly ["integer", "null"];
                    };
                    readonly firedAt: {
                        readonly type: "string";
                    };
                    readonly name: {
                        readonly type: readonly ["string", "null"];
                    };
                    readonly reason: {
                        readonly type: readonly ["string", "null"];
                    };
                };
                readonly type: "object";
            };
            readonly type: "array";
        };
    };
    readonly 'heartbeats.fires': {
        readonly cap: "heartbeats:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/heartbeat/fires-list";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly items: {
                readonly additionalProperties: false;
                readonly properties: {
                    readonly actor: {
                        readonly type: readonly ["string", "null"];
                    };
                    readonly decision: {
                        readonly type: "string";
                    };
                    readonly durationMs: {
                        readonly type: readonly ["integer", "null"];
                    };
                    readonly firedAt: {
                        readonly type: "string";
                    };
                    readonly name: {
                        readonly type: readonly ["string", "null"];
                    };
                    readonly reason: {
                        readonly type: readonly ["string", "null"];
                    };
                };
                readonly type: "object";
            };
            readonly type: "array";
        };
    };
    readonly 'heartbeats.add': {
        readonly cap: "heartbeats:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/heartbeat/add";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly createdAt: {
                    readonly type: "integer";
                };
                readonly delivery: {
                    readonly type: "string";
                };
                readonly disabledReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly enabled: {
                    readonly type: "boolean";
                };
                readonly firing: {
                    readonly type: "boolean";
                };
                readonly frequency: {
                    readonly type: "string";
                };
                readonly lastFired: {
                    readonly type: readonly ["string", "null"];
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly nextFireAt: {
                    readonly type: readonly ["string", "null"];
                };
                readonly scheduleError: {
                    readonly type: readonly ["string", "null"];
                };
                readonly spec: {
                    readonly additionalProperties: {
                        readonly type: "json";
                    };
                    readonly type: readonly ["object", "null"];
                };
                readonly waitDetail: {
                    readonly type: readonly ["string", "null"];
                };
                readonly waitReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly waitSince: {
                    readonly type: readonly ["string", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'heartbeats.edit': {
        readonly cap: "heartbeats:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/heartbeat/edit";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly createdAt: {
                    readonly type: "integer";
                };
                readonly delivery: {
                    readonly type: "string";
                };
                readonly disabledReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly enabled: {
                    readonly type: "boolean";
                };
                readonly firing: {
                    readonly type: "boolean";
                };
                readonly frequency: {
                    readonly type: "string";
                };
                readonly lastFired: {
                    readonly type: readonly ["string", "null"];
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly nextFireAt: {
                    readonly type: readonly ["string", "null"];
                };
                readonly scheduleError: {
                    readonly type: readonly ["string", "null"];
                };
                readonly spec: {
                    readonly additionalProperties: {
                        readonly type: "json";
                    };
                    readonly type: readonly ["object", "null"];
                };
                readonly waitDetail: {
                    readonly type: readonly ["string", "null"];
                };
                readonly waitReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly waitSince: {
                    readonly type: readonly ["string", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'heartbeats.enable': {
        readonly cap: "heartbeats:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/heartbeat/enable";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly createdAt: {
                    readonly type: "integer";
                };
                readonly delivery: {
                    readonly type: "string";
                };
                readonly disabledReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly enabled: {
                    readonly type: "boolean";
                };
                readonly firing: {
                    readonly type: "boolean";
                };
                readonly frequency: {
                    readonly type: "string";
                };
                readonly lastFired: {
                    readonly type: readonly ["string", "null"];
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly nextFireAt: {
                    readonly type: readonly ["string", "null"];
                };
                readonly scheduleError: {
                    readonly type: readonly ["string", "null"];
                };
                readonly spec: {
                    readonly additionalProperties: {
                        readonly type: "json";
                    };
                    readonly type: readonly ["object", "null"];
                };
                readonly waitDetail: {
                    readonly type: readonly ["string", "null"];
                };
                readonly waitReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly waitSince: {
                    readonly type: readonly ["string", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'heartbeats.rename': {
        readonly cap: "heartbeats:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/heartbeat/rename";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly createdAt: {
                    readonly type: "integer";
                };
                readonly delivery: {
                    readonly type: "string";
                };
                readonly disabledReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly enabled: {
                    readonly type: "boolean";
                };
                readonly firing: {
                    readonly type: "boolean";
                };
                readonly frequency: {
                    readonly type: "string";
                };
                readonly lastFired: {
                    readonly type: readonly ["string", "null"];
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly nextFireAt: {
                    readonly type: readonly ["string", "null"];
                };
                readonly scheduleError: {
                    readonly type: readonly ["string", "null"];
                };
                readonly spec: {
                    readonly additionalProperties: {
                        readonly type: "json";
                    };
                    readonly type: readonly ["object", "null"];
                };
                readonly waitDetail: {
                    readonly type: readonly ["string", "null"];
                };
                readonly waitReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly waitSince: {
                    readonly type: readonly ["string", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'heartbeats.archive': {
        readonly cap: "heartbeats:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/heartbeat/archive";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly archived: {
                    readonly type: "boolean";
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly success: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'heartbeats.fire': {
        readonly cap: "heartbeats:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/heartbeat/fire";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly decision: {
                    readonly type: "string";
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly reason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly success: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'activity.snapshot': {
        readonly cap: "activity:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/activity/snapshot";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly serverNow: {
                    readonly type: "integer";
                };
                readonly sessions: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly agentName: {
                                readonly type: "string";
                            };
                            readonly counts: {
                                readonly additionalProperties: false;
                                readonly properties: {
                                    readonly commands: {
                                        readonly type: "integer";
                                    };
                                    readonly subagents: {
                                        readonly type: "integer";
                                    };
                                    readonly tools: {
                                        readonly type: "integer";
                                    };
                                };
                                readonly type: "object";
                            };
                            readonly paneGroupId: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly since: {
                                readonly type: readonly ["integer", "null"];
                            };
                            readonly status: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly since: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly status: {
                    readonly type: "string";
                };
                readonly workspace: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'activity.events': {
        readonly cap: "activity:read";
        readonly effect: "read";
        readonly kind: "subscribe";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: null;
        readonly socket: {
            readonly path: "/cli/activity/events";
            readonly param: "workspace";
            readonly frame: "session_activity_changed";
            readonly refetch: "activity.snapshot";
            readonly query: {};
        };
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly agentName: {
                    readonly type: "string";
                };
                readonly app: {
                    readonly type: "string";
                };
                readonly change: {
                    readonly type: "string";
                };
                readonly counts: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly commands: {
                            readonly type: "integer";
                        };
                        readonly subagents: {
                            readonly type: "integer";
                        };
                        readonly tools: {
                            readonly type: "integer";
                        };
                    };
                    readonly type: "object";
                };
                readonly hasBrief: {
                    readonly type: "boolean";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly kind: {
                    readonly enum: readonly ["session_activity_changed", "heartbeat_changed", "ticket_changed", "rooms_changed", "room_members_changed", "app_deployed", "resync"];
                };
                readonly live: {
                    readonly type: "boolean";
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly paneGroupId: {
                    readonly type: readonly ["string", "null"];
                };
                readonly serverNow: {
                    readonly type: "integer";
                };
                readonly since: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly status: {
                    readonly type: "string";
                };
                readonly via: {
                    readonly type: readonly ["string", "null"];
                };
                readonly workspace: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'agentIdentity.get': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/agent/identity";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly canEditHandle: {
                    readonly type: "boolean";
                };
                readonly canEditIdentity: {
                    readonly type: "boolean";
                };
                readonly color: {
                    readonly type: readonly ["string", "null"];
                };
                readonly displayName: {
                    readonly type: "string";
                };
                readonly handle: {
                    readonly type: "string";
                };
                readonly image: {
                    readonly type: readonly ["string", "null"];
                };
                readonly workspace: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'agentIdentity.set': {
        readonly cap: "agent:identity";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/agent/identity";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly canEditHandle: {
                    readonly type: "boolean";
                };
                readonly canEditIdentity: {
                    readonly type: "boolean";
                };
                readonly changed: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly color: {
                    readonly type: readonly ["string", "null"];
                };
                readonly displayName: {
                    readonly type: "string";
                };
                readonly handle: {
                    readonly type: "string";
                };
                readonly image: {
                    readonly type: readonly ["string", "null"];
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly workspace: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'agentIdentity.setHandle': {
        readonly cap: "agent:handle";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/agent/handle";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly canEditHandle: {
                    readonly type: "boolean";
                };
                readonly canEditIdentity: {
                    readonly type: "boolean";
                };
                readonly changed: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly color: {
                    readonly type: readonly ["string", "null"];
                };
                readonly displayName: {
                    readonly type: "string";
                };
                readonly handle: {
                    readonly type: "string";
                };
                readonly image: {
                    readonly type: readonly ["string", "null"];
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly previousHandle: {
                    readonly type: "string";
                };
                readonly workspace: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'room.members': {
        readonly cap: "room:members";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "room-members-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/members";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly admins: {
                    readonly type: "string";
                };
                readonly count: {
                    readonly type: "integer";
                };
                readonly members: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly app: {
                                readonly type: "string";
                            };
                            readonly caps: {
                                readonly items: {
                                    readonly type: "string";
                                };
                                readonly type: "array";
                            };
                            readonly displayName: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly kind: {
                                readonly enum: readonly ["guest", "agent"];
                            };
                            readonly pendingReset: {
                                readonly type: "boolean";
                            };
                            readonly role: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly tiedTo: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly username: {
                                readonly type: readonly ["string", "null"];
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly namesHidden: {
                    readonly type: "boolean";
                };
                readonly room: {
                    readonly type: "string";
                };
                readonly workspace: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'app.self': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-self-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/self";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly deployedAt: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly deployment: {
                    readonly type: readonly ["string", "null"];
                };
                readonly health: {
                    readonly enum: readonly ["running", "starting", "crashlooping", "exited", "stopped", "suspended"];
                };
                readonly liveSeed: {
                    readonly type: readonly ["string", "null"];
                };
                readonly state: {
                    readonly type: "string";
                };
                readonly workspace: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'app.usage': {
        readonly cap: null;
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-self-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/usage";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly hours: {
                    readonly type: "integer";
                };
                readonly live: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly connections: {
                            readonly type: "integer";
                        };
                        readonly subscriptions: {
                            readonly type: "integer";
                        };
                    };
                    readonly type: "object";
                };
                readonly quotas: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly bodyBytes: {
                            readonly type: "integer";
                        };
                        readonly burst: {
                            readonly type: "integer";
                        };
                        readonly rps: {
                            readonly type: "integer";
                        };
                        readonly sockets: {
                            readonly type: "integer";
                        };
                        readonly subscriptions: {
                            readonly type: "integer";
                        };
                        readonly uploadBytes: {
                            readonly type: "integer";
                        };
                    };
                    readonly type: "object";
                };
                readonly totals: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly bytesIn: {
                            readonly type: "integer";
                        };
                        readonly bytesOut: {
                            readonly type: "integer";
                        };
                        readonly calls: {
                            readonly type: "integer";
                        };
                        readonly denied: {
                            readonly type: "integer";
                        };
                    };
                    readonly type: "object";
                };
                readonly verbs: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly calls: {
                                readonly type: "integer";
                            };
                            readonly denied: {
                                readonly type: "integer";
                            };
                            readonly route: {
                                readonly type: "string";
                            };
                            readonly verb: {
                                readonly type: readonly ["string", "null"];
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'chat.pass': {
        readonly cap: null;
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-api-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/sew/pass";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly caps: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly expiresAt: {
                    readonly type: "string";
                };
                readonly expiresInMs: {
                    readonly type: "integer";
                };
                readonly jti: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly pass: {
                    readonly type: "string";
                };
                readonly personProof: {
                    readonly type: "boolean";
                };
                readonly provenance: {
                    readonly type: "string";
                };
                readonly sewRole: {
                    readonly type: "string";
                };
                readonly sid: {
                    readonly type: "string";
                };
                readonly sub: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'guests.chat': {
        readonly cap: "guests:chat";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-chat-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/guests/chat";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: readonly ["string", "null"];
                };
                readonly appScoped: {
                    readonly type: "boolean";
                };
                readonly changed: {
                    readonly type: "boolean";
                };
                readonly enabled: {
                    readonly type: "boolean";
                };
                readonly managedBy: {
                    readonly type: readonly ["string", "null"];
                };
                readonly maxMembers: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly members: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly principalId: {
                    readonly type: "string";
                };
                readonly scope: {
                    readonly enum: readonly ["member", "guest", null];
                    readonly type: readonly ["string", "null"];
                };
                readonly username: {
                    readonly type: "string";
                };
                readonly warning: {
                    readonly type: readonly ["string", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'guests.setChat': {
        readonly cap: "guests:chat";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-chat-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/guests/chat";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: readonly ["string", "null"];
                };
                readonly appScoped: {
                    readonly type: "boolean";
                };
                readonly changed: {
                    readonly type: "boolean";
                };
                readonly enabled: {
                    readonly type: "boolean";
                };
                readonly managedBy: {
                    readonly type: readonly ["string", "null"];
                };
                readonly maxMembers: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly members: {
                    readonly type: readonly ["integer", "null"];
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly principalId: {
                    readonly type: "string";
                };
                readonly scope: {
                    readonly enum: readonly ["member", "guest", null];
                    readonly type: readonly ["string", "null"];
                };
                readonly username: {
                    readonly type: "string";
                };
                readonly warning: {
                    readonly type: readonly ["string", "null"];
                };
            };
            readonly type: "object";
        };
    };
    readonly 'chat.member': {
        readonly cap: "chat:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-chat-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/sew/app/member";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly externalId: {
                    readonly type: "string";
                };
                readonly memberId: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly principalId: {
                    readonly type: "string";
                };
                readonly state: {
                    readonly enum: readonly ["active", "deactivated"];
                    readonly type: "string";
                };
                readonly username: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'chat.whois': {
        readonly cap: "chat:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-chat-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/sew/app/whois";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly app: {
                    readonly type: "string";
                };
                readonly members: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly deleted: {
                                readonly type: "boolean";
                            };
                            readonly displayName: {
                                readonly type: "string";
                            };
                            readonly imageHash: {
                                readonly type: "string";
                            };
                            readonly isThisAppUser: {
                                readonly type: "boolean";
                            };
                            readonly kind: {
                                readonly enum: readonly ["person", "agent", "unknown"];
                                readonly type: "string";
                            };
                            readonly memberId: {
                                readonly type: "string";
                            };
                            readonly principalId: {
                                readonly type: "string";
                            };
                            readonly username: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'chat.createChannel': {
        readonly cap: "chat:channels";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-chat-v1";
        readonly door: "link";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/sew/app/channels";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly added: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly app: {
                    readonly type: "string";
                };
                readonly channelId: {
                    readonly type: "string";
                };
                readonly created: {
                    readonly type: "boolean";
                };
                readonly name: {
                    readonly type: "string";
                };
                readonly notAdded: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly reason: {
                                readonly type: "string";
                            };
                            readonly username: {
                                readonly type: "string";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly private: {
                    readonly type: "boolean";
                };
                readonly skipped: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly slug: {
                    readonly type: "string";
                };
                readonly spaceId: {
                    readonly type: "string";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'project.list': {
        readonly cap: "thread:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-project-chat-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/projects";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly projects: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly canPost: {
                                readonly type: "boolean";
                            };
                            readonly color: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly groupId: {
                                readonly type: "string";
                            };
                            readonly lastReadSeq: {
                                readonly type: readonly ["integer", "null"];
                            };
                            readonly lastSeq: {
                                readonly type: "integer";
                            };
                            readonly members: {
                                readonly items: {
                                    readonly additionalProperties: false;
                                    readonly properties: {
                                        readonly handle: {
                                            readonly type: "string";
                                        };
                                        readonly room: {
                                            readonly type: "string";
                                        };
                                    };
                                    readonly type: "object";
                                };
                                readonly type: "array";
                            };
                            readonly muted: {
                                readonly type: readonly ["boolean", "null"];
                            };
                            readonly name: {
                                readonly type: "string";
                            };
                            readonly poc: {
                                readonly type: readonly ["string", "null"];
                            };
                            readonly unread: {
                                readonly type: readonly ["integer", "null"];
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'project.read': {
        readonly cap: "thread:read";
        readonly effect: "read";
        readonly kind: "call";
        readonly feature: "app-project-chat-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "GET";
            readonly route: "/cli/skin/projects/messages";
            readonly params: {
                readonly afterSeq: "after_seq";
                readonly beforeSeq: "before_seq";
            };
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly after_seq: {
                    readonly type: "integer";
                };
                readonly before_seq: {
                    readonly type: "integer";
                };
                readonly changed: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly groupId: {
                    readonly type: "string";
                };
                readonly has_more: {
                    readonly type: "boolean";
                };
                readonly messages: {
                    readonly items: {
                        readonly additionalProperties: false;
                        readonly properties: {
                            readonly author: {
                                readonly type: "string";
                            };
                            readonly authorKind: {
                                readonly enum: readonly ["person", "agent", "guest"];
                            };
                            readonly body: {
                                readonly type: "string";
                            };
                            readonly createdAt: {
                                readonly type: "integer";
                            };
                            readonly groupId: {
                                readonly type: "string";
                            };
                            readonly id: {
                                readonly type: "string";
                            };
                            readonly seq: {
                                readonly type: "integer";
                            };
                        };
                        readonly type: "object";
                    };
                    readonly type: "array";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly removed: {
                    readonly items: {
                        readonly type: "string";
                    };
                    readonly type: "array";
                };
                readonly truncated: {
                    readonly type: "boolean";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'project.post': {
        readonly cap: "project:write";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-project-chat-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/projects/post";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly author: {
                    readonly type: "string";
                };
                readonly authorKind: {
                    readonly enum: readonly ["guest"];
                };
                readonly body: {
                    readonly type: "string";
                };
                readonly createdAt: {
                    readonly type: "integer";
                };
                readonly delivered: {
                    readonly type: "boolean";
                };
                readonly deliveryReason: {
                    readonly type: readonly ["string", "null"];
                };
                readonly groupId: {
                    readonly type: "string";
                };
                readonly groupName: {
                    readonly type: "string";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly seq: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'project.markRead': {
        readonly cap: "thread:read";
        readonly effect: "write";
        readonly kind: "call";
        readonly feature: "app-project-chat-v1";
        readonly door: "both";
        readonly http: {
            readonly method: "POST";
            readonly route: "/cli/skin/projects/read";
            readonly params: {};
            readonly query: readonly [];
        };
        readonly socket: null;
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly changed: {
                    readonly type: "boolean";
                };
                readonly groupId: {
                    readonly type: "string";
                };
                readonly lastReadSeq: {
                    readonly type: "integer";
                };
                readonly lastSeq: {
                    readonly type: "integer";
                };
                readonly muted: {
                    readonly type: "boolean";
                };
                readonly ok: {
                    readonly type: "boolean";
                };
                readonly unread: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
    readonly 'project.events': {
        readonly cap: "thread:read";
        readonly effect: "read";
        readonly kind: "subscribe";
        readonly feature: "app-project-chat-v1";
        readonly door: "both";
        readonly http: null;
        readonly socket: {
            readonly path: "/cli/skin/projects/events";
            readonly param: "group";
            readonly frame: "project";
            readonly refetch: "project.read";
            readonly query: {};
        };
        readonly responseNames: {};
        readonly response: {
            readonly additionalProperties: false;
            readonly properties: {
                readonly collection: {
                    readonly enum: readonly ["project", "read"];
                };
                readonly doc: {
                    readonly additionalProperties: false;
                    readonly properties: {
                        readonly author: {
                            readonly type: "string";
                        };
                        readonly authorKind: {
                            readonly enum: readonly ["person", "agent", "guest"];
                        };
                        readonly body: {
                            readonly type: "string";
                        };
                        readonly createdAt: {
                            readonly type: "integer";
                        };
                        readonly groupId: {
                            readonly type: "string";
                        };
                        readonly id: {
                            readonly type: "string";
                        };
                        readonly seq: {
                            readonly type: "integer";
                        };
                    };
                    readonly type: "object";
                };
                readonly groupId: {
                    readonly type: "string";
                };
                readonly id: {
                    readonly type: "string";
                };
                readonly lastReadSeq: {
                    readonly type: "integer";
                };
                readonly lastSeq: {
                    readonly type: "integer";
                };
                readonly muted: {
                    readonly type: "boolean";
                };
                readonly seq: {
                    readonly type: "integer";
                };
                readonly unread: {
                    readonly type: "integer";
                };
                readonly waiting: {
                    readonly type: "integer";
                };
            };
            readonly type: "object";
        };
    };
};
export type AppVerb = keyof typeof APP_VERBS;
export interface AppMeCan {
    readonly activity: boolean;
    readonly chat: boolean;
    /** files:download in the room: it may save files (catalog v22+). */
    readonly download?: boolean;
    readonly files: string;
    readonly heartbeats: string;
    readonly identity: boolean;
    readonly members: boolean;
    readonly send: boolean;
    readonly store: string;
    readonly tickets: string;
    /** files:upload in the room: it may drop files in the drop folders (catalog v22+). */
    readonly upload?: boolean;
    readonly wiki: boolean;
}
export interface AppMeAgentFolders {
    /** Folders or single files it may save with files:download, without files:read. */
    readonly download?: string[];
    /** Room-relative folders it may list and read. */
    readonly read?: string[];
    /** Drop folders: with files:upload it may add files there (in its own subfolder), never list or read them. */
    readonly upload?: string[];
    /** Where an upload lands when it names no folder. */
    readonly uploadDir?: string;
    /** Room-relative folders it may also write. */
    readonly write?: string[];
}
export interface AppMeAgent {
    /** What the caps allow, for drawing buttons: files, tickets, heartbeats and store are none, read or write. */
    readonly can: AppMeCan;
    readonly caps: string[];
    readonly color: string | null;
    readonly displayName: string;
    /** This login's folders in the room (room-scope R3). Absent: the whole room. */
    readonly folders?: AppMeAgentFolders;
    readonly handle: string;
    /** The agent's picture as a data URL, or null. */
    readonly image: string | null;
    /** This login muted the room's Chat. With thread:read, like unread. */
    readonly muted?: boolean;
    /** The room's project id. */
    readonly room: string;
    /** The room's tickets waiting on someone (waiting or needs_discussion). Present with tickets:read in the room (agent-unread-tickets-v1). */
    readonly ticketsWaiting?: number;
    /** This login's unread items on the room's Chat (capped at 100: show 99+). Present with thread:read in the room, on a server with agent-unread-tickets-v1. */
    readonly unread?: number;
    /** Cards on the room's Chat waiting for this login's answer. With thread:read, like unread. */
    readonly waiting?: number;
}
export interface AppMeFolders {
    /** Folders or single files it may save with files:download, without files:read. */
    readonly download?: string[];
    /** Room-relative folders it may list and read. */
    readonly read?: string[];
    /** Drop folders: with files:upload it may add files there (in its own subfolder), never list or read them. */
    readonly upload?: string[];
    /** Where an upload lands when it names no folder. */
    readonly uploadDir?: string;
    /** Room-relative folders it may also write. */
    readonly write?: string[];
}
export interface AppMeRoom {
    readonly caps: string[];
    /** This login's folders in the room (room-scope R3). Absent: the whole room. */
    readonly folders?: AppMeFolders;
    readonly handle: string;
    /** The room's project id. */
    readonly room: string;
}
export interface AppMe {
    /** The agents this person may reach here, one per room on the pass (the only list of 'my agents'; a room not on it doesn't exist for this pass). */
    readonly agents: AppMeAgent[];
    /** host, or the App's service id this login belongs to ('' for a platform pass). */
    readonly app: string;
    readonly catalogVersion: number;
    /** This server's /boot-status feature keys. */
    readonly features: string[];
    /** Changes when the server restarts. */
    readonly instanceId: string;
    /** guest: a guest's login; app: this App's own pass on its link; token: an owner-minted platform pass. */
    readonly kind: 'guest' | 'app' | 'token';
    readonly ok: boolean;
    readonly principalId: string | null;
    /** <label>.k2.dev when the server has a tunnel. */
    readonly publicHost: string | null;
    readonly rooms: AppMeRoom[];
    /** This server's stable id (key rows across servers by serverId + room). */
    readonly serverId: string | null;
    readonly username: string;
}
export interface AppRoomAgent {
    /** #rrggbb. */
    readonly color: string | null;
    readonly displayName: string;
    readonly handle: string;
    /** A data: URL. */
    readonly image: string | null;
    /** The room's Chat is muted for this pass. With thread:read, like unread. */
    readonly muted?: boolean;
    readonly projectId: string;
    /** The room's tickets waiting on someone (waiting or needs_discussion). Present with tickets:read in the room (agent-unread-tickets-v1). */
    readonly ticketsWaiting?: number;
    /** This pass's unread items on the room's Chat (capped at 100: show 99+). Present with thread:read in the room, on a server with agent-unread-tickets-v1. */
    readonly unread?: number;
    /** Cards on the room's Chat waiting for this pass's answer. With thread:read, like unread. */
    readonly waiting?: number;
}
export interface AppRoomAgents {
    readonly agents: AppRoomAgent[];
}
export interface AppDevice {
    readonly id: string;
    readonly name?: string;
}
export interface AppDeviceRef {
    readonly id: string;
    readonly name: string;
}
export interface AppRoomAccess {
    readonly caps: string[];
    readonly handle: string;
}
export interface AppGuestLogin {
    /** host or the App's service id. */
    readonly app: string;
    /** Every cap in any room. */
    readonly caps: string[];
    readonly device?: AppDeviceRef;
    readonly ok: boolean;
    /** The guest's session pass (k2skn_…): send it as Authorization: Bearer. */
    readonly pass: string;
    /** With device: when the pass ends, seconds since the epoch. */
    readonly passExpiresAt?: number;
    readonly principalId: string;
    /** With device: the refresh token (k2skr_...). Store it; guests.refresh trades it. */
    readonly refresh?: string;
    /** With device: when the refresh token ends unless used, seconds since the epoch. */
    readonly refreshExpiresAt?: number;
    readonly role: string | null;
    readonly roomAccess: AppRoomAccess[];
    readonly roomHandles: string[];
    /** Project ids. */
    readonly rooms: string[];
    readonly username: string;
}
export interface AppGuestRooms {
    /** The App's service id. */
    readonly app: string;
    /** true only when a set wrote something (the same rooms are a no-op). */
    readonly changed: boolean;
    /** Whether the guest's grant on this App is on. */
    readonly enabled: boolean;
    readonly ok: boolean;
    /** The role on this App's grant; null = Thread-only rooms. */
    readonly role: string | null;
    readonly roomHandles: string[];
    /** Project ids. */
    readonly rooms: string[];
    readonly username: string;
}
export interface AppRoomRef {
    readonly displayName: string;
    readonly handle: string;
    /** The room's project id. */
    readonly room: string;
}
export interface AppOwnRoom {
    readonly displayName: string;
    readonly handle: string;
    /** The room's project id. */
    readonly room: string;
}
export interface AppRooms {
    /** The App's service id. */
    readonly app: string;
    /** Project ids this App may hand its guests (the list guests.setRooms enforces); [] without the grant. */
    readonly ceiling: string[];
    /** The rooms the grant adds beyond the own room; [] without the grant. */
    readonly grantRooms: AppRoomRef[];
    /** Whether the owner granted this App guests-rooms (k2 app grant <app> --kind guests-rooms). */
    readonly granted: boolean;
    /** When the owner granted it, seconds since the epoch; null without the grant. */
    readonly grantedAt: number | null;
    readonly ok: boolean;
    /** The App's own workspace room; null when it no longer resolves. */
    readonly ownRoom: AppOwnRoom | null;
}
export interface AppGuestRow {
    /** A marked agent login (k2 app user agent), not a person. */
    readonly agent: boolean;
    /** Whether the guest's grant on this App is on. */
    readonly enabled: boolean;
    /** The role on this App's grant; null = Thread-only rooms. */
    readonly role: string | null;
    readonly roomHandles: string[];
    /** Project ids on this App only. */
    readonly rooms: string[];
    readonly username: string;
}
export interface AppGuestList {
    /** The App's service id. */
    readonly app: string;
    /** This App's own guests, by username. */
    readonly guests: AppGuestRow[];
    /** Pass it as cursor for the next page; null on the last page. */
    readonly nextCursor: string | null;
    readonly ok: boolean;
}
export interface AppPasswordChanged {
    readonly ok: boolean;
    /** This pass stayed signed in (stay: true). */
    readonly stayed?: boolean;
}
export interface WalletItem {
    /** The App it was added through. */
    readonly addedVia: string | null;
    /** Seconds since the epoch. */
    readonly createdAt: number;
    /** Seconds since the epoch: set when the person lost App access; the item is deleted 30 days later. */
    readonly dormantSince: number | null;
    /** Seconds since the epoch. */
    readonly expiresAt: number | null;
    readonly id: string;
    /** This is the person's pick for its provider. */
    readonly inUse: boolean;
    readonly label: string;
    /** Seconds since the epoch. */
    readonly lastUsedAt: number | null;
    readonly plan: string | null;
    /** claude, codex, grok or gemini. */
    readonly provider: string;
    /** The secret is sealed in K2's vault (false only while the vault is closed). */
    readonly sealed: boolean;
    /** not_set_up, signing_in, signed_in or needs_login. */
    readonly state: string;
    /** More item types (other third-party logins) come later. */
    readonly type: 'llm_subscription' | 'llm_api_key';
    /** Handles of the person's own agents that run on it. */
    readonly usedBy: string[];
}
export interface WalletProviders {
    readonly apiKey: string[];
    readonly subscription: string[];
}
export interface WalletList {
    readonly items: WalletItem[];
    readonly ok: boolean;
    readonly providers: WalletProviders;
    /** provider → the item id the person's agents use. */
    readonly uses: Record<string, string>;
}
export interface WalletItemAnswer {
    readonly item: WalletItem;
    readonly ok: boolean;
}
export interface WalletLogin {
    /** A device code to enter on the vendor's page. */
    readonly code: string | null;
    readonly done: boolean;
    readonly error: string | null;
    readonly itemId: string;
    readonly label: string;
    readonly loginId: string;
    /** The vendor's page shows a code to paste back (wallet.loginInput). */
    readonly needsPaste: boolean;
    readonly provider: string;
    /** Seconds since the epoch. */
    readonly startedAt: number;
    /** signing_in, waiting_for_browser, waiting_for_code, signed_in, failed, cancelled or timed_out. */
    readonly state: string;
    /** The whole sign-in URL: open and copy exactly this. */
    readonly url: string | null;
    /** The vendor's host, for "Sign in at …". */
    readonly urlHost: string | null;
}
export interface WalletSignIn {
    readonly item: WalletItem;
    readonly login: WalletLogin | null;
    readonly ok: boolean;
}
export interface WalletLoginStatus {
    readonly login: WalletLogin;
    readonly ok: boolean;
}
export interface AppOk {
    readonly ok: boolean;
}
export interface WalletUse {
    readonly id: string | null;
    readonly ok: boolean;
    readonly provider: string;
}
export interface WalletRemoved {
    readonly note: string;
    readonly ok: boolean;
    readonly removed: string;
}
export interface AppDeviceSession {
    readonly app: string;
    readonly device: AppDeviceRef;
    readonly ok: boolean;
    /** A new session pass (k2skn_...); the old one has ended. */
    readonly pass: string;
    /** Seconds since the epoch. */
    readonly passExpiresAt: number;
    readonly principalId: string;
    /** The next refresh token: store it in place of the old one. */
    readonly refresh: string;
    /** Seconds since the epoch: unused this long, the device signs in again. */
    readonly refreshExpiresAt: number;
    readonly username: string;
}
export interface AppDeviceLogin {
    /** Signed in, seconds since the epoch. */
    readonly createdAt: number;
    /** The device this pass belongs to. */
    readonly current: boolean;
    readonly deviceId: string;
    /** Signs out unless used before then, seconds since the epoch. */
    readonly expiresAt: number;
    /** What guests.revokeDevice takes. */
    readonly id: string;
    /** Last refresh, seconds since the epoch. */
    readonly lastUsedAt: number;
    readonly name: string;
    /** How it started: a QR pairing, or a device login with the password. */
    readonly via: 'pairing' | 'password';
}
export interface AppDevices {
    readonly devices: AppDeviceLogin[];
    readonly ok: boolean;
}
export interface AppProfileCan {
    readonly editImage: boolean;
    readonly editNames: boolean;
}
export interface AppProfileCounts {
    readonly devices: number;
    readonly pushDevices: number;
    readonly sessions: number;
}
export interface AppProfileRoom {
    readonly handle: string;
    /** The agent's display name. */
    readonly name: string;
    readonly room: string;
}
export interface AppProfile {
    /** An agent's own login: reads only. */
    readonly agentLogin: boolean;
    readonly app: string;
    readonly can: AppProfileCan;
    readonly counts: AppProfileCounts;
    /** Account made, seconds since the epoch. */
    readonly createdAt: number;
    /** A short name the App may show; K2 keeps using fullName. */
    readonly displayName: string | null;
    readonly email: string | null;
    readonly fullName: string | null;
    readonly hasPassword: boolean;
    /** data:image/jpeg;base64,… (K2's own re-encode, at most 64 KiB). Present only with image=1, or when ifImageHash differs. */
    readonly image?: string | null;
    /** SHA-256 of the picture: cache it, and ask again with ifImageHash. */
    readonly imageHash: string | null;
    /** Last sign-in to this App (not a refresh), seconds since the epoch; null before the first since 0.46.5. */
    readonly lastSignInAt: number | null;
    /** The person set fullName themselves (K2's own views show the username beside it). */
    readonly nameSetBySelf: boolean;
    readonly ok: boolean;
    /** The role on this App's grant (this App only). */
    readonly role: string | null;
    readonly rooms: AppProfileRoom[];
    /** A test guest (names and picture only; never a reset). */
    readonly test: boolean;
    /** Fixed: never changes. */
    readonly username: string;
}
export interface AppProfileUpdated {
    readonly ok: boolean;
    readonly profile: AppProfile;
}
export interface AppProfileImage {
    /** The stored picture, data:image/jpeg;base64,… */
    readonly image: string;
    readonly imageHash: string;
    readonly ok: boolean;
}
export interface AppSession {
    /** A coarse label ("Safari on macOS", a device's name, or the clientLabel an App's server gave at login). Never an IP. */
    readonly client: string | null;
    /** Signed in, seconds since the epoch. */
    readonly createdAt: number;
    /** The pass making this call. */
    readonly current: boolean;
    /** Ends unless used before then, seconds since the epoch. */
    readonly expiresAt: number | null;
    /** What profile.revokeSession takes: ses_… for a browser (opaque; list again after a server restart), the device sign-in id for a device (as guests.devices). */
    readonly id: string;
    readonly kind: 'browser' | 'device';
    /** Last used (to about 10 minutes), seconds since the epoch. */
    readonly lastSeenAt: number | null;
    /** A notification device is linked to it. */
    readonly push: boolean;
    /** ios or android, when push is true. */
    readonly pushPlatform?: string;
    /** password, device or refresh; null for a session from before 0.46.5. */
    readonly via: string | null;
}
export interface AppSessions {
    readonly ok: boolean;
    readonly sessions: AppSession[];
}
export interface AppSignedOut {
    /** How many sessions and device sign-ins ended. */
    readonly ended: number;
    readonly ok: boolean;
}
export interface AppPushConfigured {
    /** The owner set this App's Apple key. */
    readonly apns: boolean;
    /** The owner set this App's Firebase service account. */
    readonly fcm: boolean;
}
export interface AppPushDevice {
    readonly channel: 'app' | 'k2-companion';
    readonly configured: AppPushConfigured;
    readonly deviceId: string;
    readonly environment: 'production' | 'sandbox';
    /** This registration's id (apd_...). */
    readonly id: string;
    /** Last register, seconds since the epoch. */
    readonly lastSeenAt: number;
    /** Topics this device skips. */
    readonly mutedTopics: string[];
    readonly ok: boolean;
    readonly platform: 'apns' | 'fcm';
    /** This device's own choice for agent-reply text; null = on. */
    readonly previews: boolean | null;
    /** An opaque key for this person on this App; every push carries it as k2Who. */
    readonly who: string;
}
export interface AppPushRemoved {
    readonly ok: boolean;
    /** false when this guest had no such device (the same answer for anyone else's). */
    readonly removed: boolean;
}
export interface AppPushTo {
    /** A principal id or username of this App's guest. */
    readonly guest?: string;
    /** At most 1,000. */
    readonly guests?: string[];
    /** One of this App's rooms (a handle or project id): its guests with a grant there. */
    readonly room?: string;
}
export interface AppPushSkipped {
    /** Devices that muted this topic. */
    readonly muted_topic: number;
    /** Guests with no registered phone. */
    readonly no_devices: number;
    /** Names that aren't this App's guests (unknown, or another App's: the same count). */
    readonly not_a_guest: number;
    /** Devices on a platform this App has no key for. */
    readonly platform_not_configured: number;
    /** Guests over their hourly push budget. */
    readonly rate_limited: number;
}
export interface AppPushQueued {
    readonly ok: boolean;
    /** Device sends queued. */
    readonly queued: number;
    /** What push.status takes. */
    readonly sendId: string;
    readonly skipped: AppPushSkipped;
}
export interface AppPushStatus {
    /** When it was sent, seconds since the epoch. */
    readonly at: number;
    /** The phone's token was gone; K2 removed that device. */
    readonly dead: number;
    /** Dropped before sending: air-gap, or the phone, the person's access or the App's push changed after it was queued. */
    readonly dropped: number;
    readonly failed: number;
    readonly ok: boolean;
    readonly queued: number;
    readonly sendId: string;
    /** Apple or Google accepted it (neither reports display). */
    readonly sent: number;
    readonly skipped: AppPushSkipped;
    /** Tokens from another app (check the bundle id). */
    readonly wrongApp: number;
}
export interface AppPushGuestDevices {
    readonly devices: number;
    /** The guest's principal id. */
    readonly guest: string;
    readonly platforms: string[];
}
export interface AppPushDevices {
    readonly guests: AppPushGuestDevices[];
    readonly ok: boolean;
}
export interface AppQr {
    /** One string of 1 (dark) and 0 (light) per row, no quiet zone. */
    readonly rows: string[];
    /** Modules per side (not a time). */
    readonly size: number;
}
export interface AppPairingStart {
    /** The one-time code (K2A-...), shown once: draw qr, never log it. */
    readonly code: string;
    /** When the code stops working, seconds since the epoch. */
    readonly expiresAt: number;
    /** The code's life in seconds (start again when it runs out). */
    readonly expiresInSecs: number;
    /** The host the link names. */
    readonly host: string;
    readonly ok: boolean;
    /** https://<host>/.well-known/k2-pair#<code>: what the QR holds; the code rides in the fragment. */
    readonly pairUrl: string;
    /** Not a secret: what pairing.status and pairing.cancel take (apr_...). */
    readonly pairingId: string;
    readonly qr: AppQr;
}
export interface AppPairedDevice {
    /** What guests.revokeDevice takes (Not you? End it). */
    readonly deviceLoginId: string;
    /** The name the phone gave; it proves nothing on its own. */
    readonly name: string;
    /** Seconds since the epoch. */
    readonly pairedAt: number;
}
export interface AppPairingStatus {
    readonly device?: AppPairedDevice;
    /** While pending: seconds left. */
    readonly expiresInSecs?: number;
    readonly ok: boolean;
    readonly pairingId: string;
    /** expired: it ran out or a redeem used it and was refused; start again. */
    readonly state: 'pending' | 'paired' | 'expired' | 'cancelled';
}
export interface AppPairingCancelled {
    /** false when nothing of this guest's was pending under that id. */
    readonly cancelled: boolean;
    readonly ok: boolean;
}
export interface AppPairingCheck {
    readonly app: string;
    readonly appName: string;
    /** Seconds left. */
    readonly expiresInSecs: number;
    readonly fullName?: string;
    readonly ok: boolean;
    /** Who the code signs in: show it before redeeming. */
    readonly username: string;
}
export interface AppPairingPush {
    /** Leave it out: it is device.id. */
    readonly deviceId?: string;
    /** APNs only: sandbox for Debug builds. */
    readonly environment?: 'production' | 'sandbox';
    /** Topics to skip (at most 32, each a-z 0-9 . _ -). */
    readonly mutedTopics?: string[];
    /** apns: the iOS device token as hex. fcm: an FCM token (Android, or iOS through Firebase). */
    readonly platform: 'apns' | 'fcm';
    /** Show agent-reply text on this device; null = on. */
    readonly previews?: boolean | null;
    /** The native token from the OS (never an Expo push token). */
    readonly pushToken: string;
}
export interface AppPairingPushResult {
    /** Why it didn't register (push's own refusal code). */
    readonly code?: string;
    readonly configured?: AppPushConfigured;
    /** The push registration's id (apd_...). */
    readonly id?: string;
    readonly registered: boolean;
    /** This person's opaque key on this App (k2Who). */
    readonly who?: string;
}
export interface AppPairedSession {
    readonly app: string;
    /** Every cap in any room. */
    readonly caps: string[];
    readonly device: AppDeviceRef;
    readonly ok: boolean;
    /** The phone's session pass (k2skn_...): send it as Authorization: Bearer. */
    readonly pass: string;
    /** When the pass ends, seconds since the epoch. */
    readonly passExpiresAt: number;
    readonly principalId: string;
    readonly push?: AppPairingPushResult;
    /** The refresh token (k2skr_...): store it; guests.refresh trades it. */
    readonly refresh: string;
    /** When the refresh token ends unless used, seconds since the epoch. */
    readonly refreshExpiresAt: number;
    readonly role: string | null;
    readonly roomAccess: AppRoomAccess[];
    readonly roomHandles: string[];
    /** Project ids. */
    readonly rooms: string[];
    readonly username: string;
    /** This device sign-in started from a pairing. */
    readonly via: 'pairing';
}
export interface AppEmailAccepted {
    /** What email.status takes. */
    readonly id: string;
    readonly ok: boolean;
    readonly recipients: number;
    /** submitted = accepted for delivery (delivery events arrive later). */
    readonly status: 'submitted' | 'scheduled' | 'held';
}
export interface AppEmailStatus {
    /** Seconds since the epoch. */
    readonly createdAt: number;
    /** The refusal or failure code. */
    readonly error?: string;
    readonly id: string;
    readonly ok: boolean;
    readonly recipients: number;
    /** When a scheduled email goes, seconds since the epoch. */
    readonly sendAt?: number;
    readonly status: 'queued' | 'scheduled' | 'held' | 'submitted' | 'failed' | 'refused' | 'cancelled';
    /** Seconds since the epoch. */
    readonly updatedAt: number;
}
export interface AppEmailLogItem {
    /** Seconds since the epoch. */
    readonly createdAt: number;
    readonly error?: string;
    readonly from?: string;
    readonly id: string;
    readonly origin?: string;
    readonly purpose?: string;
    readonly status: string;
    readonly subject: string;
    readonly tags: string[];
    readonly to: string[];
}
export interface AppEmailLog {
    readonly items: AppEmailLogItem[];
    /** Pass as before for the next page (a createdAt, seconds since the epoch). */
    readonly next?: number;
    readonly ok: boolean;
}
export interface AppSignIn {
    /** password (a username or an email). */
    readonly methods: string[];
    /** A login that names a device gets a refresh token. */
    readonly staysSignedIn: boolean;
    /** Where the App signs people in, when it is published. */
    readonly url: string | null;
}
export interface AppInfo {
    readonly app: string;
    /** cmd or skin. */
    readonly kind: string;
    /** The App's display name. */
    readonly name: string;
    readonly signIn: AppSignIn;
}
export interface AppThreadFile {
    /** true when you may save this file (mode=download): people always; an App guest only with files:download in the room. Absent on a hidden file. */
    readonly canDownload?: boolean;
    /** file. */
    readonly kind: string;
    readonly line?: number;
    readonly lineEnd?: number;
    /** Content type from the name (image/png, application/pdf, text/markdown, ...). */
    readonly mime?: string;
    /** Milliseconds since the epoch: the file's modified time when it was sent. */
    readonly modifiedAt?: number;
    readonly name?: string;
    /** Bytes, when it was sent. */
    readonly size?: number;
    /** ok, changed (since it was sent), gone (moved or deleted) or hidden (not shared with you: no name, type or size). */
    readonly state: string;
}
export interface AppPostAuthor {
    /** owner, user (a Connect login) or guest (an App pass or guest login). */
    readonly kind: string;
    readonly login: string;
    /** The owner's display name, on an owner's answer only. */
    readonly name?: string;
}
export interface ChoiceOption {
    readonly label: string;
}
export interface AppChoiceCard {
    readonly allow_custom: boolean;
    readonly answer?: string;
    /** Seconds since the epoch, when answered. */
    readonly answeredAt?: number;
    readonly answeredBy?: AppPostAuthor;
    /** Seconds since the epoch, when a pending card expires. */
    readonly expiresAt?: number;
    readonly options: ChoiceOption[];
    readonly prompt: string;
    /** pending, answered, voided or expired. */
    readonly status: string;
}
export interface AppSecretCard {
    /** Seconds since the epoch, when answered. */
    readonly answeredAt?: number;
    readonly answeredBy?: AppPostAuthor;
    /** fetch, file or env. */
    readonly delivery?: string;
    /** Seconds since the epoch, when a pending card expires. */
    readonly expiresAt?: number;
    readonly hosts?: string[];
    readonly name: string;
    readonly prompt?: string;
    /** A k2v:// reference to the stored secret (never its value). */
    readonly ref?: string;
    /** workspace, server or session. */
    readonly scope?: string;
    readonly secretKind?: string;
    /** pending, set, voided or expired. */
    readonly status: string;
}
export interface AppPostWidget {
    readonly garden: string;
    readonly widget: string;
}
export interface AppThreadDoc {
    /** Files that ride the message; one your file rights don't cover is {kind, state: hidden}. Read one with thread.attachmentGet by its index. */
    readonly attachments?: AppThreadFile[];
    readonly author?: AppPostAuthor;
    /** The text (redacted); a card's prompt. */
    readonly body?: string;
    readonly choice?: AppChoiceCard;
    /** Seconds since the epoch (not milliseconds). */
    readonly created_at: number;
    readonly from: string;
    readonly id: string;
    /** text, choice or secret. */
    readonly kind: string;
    readonly secret?: AppSecretCard;
    readonly to?: string;
    /** Where it came from: thread, compose, card, msg, inbox, v1, … */
    readonly via?: string;
    readonly widget?: AppPostWidget;
}
export interface AppThreadItem {
    /** Always thread. */
    readonly collection?: string;
    readonly doc: AppThreadDoc;
    readonly id: string;
    readonly seq: number;
}
export interface AppThreadRemoved {
    readonly id: string;
    readonly seq: number;
}
export interface AppThreadPage {
    readonly addr: string;
    /** A delta's cursor, echoed. */
    readonly after_seq?: number;
    /** A delta only: items at or below after_seq changed since (cards answered, voided, expired). Upsert by id. */
    readonly changed?: AppThreadItem[];
    readonly collection?: string;
    /** The pinned Chat's conversation id: thread.events subscribes with it. */
    readonly conversation: string;
    /** A page: older items exist below it. A delta: newer items remain. */
    readonly has_more: boolean;
    readonly items: AppThreadItem[];
    readonly ok: boolean;
    readonly pastAddresses?: string[];
    /** A delta only: items that left this Thread. */
    readonly removed?: AppThreadRemoved[];
}
export interface AppItemError {
    readonly code: string;
    readonly hint: string;
}
export interface AppThreadLatestItem {
    readonly addr: string;
    /** Seconds since the epoch. */
    readonly at?: number | null;
    readonly author?: AppPostAuthor | null;
    readonly conversation?: string;
    readonly error?: AppItemError;
    readonly from?: string | null;
    readonly kind?: string | null;
    /** One line, at most 140 characters. */
    readonly preview?: string | null;
    readonly seq?: number | null;
    readonly via?: string | null;
}
export interface AppThreadLatest {
    readonly items: AppThreadLatestItem[];
    readonly ok: boolean;
}
export interface AppThreadChildCounts {
    readonly monitor: number;
    readonly shell: number;
    readonly subagent: number;
    readonly task: number;
}
export interface AppThreadTurnEnd {
    /** done, stopped or failed. */
    readonly reason: string;
}
export interface AppThreadTally {
    readonly cmd: number;
    readonly edit: number;
    readonly read: number;
    readonly search: number;
}
export interface AppThreadTurn {
    /** Background tasks running now. */
    readonly background: number;
    /** Children running now, by kind (numbers only). */
    readonly childCounts?: AppThreadChildCounts;
    /** Null while it runs; the turn's end frame says how it ended. */
    readonly end: AppThreadTurnEnd | null;
    /** Milliseconds since the epoch, rounded down to 10 s: the agent's last sign of life; null when unknown. */
    readonly evidenceAt?: number | null;
    /** Thread messages in this turn. */
    readonly messages: number;
    /** Why the turn is still open: lead, children, waiting or stale; null while delivering and on an end frame. */
    readonly openBecause?: string | null;
    /** delivering, working, tool (no line), thinking, waiting, stale or children. */
    readonly phase: string;
    /** Milliseconds since the epoch: when this phase began. */
    readonly phaseSince: number | null;
    /** This turn's frame counter (not a time): a later frame has a higher rev. */
    readonly rev: number;
    /** 2 since K2 0.46.5 (absent from an older server): the frame names its step kind, child counts and why it is open. */
    readonly schema?: number;
    /** Milliseconds since the epoch: the server clock when the frame was built. */
    readonly serverNow: number | null;
    /** Milliseconds since the epoch: the first message's receipt time (the clock). */
    readonly startedAt: number | null;
    /** working, monitoring, needs-you, unverifiable, stopped or idle. */
    readonly state: string;
    /** While phase is tool: read, search, cmd, edit, web, subagent or tool (never the tool's name or input); else null. */
    readonly stepKind?: string | null;
    /** Subagents running now. */
    readonly subagents: number;
    /** Subagents done in this turn. */
    readonly subagentsDone: number;
    /** Tools called in this turn, by kind (numbers only). */
    readonly tally: AppThreadTally;
    /** The Thread message that started the turn. */
    readonly turnId: string;
    /** owner while the agent waits on the owner (never which prompt), else null. */
    readonly waitingOn: string | null;
}
export interface AppThreadActivity {
    readonly addr: string;
    /** The room's pinned Chat. */
    readonly conversation: string;
    readonly ok: boolean;
    /** The counts-only ear: the room's pinned-Chat Thread turn a Thread message started (no tool line, path, command or text). */
    readonly turn: AppThreadTurn | null;
}
export interface AppThreadActivityRooms {
    readonly ok: boolean;
    /** A map: room id to its pinned-Chat turn (the shape thread.activity answers, AppThreadTurn), or null for none; every asked room where the pass has thread:read and activity:read (rooms without the ear are left out). */
    readonly turns: unknown;
    /** Asked ids this pass can't read (the same whether or not they exist). */
    readonly unknown: string[];
}
export interface AppThreadAttach {
    readonly line?: number;
    readonly lineEnd?: number;
    /** Room-relative path (never a host path). */
    readonly relPath: string;
}
export interface AppThreadPosted {
    readonly addr: string;
    readonly author?: AppPostAuthor;
    readonly body?: string | null;
    readonly conversation: string;
    readonly from: string;
    readonly id: string;
    readonly kind: string;
    readonly ok: boolean;
    readonly seq: number;
    readonly to?: string | null;
    readonly via?: string | null;
}
export interface AppCardAnswered {
    readonly addr?: string;
    readonly agentCanSee?: boolean;
    readonly answer?: string;
    readonly conversation?: string;
    readonly id: string;
    readonly kind: string;
    readonly name?: string;
    readonly ok: boolean;
    readonly ref?: string;
    readonly seq?: number;
    readonly status: string;
    readonly trimmed?: boolean;
}
export interface AppCardVoided {
    readonly addr?: string;
    readonly conversation?: string;
    readonly id: string;
    readonly kind?: string;
    readonly ok: boolean;
    readonly seq?: number;
    readonly status: string;
}
export interface AppStreamFrame {
    /** The counts-only ear: the room's pinned-Chat Thread turn a Thread message started (no tool line, path, command or text). */
    readonly activity?: AppThreadTurn;
    readonly all?: boolean;
    readonly bodyBytes?: number;
    /** inbox_error: usage, too_many or too_many_subscriptions. */
    readonly code?: string;
    /** Always thread. */
    readonly collection?: string;
    /** The room's pinned Chat (the conversation thread.page answers for it). */
    readonly conversation?: string;
    /** inbox_subscribed: their pinned Chats, in the same order. */
    readonly conversations?: string[];
    readonly doc?: AppThreadDoc;
    readonly hint?: string;
    readonly id?: string;
    readonly kind: 'inbox_subscribed' | 'thread_message' | 'thread_activity' | 'inbox_error' | 'inbox_reset' | 'activity_reset';
    readonly limit?: number;
    readonly max?: number;
    /** inbox_reset: lagged; catch up with thread.page sinceSeq (or POST /cli/thread/catchup). */
    readonly reason?: string;
    /** inbox_subscribed: room id to its pinned Chat. */
    readonly resolved?: Record<string, string>;
    /** This socket's own counter, not a time: a later frame has a higher rev; it restarts with the socket. */
    readonly rev?: number;
    /** The room (its id in me.get) the item is in. */
    readonly room?: string;
    /** inbox_subscribed: the rooms this socket streams now. */
    readonly rooms?: string[];
    readonly seq?: number;
    /** Over 32 KiB: the body (or the whole doc) was left off; read the item with thread.page. */
    readonly truncated?: boolean;
    /** activity_reset (0.46.5): the socket fell behind and turn frames were dropped. A map: room id to that room's turn as it is now (the shape thread.activity answers, AppThreadTurn), or null for none. Replace what you show with it. */
    readonly turns?: unknown;
    /** inbox_subscribed: asked-for ids this pass can't read (the same whether or not they exist). */
    readonly unknown?: string[];
}
export interface AppThreadFileData {
    /** preview or thumb: the bytes. Show them as an image (an <img>, never inline markup). */
    readonly base64?: string;
    readonly eof?: boolean;
    /** thumb: pixels. */
    readonly height?: number;
    /** file (meta), text, image or pdf (preview, thumb). */
    readonly kind: string;
    /** range: bytes in this chunk. */
    readonly len?: number;
    readonly line?: number;
    readonly lineEnd?: number;
    /** The content type; a thumb is image/jpeg, image/png or a sanitized image/svg+xml. */
    readonly mime?: string;
    /** Milliseconds since the epoch (meta). */
    readonly modifiedAt?: number;
    readonly name?: string;
    /** range: the first byte. */
    readonly offset?: number;
    readonly ok?: boolean;
    /** An SVG with scripts, event handlers and outside links removed. */
    readonly sanitized?: boolean;
    /** Bytes now. */
    readonly size?: number;
    /** ok, changed (since it was sent), gone (moved or deleted) or hidden (not shared with you: no name, type or size). */
    readonly state?: string;
    /** preview of a text file: the first 512 KB, vault values redacted. */
    readonly text?: string;
    readonly truncated?: boolean;
    /** thumb: pixels. */
    readonly width?: number;
}
export interface AppCatchupAsk {
    readonly addr: string;
    readonly afterSeq?: number;
    readonly newest?: boolean;
}
export interface AppCatchupError {
    readonly code: string;
    readonly hint?: string;
}
export interface AppCatchupEntry {
    readonly addr: string;
    readonly after_seq?: number;
    readonly changed?: AppThreadItem[];
    readonly conversation?: string;
    readonly deferred?: boolean;
    readonly error?: AppCatchupError;
    readonly has_more?: boolean;
    readonly items?: AppThreadItem[];
    readonly removed?: AppThreadRemoved[];
}
export interface AppCatchup {
    /** Some entries are deferred (the 4 MiB budget): call again with those. */
    readonly has_more?: boolean;
    readonly ok: boolean;
    readonly threads: AppCatchupEntry[];
}
export interface AppReadState {
    /** The address as asked. */
    readonly addr?: string;
    /** The cursor (or the mute) moved. */
    readonly changed?: boolean;
    /** The conversation id. */
    readonly id?: string;
    readonly lastReadSeq: number;
    readonly lastSeq: number;
    readonly muted?: boolean;
    /** Seconds since the epoch, when a timed mute ends. */
    readonly mutedUntil?: number;
    readonly ok: boolean;
    /** The badge after the change: a guest's over its own rooms only. */
    readonly total?: number;
    readonly unread: number;
    /** Cards waiting for an answer. */
    readonly waiting?: number;
    /** Cards waiting for an answer, muted or not. */
    readonly waitingTotal?: number;
}
export interface AppUnreadRow {
    /** The room's handle (absent for a Garden row that isn't a room's Chat). */
    readonly addr?: string;
    /** The conversation id. */
    readonly id: string;
    readonly lastReadSeq: number;
    readonly lastSeq: number;
    /** Muted: its unread leaves the total. */
    readonly muted?: boolean;
    /** Seconds since the epoch, when a timed mute ends. */
    readonly mutedUntil?: number;
    readonly unread: number;
    /** Cards waiting for an answer. */
    readonly waiting?: number;
}
export interface AppMutedThread {
    /** The room's handle. */
    readonly addr?: string;
    /** The conversation id. */
    readonly id: string;
    /** Seconds since the epoch, when a timed mute ends. */
    readonly mutedUntil?: number;
}
export interface AppUnread {
    /** The person's opaque badge key on this server (the same on every surface they sign in on); absent for an App's own pass, which is not a person. */
    readonly badgeKey?: string;
    readonly byConversation: AppUnreadRow[];
    readonly muted?: AppMutedThread[];
    readonly ok: boolean;
    /** The badge: unread across unmuted Threads plus every waiting card. */
    readonly total: number;
    /** Cards waiting for an answer, muted or not. */
    readonly waitingTotal?: number;
}
export interface AppLink {
    readonly input: string;
    /** file or directory (linkable only). */
    readonly kind?: string;
    readonly linkable: boolean;
    /** Room-relative (linkable only); read it with files.read. */
    readonly rel?: string;
}
export interface AppLinks {
    readonly links: AppLink[];
}
export interface AppPinnedChat {
    readonly freshSpawn: boolean;
    readonly pendingSessionDiscovery: boolean;
    /** The agent's model CLI ('' when none). */
    readonly provider: string;
    readonly resumedExisting: boolean;
    readonly reused: boolean;
}
export interface AppFileEntry {
    /** A symlinked folder reads false. */
    readonly isDirectory: boolean;
    /** Milliseconds since the epoch. */
    readonly modifiedAt: number;
    readonly name: string;
    readonly relPath: string;
    /** Bytes. */
    readonly size: number;
}
export interface AppFileText {
    readonly content: string;
    readonly name: string;
    readonly relPath: string;
}
export interface AppFileBase64 {
    readonly base64: string;
}
export interface AppFileRange {
    readonly base64: string;
    readonly eof: boolean;
    /** Bytes returned. */
    readonly len: number;
    /** The file's size in bytes. */
    readonly size: number;
}
export interface AppFileStat {
    /** Lower-case extension without the dot ('' for none or a folder). */
    readonly ext: string;
    /** Symlinks are followed. */
    readonly kind: 'file' | 'directory' | 'other';
    /** A content type from the extension (application/octet-stream when unknown, inode/directory for a folder). */
    readonly mime: string;
    /** Milliseconds since the epoch. */
    readonly modifiedAt: number;
    readonly name: string;
    readonly relPath: string;
    /** Bytes (0 for a folder). */
    readonly size: number;
}
export interface AppDownloadItem {
    /** A content type from the extension (application/octet-stream when unknown). */
    readonly mime: string;
    /** Milliseconds since the epoch. */
    readonly modifiedAt: number;
    readonly name: string;
    /** Room-relative path (never a host path). */
    readonly relPath: string;
    /** Bytes. */
    readonly size: number;
}
export interface AppDownloads {
    /** Files the room's download entries expose to this login (a folder entry walks 8 levels); missing entries are left out. */
    readonly items: AppDownloadItem[];
    readonly ok: boolean;
    /** true past 500 files in all. */
    readonly truncated: boolean;
}
export interface AppFileDownload {
    readonly base64: string;
    readonly eof: boolean;
    /** Bytes returned. */
    readonly len: number;
    /** A content type from the extension (application/octet-stream when unknown). */
    readonly mime: string;
    /** The file's name, for the saved file. */
    readonly name: string;
    /** This chunk's byte offset. */
    readonly offset: number;
    readonly ok: boolean;
    /** The file's size in bytes. */
    readonly size: number;
}
export interface AppFileDone {
    readonly success: boolean;
}
export interface AppFileUploaded {
    /** Room-relative path where it landed. */
    readonly relPath: string;
    /** Bytes written (catalog v22+). */
    readonly size?: number;
}
export interface AppFileChunk {
    readonly done: boolean;
    /** Bytes K2 holds. */
    readonly received?: number;
    /** Where the file landed, when done. */
    readonly relPath?: string;
    /** The landed file's bytes, when done (catalog v22+). */
    readonly size?: number;
}
export interface AppFileLanded {
    /** Room-relative path where it landed. */
    readonly relPath: string;
}
export interface AppFsFrame {
    readonly kind: 'fs_changed' | 'workspace_resources_changed';
    /** Room-relative paths that changed ('.' = the root). */
    readonly paths?: string[];
    readonly workspace: string;
    readonly workspaceId?: string;
}
export interface AppResource {
    /** Seconds since the epoch. */
    readonly addedAt: number;
    readonly fileName: string;
    /** Room-relative. */
    readonly filePath: string;
    readonly missing: boolean;
}
export interface AppResources {
    readonly docs: AppResource[];
    readonly ok: boolean;
    readonly workspaceId: string;
}
export interface AppResourceAdded {
    readonly fileName: string;
    readonly filePath: string;
    readonly ok: boolean;
    readonly workspaceId: string;
}
export interface AppResourceRemoved {
    readonly filePath: string;
    readonly ok: boolean;
    readonly workspaceId: string;
}
export interface AppTicket {
    readonly agentName: string;
    readonly answer: string | null;
    /** Seconds since the epoch. */
    readonly answeredAt: number | null;
    readonly assignees: string[];
    readonly body: string | null;
    readonly briefBytes: number | null;
    readonly commentCount: number;
    /** Seconds since the epoch. */
    readonly createdAt: number;
    readonly hasBrief: boolean;
    readonly id: string;
    /** question, approval or fyi. */
    readonly kind: string;
    readonly options: string[] | null;
    /** 1 (low) to 5. */
    readonly priority: number;
    readonly projectId: string;
    /** waiting, answered, resolved, dismissed, planned or needs_discussion. */
    readonly status: string;
    readonly title: string;
    /** Seconds since the epoch. */
    readonly updatedAt: number;
}
export interface AppTickets {
    readonly items: AppTicket[];
    readonly ok: boolean;
}
export interface AppTicketBrief {
    readonly bytes: number;
    /** Seconds since the epoch. */
    readonly createdAt: number;
    readonly html: string;
    readonly sanitizer: string;
    readonly sha256: string;
    readonly text: string;
}
export interface AppTicketComment {
    /** Seconds since the epoch. */
    readonly at: number;
    /** The message's files as you see them; one your file rights don't cover is {kind, state: hidden}. Read one with tickets.attachment by the message id and its index. */
    readonly attachments?: AppThreadFile[];
    readonly author: string;
    readonly body: string;
    /** The message's id (tickets.attachment reads its files). */
    readonly id: string;
}
export interface AppTicketDetail {
    readonly agentName: string;
    readonly answer: string | null;
    /** Seconds since the epoch. */
    readonly answeredAt: number | null;
    readonly assignees: string[];
    readonly body: string | null;
    readonly brief?: AppTicketBrief | null;
    readonly briefBytes: number | null;
    readonly commentCount: number;
    readonly comments: AppTicketComment[];
    /** Seconds since the epoch. */
    readonly createdAt: number;
    /** Files across the ticket's messages. */
    readonly fileCount?: number;
    readonly hasBrief: boolean;
    readonly id: string;
    /** question, approval or fyi. */
    readonly kind: string;
    readonly ok: boolean;
    readonly options: string[] | null;
    /** 1 (low) to 5. */
    readonly priority: number;
    readonly projectId: string;
    /** waiting, answered, resolved, dismissed, planned or needs_discussion. */
    readonly status: string;
    readonly title: string;
    /** Seconds since the epoch. */
    readonly updatedAt: number;
    /** The agent's display name. */
    readonly workspace?: string | null;
}
export interface AppTicketWarning {
    readonly code: string;
    readonly hint: string;
}
export interface AppTicketCreated {
    readonly agentName: string;
    readonly answer: string | null;
    /** Seconds since the epoch. */
    readonly answeredAt: number | null;
    readonly assignees: string[];
    /** The opening message's files as you see them (always present on a 0.46.6 daemon). */
    readonly attachments?: AppThreadFile[];
    readonly body: string | null;
    readonly briefBytes: number | null;
    readonly commentCount: number;
    /** Seconds since the epoch. */
    readonly createdAt: number;
    readonly fileCount?: number;
    readonly hasBrief: boolean;
    readonly id: string;
    /** question, approval or fyi. */
    readonly kind: string;
    readonly ok: boolean;
    readonly options: string[] | null;
    /** 1 (low) to 5. */
    readonly priority: number;
    readonly projectId: string;
    /** waiting, answered, resolved, dismissed, planned or needs_discussion. */
    readonly status: string;
    readonly title: string;
    /** Seconds since the epoch. */
    readonly updatedAt: number;
    readonly warnings: AppTicketWarning[];
    readonly workspace?: string | null;
}
export interface AppTicketCommented {
    readonly answered: boolean;
    /** This message's files as you see them. */
    readonly attachments?: AppThreadFile[];
    readonly author: string;
    readonly commentId: string;
    readonly delivered: boolean;
    readonly deliveryReason: string | null;
    readonly id: string;
    readonly ok: boolean;
    readonly status: string;
}
export interface AppTicketAnswered {
    readonly answer: string | null;
    /** Seconds since the epoch. */
    readonly answeredAt: number | null;
    /** The answer's files as you see them. */
    readonly attachments?: AppThreadFile[];
    readonly commentId?: string;
    readonly delivered: boolean;
    readonly deliveryReason: string | null;
    readonly id: string;
    readonly ok: boolean;
    readonly status: string;
}
export interface AppTicketStatus {
    readonly id: string;
    readonly ok: boolean;
    readonly status: string;
}
export interface AppTicketAssigned {
    readonly assignees: string[];
    readonly id: string;
    readonly ok: boolean;
    readonly warnings: AppTicketWarning[];
}
export interface AppWikiLink {
    readonly missing?: boolean;
    readonly source: string;
    readonly target: string;
}
export interface AppWikiNode {
    readonly aliases: string[];
    readonly exists: boolean;
    readonly id: string;
    readonly tags: string[];
    readonly title: string;
}
export interface AppWikiIndex {
    /** Seconds since the epoch, as a string. */
    readonly generatedAt: string;
    readonly links: AppWikiLink[];
    readonly nodes: AppWikiNode[];
    readonly noteCount: number;
    readonly scope: string;
    readonly wikiRel: string;
}
export interface AppWikiNote {
    readonly aliases: string[];
    /** Markdown without its frontmatter. */
    readonly body: string;
    readonly id: string;
    readonly tags: string[];
    readonly title: string;
}
export interface AppStoreCollections {
    readonly collections: string[];
    readonly ok: boolean;
}
export interface AppStoreDoc {
    readonly collection: string;
    readonly doc: unknown;
    readonly id: string;
    readonly ok: boolean;
}
export interface AppStoreDocItem {
    readonly doc: unknown;
    readonly id: string;
}
export interface AppStoreDocs {
    readonly collection: string;
    readonly docs: AppStoreDocItem[];
    readonly ok: boolean;
}
export interface AppDbTables {
    readonly ok: boolean;
    readonly tables: string[];
}
export interface AppDbRows {
    readonly id?: string;
    readonly ok: boolean;
    /** With id: that row. */
    readonly row?: Record<string, unknown>;
    /** Without id: the rows. */
    readonly rows?: Array<Record<string, unknown>>;
    readonly table: string;
}
export interface AppDbRow {
    readonly id?: string;
    readonly ok: boolean;
    /** The row as Postgres row_to_json writes it. */
    readonly row: Record<string, unknown>;
    readonly table: string;
}
export interface AppDbQuery {
    /** The first row's keys (alphabetical); absent for a write with no RETURNING. */
    readonly columns?: string[];
    readonly rowCount: number;
    readonly rows: Array<Record<string, unknown>>;
}
export interface AppHeartbeat {
    /** Seconds since the epoch. */
    readonly createdAt: number;
    /** pinned or own. */
    readonly delivery: string;
    /** failures, wakeup_missing or wakeup_empty. */
    readonly disabledReason: string | null;
    readonly enabled: boolean;
    readonly firing: boolean;
    /** hourly, daily, weekly, monthly, yearly or scheduled. */
    readonly frequency: string;
    /** RFC 3339. */
    readonly lastFired: string | null;
    readonly name: string;
    /** RFC 3339. */
    readonly nextFireAt: string | null;
    readonly scheduleError: string | null;
    /** The schedule (frequency-specific keys). */
    readonly spec: Record<string, unknown> | null;
    readonly waitDetail: string | null;
    /** Why it isn't firing now: scheduled, in_flight, backoff, disabled:user, … */
    readonly waitReason: string | null;
    /** RFC 3339. */
    readonly waitSince: string | null;
}
export interface AppHeartbeatDetail {
    /** Seconds since the epoch. */
    readonly createdAt: number;
    /** pinned or own. */
    readonly delivery: string;
    /** failures, wakeup_missing or wakeup_empty. */
    readonly disabledReason: string | null;
    readonly enabled: boolean;
    readonly firing: boolean;
    /** hourly, daily, weekly, monthly, yearly or scheduled. */
    readonly frequency: string;
    /** WAKEUP.md without its frontmatter (with files:read). */
    readonly instructions?: string;
    /** true when the pass lacks files:read. */
    readonly instructionsHidden?: boolean;
    /** RFC 3339. */
    readonly lastFired: string | null;
    readonly name: string;
    /** RFC 3339. */
    readonly nextFireAt: string | null;
    readonly scheduleError: string | null;
    /** The schedule (frequency-specific keys). */
    readonly spec: Record<string, unknown> | null;
    readonly waitDetail: string | null;
    /** Why it isn't firing now: scheduled, in_flight, backoff, disabled:user, … */
    readonly waitReason: string | null;
    /** RFC 3339. */
    readonly waitSince: string | null;
}
export interface AppHeartbeatFire {
    /** app:<user>, app-token:<name>, agent:<handle>, user:<name>, owner-token, or null for the scheduler. */
    readonly actor: string | null;
    readonly decision: string;
    /** Milliseconds. */
    readonly durationMs: number | null;
    /** RFC 3339 with offset. */
    readonly firedAt: string;
    readonly name: string | null;
    readonly reason: string | null;
}
export interface AppHeartbeatArchived {
    readonly archived: boolean;
    readonly name: string;
    readonly success: boolean;
}
export interface AppHeartbeatFired {
    readonly decision: string;
    readonly name: string;
    readonly reason: string | null;
    readonly success: boolean;
}
export interface AppActivityCounts {
    readonly commands: number;
    readonly subagents: number;
    readonly tools: number;
}
export interface AppActivitySession {
    readonly agentName: string;
    readonly counts: AppActivityCounts;
    readonly paneGroupId: string | null;
    /** Milliseconds since the epoch. */
    readonly since: number | null;
    readonly status: string;
}
export interface AppActivity {
    /** Milliseconds since the epoch. */
    readonly serverNow: number;
    readonly sessions: AppActivitySession[];
    /** Milliseconds since the epoch. */
    readonly since: number | null;
    /** working, permission, idle or unknown. */
    readonly status: string;
    readonly workspace: string;
}
export interface AppActivityFrame {
    readonly agentName?: string;
    /** app_deployed: the App that went live (refetch app.self). */
    readonly app?: string;
    readonly change?: string;
    readonly counts?: AppActivityCounts;
    readonly hasBrief?: boolean;
    /** ticket_changed: the ticket. */
    readonly id?: string;
    readonly kind: 'session_activity_changed' | 'heartbeat_changed' | 'ticket_changed' | 'rooms_changed' | 'room_members_changed' | 'app_deployed' | 'resync';
    readonly live?: boolean;
    /** heartbeat_changed: the heartbeat. */
    readonly name?: string;
    readonly paneGroupId?: string | null;
    /** Milliseconds since the epoch. */
    readonly serverNow?: number;
    /** Milliseconds since the epoch. */
    readonly since?: number | null;
    readonly status?: string;
    readonly via?: string | null;
    readonly workspace?: string;
}
export interface AppAgentIdentity {
    readonly canEditHandle: boolean;
    readonly canEditIdentity: boolean;
    /** #rrggbb. */
    readonly color: string | null;
    readonly displayName: string;
    readonly handle: string;
    /** A data: URL. */
    readonly image: string | null;
    /** The agent's handle. */
    readonly workspace: string;
}
export interface AppAgentIdentityChanged {
    readonly canEditHandle: boolean;
    readonly canEditIdentity: boolean;
    readonly changed: string[];
    /** #rrggbb. */
    readonly color: string | null;
    readonly displayName: string;
    readonly handle: string;
    /** A data: URL. */
    readonly image: string | null;
    readonly ok: boolean;
    /** The agent's handle. */
    readonly workspace: string;
}
export interface AppAgentHandleChanged {
    readonly canEditHandle: boolean;
    readonly canEditIdentity: boolean;
    readonly changed: string[];
    /** #rrggbb. */
    readonly color: string | null;
    readonly displayName: string;
    readonly handle: string;
    /** A data: URL. */
    readonly image: string | null;
    readonly ok: boolean;
    readonly previousHandle: string;
    /** The agent's handle. */
    readonly workspace: string;
}
export interface RoomMember {
    /** host (the box login) or the App id the grant is for. */
    readonly app: string;
    readonly caps: string[];
    /** The full name when set, else the username; null when names are hidden. */
    readonly displayName: string | null;
    /** agent: a login marked as an agent (k2 app user agent). */
    readonly kind: 'guest' | 'agent';
    /** Owner, agent and Garden views only (never an App's): an agent set this person up and the extra access the owner gave waits for their password reset. */
    readonly pendingReset?: boolean;
    /** The role on the grant; null = a grant with rooms only (Thread). */
    readonly role: string | null;
    /** Owner, agent and Garden views only: the workspace whose agent set this person up (their access stays there until an owner reset), or null. */
    readonly tiedTo?: string | null;
    /** null when the owner hides names in this room (App callers only). */
    readonly username: string | null;
}
export interface RoomMembers {
    /** Connect logins have no rooms: always the one line "admins of this box". */
    readonly admins: string;
    /** How many members. */
    readonly count: number;
    readonly members: RoomMember[];
    /** The owner hides guests' names in this room: an App sees null names. A Garden (the owner) always sees them. */
    readonly namesHidden: boolean;
    /** The room's project id. */
    readonly room: string;
    /** The room's handle. */
    readonly workspace: string;
}
export interface AppSelf {
    /** The App id. */
    readonly app: string;
    /** When the live deployment went live, seconds since the epoch. */
    readonly deployedAt: number | null;
    /** The live deployment id. */
    readonly deployment: string | null;
    readonly health: 'running' | 'starting' | 'crashlooping' | 'exited' | 'stopped' | 'suspended';
    /** The live Seed's hash; null before the first deploy. */
    readonly liveSeed: string | null;
    /** The App row's state (running, stopped, crashlooping, failed, installing). */
    readonly state: string;
    /** The handle of the workspace that owns the App. */
    readonly workspace: string;
}
export interface AppLiveUse {
    readonly connections: number;
    readonly subscriptions: number;
}
export interface AppQuotas {
    readonly bodyBytes: number;
    readonly burst: number;
    /** Sustained calls per second. */
    readonly rps: number;
    readonly sockets: number;
    readonly subscriptions: number;
    readonly uploadBytes: number;
}
export interface AppCallTotals {
    readonly bytesIn: number;
    readonly bytesOut: number;
    readonly calls: number;
    readonly denied: number;
}
export interface AppVerbUse {
    readonly calls: number;
    /** Refused calls (status 400 or more). */
    readonly denied: number;
    /** METHOD /path, as the gateway recorded it. */
    readonly route: string;
    /** The catalog verb, or null for a call that isn't one (a refused route). */
    readonly verb: string | null;
}
export interface AppUsage {
    /** The App id. */
    readonly app: string;
    /** The window the counts cover, in hours. */
    readonly hours: number;
    readonly live: AppLiveUse;
    readonly quotas: AppQuotas;
    readonly totals: AppCallTotals;
    readonly verbs: AppVerbUse[];
}
export interface AppChatPass {
    readonly app?: string;
    readonly caps?: string[];
    /** RFC 3339. */
    readonly expiresAt: string;
    /** Milliseconds the pass lives (a duration). */
    readonly expiresInMs: number;
    readonly jti: string;
    readonly ok: boolean;
    /** The Sew chat pass (a signed JWT) for this guest; hand it to Sew, never to K2. */
    readonly pass: string;
    /** Always false: a chat pass never proves a person is present. */
    readonly personProof: boolean;
    readonly provenance: string;
    readonly sewRole: string;
    readonly sid: string;
    /** k2:principal:<id> for an App guest. */
    readonly sub: string;
}
export interface AppGuestChat {
    /** The App's service id. */
    readonly app: string | null;
    /** This server's Sew keeps this App's sessions to its own Spaces (app_scoped_sessions). */
    readonly appScoped: boolean;
    /** true only when a set wrote something. */
    readonly changed: boolean;
    /** Whether the person's chat grant is on. */
    readonly enabled: boolean;
    /** app = this App gave it; owner = the owner (or another App) did; null = no chat. */
    readonly managedBy: string | null;
    /** This App's ceiling (the owner's grant). */
    readonly maxMembers: number | null;
    /** People this App gave chat to. */
    readonly members: number | null;
    readonly ok: boolean;
    /** The person's id: k2:principal:<id> in Sew. Store this, never the email. */
    readonly principalId: string;
    /** member | guest; null = no chat. */
    readonly scope: 'member' | 'guest' | null;
    readonly username: string;
    /** Until appScoped: this App can see all chat this person can. */
    readonly warning: string | null;
}
export interface AppChatMember {
    readonly app: string;
    /** k2:principal:<principalId>, the stable key in Sew. */
    readonly externalId: string;
    /** Sew's member id: open DMs, ring calls, mention or add members with it. */
    readonly memberId: string;
    readonly ok: boolean;
    readonly principalId: string;
    readonly state: 'active' | 'deactivated';
    readonly username: string;
}
export interface AppChatWhoisRow {
    /** The person was deleted in K2: show a deleted user. */
    readonly deleted: boolean;
    /** The name Sew shows (K2's profile display name, else full name, else username). */
    readonly displayName?: string;
    /** The K2 profile picture's hash, when there is one. */
    readonly imageHash?: string;
    readonly isThisAppUser: boolean;
    readonly kind: 'person' | 'agent' | 'unknown';
    readonly memberId: string;
    /** Only for this App's users. */
    readonly principalId?: string;
    /** This App's users, and its own room agent's handle. */
    readonly username?: string;
}
export interface AppChatWhois {
    readonly app: string;
    readonly members: AppChatWhoisRow[];
    readonly ok: boolean;
}
export interface AppChatChannelNotAdded {
    /** not_found (not this App's user), no_chat or guest (give chat as a member first), not_provisioned (retry in a few seconds), deactivated, not_space_member. */
    readonly reason: string;
    readonly username: string;
}
export interface AppChatChannel {
    /** Listed usernames in the channel now (added by this call or already in). */
    readonly added: string[];
    readonly app: string;
    /** Sew's channel id. */
    readonly channelId: string;
    /** false: the channel was there already (same Space and slug). */
    readonly created: boolean;
    /** The channel's current name (a repeat never renames it). */
    readonly name: string;
    readonly notAdded: AppChatChannelNotAdded[];
    readonly ok: boolean;
    /** Whether it is private (current; a repeat never changes it). */
    readonly private: boolean;
    /** Listed usernames banned in the Space: K2 never adds them again. */
    readonly skipped: string[];
    readonly slug: string;
    readonly spaceId: string;
}
export interface AppProjectMember {
    readonly handle: string;
    /** The member agent's project id. */
    readonly room: string;
}
export interface AppProject {
    /** This pass holds project:write in every member room. */
    readonly canPost: boolean;
    readonly color: string | null;
    readonly groupId: string;
    readonly lastReadSeq: number | null;
    /** The newest message's seq (0 = no messages). */
    readonly lastSeq: number;
    readonly members: AppProjectMember[];
    readonly muted: boolean | null;
    readonly name: string;
    /** The point of contact agent's handle (it answers the chat). */
    readonly poc: string | null;
    /** Your unread lines (capped at 100); null for a pass with no person (an App's own pass). */
    readonly unread: number | null;
}
export interface AppProjectList {
    readonly ok: boolean;
    readonly projects: AppProject[];
}
export interface AppProjectMessage {
    /** owner, a Connect login, an agent's workspace name, or guest:<username> for an App guest's line. */
    readonly author: string;
    readonly authorKind: 'person' | 'agent' | 'guest';
    readonly body: string;
    /** Seconds since the epoch. */
    readonly createdAt: number;
    readonly groupId: string;
    readonly id: string;
    /** 1..n per project, in read order (the paging cursor). */
    readonly seq: number;
}
export interface AppProjectPage {
    readonly after_seq?: number;
    readonly before_seq?: number;
    /** Always empty: project messages never change. */
    readonly changed?: string[];
    readonly groupId: string;
    /** afterSeq: newer messages remain; beforeSeq: older ones remain. */
    readonly has_more?: boolean;
    readonly messages: AppProjectMessage[];
    readonly ok: boolean;
    /** Always empty. */
    readonly removed?: string[];
    readonly truncated: boolean;
}
export interface AppProjectPosted {
    /** guest:<username>. */
    readonly author: string;
    readonly authorKind: 'guest';
    readonly body: string;
    /** Seconds since the epoch. */
    readonly createdAt: number;
    /** The project's point of contact agent got the line. */
    readonly delivered: boolean;
    readonly deliveryReason: string | null;
    readonly groupId: string;
    readonly groupName: string;
    readonly id: string;
    readonly ok: boolean;
    readonly seq: number;
}
export interface AppProjectReadState {
    /** The cursor moved. */
    readonly changed: boolean;
    readonly groupId: string;
    readonly lastReadSeq: number;
    readonly lastSeq: number;
    readonly muted: boolean;
    readonly ok: boolean;
    readonly unread: number;
}
export interface AppProjectFrame {
    /** project: a new line (seq, doc); read: your own cursor moved on some surface (the read fields). */
    readonly collection: 'project' | 'read';
    readonly doc?: AppProjectMessage;
    readonly groupId: string;
    /** The line's id; on a read frame, the project's groupId. */
    readonly id: string;
    readonly lastReadSeq?: number;
    readonly lastSeq?: number;
    readonly muted?: boolean;
    readonly seq?: number;
    readonly unread?: number;
    /** Always 0: project chats have no cards. */
    readonly waiting?: number;
}
/** The shared error codes (catalog `errors`). */
export declare const APP_ERROR_CODES: readonly ["cap_not_granted", "not_bound", "rate_limited", "too_large", "unknown_verb", "verb_unavailable", "verb_local", "not_exposed", "sending_off", "server_not_saved", "unreachable", "removed", "no_history", "failed", "conflict", "space_full", "value_too_large", "bad_key", "not_granted", "resync", "author_unknown", "stale_widget", "store_full", "bad_tx", "tx_expired", "busy", "paused", "blob_missing", "bad_args", "bad_shape", "pass_required", "pass_invalid", "pass_wrong_link", "not_on_link", "json_required", "skin_room", "usage", "not_found", "bad_login", "locked", "card_not_pending", "guest_required", "no_app_pass", "no_app_key", "no_chat_grant", "no_chat_caps", "app_claim_invalid", "not_app_granted", "chat_off_for_person", "shared_person", "has_assistants", "chat_limit", "username_unusable", "chat_not_app_scoped", "no_chat_member", "sew_feature_missing", "space_unbound", "app_mismatch", "chat_unavailable", "app_pass_required", "grant_required", "outside_app_rooms", "above_role", "has_role", "confirm_required", "declined", "refresh_invalid", "refresh_reused", "person_pass_required", "wallet_off", "push_off", "not_for_host", "channel_unavailable", "push_devices_full", "push_not_configured", "too_many_recipients", "push_queue_full", "airgap", "vault_closed", "token_in_use", "pairing_off", "pairing_from_device", "reauth_required", "device_signin_off", "host_not_paired", "no_public_host", "pairing_not_served", "pair_code_invalid", "pairing_not_from_browser", "duplicate_label", "needs_editor", "retired", "card_person_only", "edit_off", "type_refused", "type_mismatch", "bad_image", "agent_login", "email_off", "no_sender", "sender_not_granted", "system_sender", "recipient_out_of_scope", "header_not_allowed", "message_too_large", "domain_not_ready", "dmarc_missing", "relay_not_ready", "idempotency_conflict", "no_mail_path"];
export type AppErrorCode = (typeof APP_ERROR_CODES)[number];
/** Every App verb: its arguments and its answer (or frame). */
export interface AppVerbs {
    'me.get': {
        args: Record<string, never>;
        value: AppMe;
    };
    'roomAgents.list': {
        args: Record<string, never>;
        value: AppRoomAgents;
    };
    'guests.login': {
        args: {
            app?: string;
            device?: AppDevice;
            password: string;
            username: string;
        };
        value: AppGuestLogin;
    };
    'guests.rooms': {
        args: {
            app?: string;
            username: string;
        };
        value: AppGuestRooms;
    };
    'guests.setRooms': {
        args: {
            app?: string;
            rooms: string[];
            username: string;
        };
        value: AppGuestRooms;
    };
    'app.rooms': {
        args: {
            app?: string;
        };
        value: AppRooms;
    };
    'guests.list': {
        args: {
            app?: string;
            cursor?: string;
            limit?: number;
        };
        value: AppGuestList;
    };
    'password.change': {
        args: {
            oldPassword: string;
            password: string;
            stay?: boolean;
        };
        value: AppPasswordChanged;
    };
    'wallet.list': {
        args: Record<string, never>;
        value: WalletList;
    };
    'wallet.status': {
        args: {
            id: string;
        };
        value: WalletItemAnswer;
    };
    'wallet.add': {
        args: {
            key?: string;
            label?: string;
            provider: string;
            type?: 'llm_subscription' | 'llm_api_key';
        };
        value: WalletSignIn;
    };
    'wallet.login': {
        args: {
            id: string;
        };
        value: WalletSignIn;
    };
    'wallet.loginStatus': {
        args: {
            loginId: string;
        };
        value: WalletLoginStatus;
    };
    'wallet.loginInput': {
        args: {
            loginId: string;
            text: string;
        };
        value: AppOk;
    };
    'wallet.loginCancel': {
        args: {
            loginId: string;
        };
        value: WalletLoginStatus;
    };
    'wallet.use': {
        args: {
            id?: string | null;
            provider: string;
        };
        value: WalletUse;
    };
    'wallet.remove': {
        args: {
            id: string;
        };
        value: WalletRemoved;
    };
    'guests.logout': {
        args: {};
        value: AppOk;
    };
    'guests.refresh': {
        args: {
            app?: string;
            refresh: string;
        };
        value: AppDeviceSession;
    };
    'guests.devices': {
        args: {};
        value: AppDevices;
    };
    'guests.revokeDevice': {
        args: {
            id: string;
        };
        value: AppOk;
    };
    'profile.get': {
        args: {
            ifImageHash?: string;
            image?: string;
        };
        value: AppProfile;
    };
    'profile.update': {
        args: {
            displayName?: string | null;
            fullName?: string | null;
        };
        value: AppProfileUpdated;
    };
    'profile.setImage': {
        args: {
            image: string;
        };
        value: AppProfileImage;
    };
    'profile.clearImage': {
        args: {};
        value: AppOk;
    };
    'profile.sessions': {
        args: {};
        value: AppSessions;
    };
    'profile.revokeSession': {
        args: {
            id: string;
        };
        value: AppOk;
    };
    'profile.signOutOthers': {
        args: {};
        value: AppSignedOut;
    };
    'push.register': {
        args: {
            channel?: 'app' | 'k2-companion';
            deviceId: string;
            environment?: 'production' | 'sandbox';
            mutedTopics?: string[];
            platform: 'apns' | 'fcm';
            previews?: boolean | null;
            pushToken: string;
        };
        value: AppPushDevice;
    };
    'push.unregister': {
        args: {
            deviceId: string;
        };
        value: AppPushRemoved;
    };
    'push.device': {
        args: {
            deviceId: string;
        };
        value: AppPushDevice;
    };
    'push.prefs': {
        args: {
            deviceId: string;
            mutedTopics?: string[];
            previews?: boolean | null;
        };
        value: AppPushDevice;
    };
    'push.send': {
        args: {
            androidChannel?: string;
            badge?: number;
            body?: string;
            collapseId?: string;
            data?: Record<string, string>;
            mutable?: boolean;
            priority?: 'high' | 'normal';
            silent?: boolean;
            sound?: string;
            threadId?: string;
            title?: string;
            to: AppPushTo;
            topic?: string;
            ttlSecs?: number;
        };
        value: AppPushQueued;
    };
    'push.status': {
        args: {
            sendId: string;
        };
        value: AppPushStatus;
    };
    'push.devices': {
        args: {};
        value: AppPushDevices;
    };
    'pairing.start': {
        args: {
            host?: string;
            password?: string;
        };
        value: AppPairingStart;
    };
    'pairing.status': {
        args: {
            pairingId: string;
        };
        value: AppPairingStatus;
    };
    'pairing.cancel': {
        args: {
            pairingId: string;
        };
        value: AppPairingCancelled;
    };
    'pairing.check': {
        args: {
            app?: string;
            code: string;
        };
        value: AppPairingCheck;
    };
    'pairing.redeem': {
        args: {
            app?: string;
            code: string;
            device: AppDevice;
            push?: AppPairingPush;
        };
        value: AppPairedSession;
    };
    'email.send': {
        args: {
            bcc?: string[];
            cc?: string[];
            from?: string;
            fromName?: string;
            headers?: Record<string, string>;
            html?: string;
            idempotencyKey?: string;
            replyTo?: string[];
            sendAt?: number;
            subject: string;
            tags?: string[];
            text?: string;
            to: string[];
        };
        value: AppEmailAccepted;
    };
    'email.status': {
        args: {
            id: string;
        };
        value: AppEmailStatus;
    };
    'email.log': {
        args: {
            before?: number;
            limit?: number;
            status?: string;
            tag?: string;
        };
        value: AppEmailLog;
    };
    'app.info': {
        args: {
            app: string;
        };
        value: AppInfo;
    };
    'password.forgot': {
        args: {
            username: string;
        };
        value: AppOk;
    };
    'password.reset': {
        args: {
            password: string;
            resetToken: string;
        };
        value: AppOk;
    };
    'thread.page': {
        args: {
            addr: string;
            afterSeq?: number;
            beforeSeq?: number;
            limit?: number;
            sinceSeq?: number;
        };
        value: AppThreadPage;
    };
    'thread.latest': {
        args: {
            addrs: string[];
        };
        value: AppThreadLatest;
    };
    'thread.activity': {
        args: {
            addr: string;
        };
        value: AppThreadActivity;
    };
    'thread.activityRooms': {
        args: {
            rooms: string;
        };
        value: AppThreadActivityRooms;
    };
    'thread.send': {
        args: {
            addr: string;
            attachments?: AppThreadAttach[];
            text?: string;
            via?: string;
        };
        value: AppThreadPosted;
    };
    'thread.answerCard': {
        args: {
            addr: string;
            answer?: string;
            cardId: string;
            confirm?: boolean;
            delivery?: string;
            hosts?: string[];
            scope?: string;
            secret?: string;
        };
        value: AppCardAnswered;
    };
    'thread.voidCard': {
        args: {
            addr: string;
            cardId: string;
        };
        value: AppCardVoided;
    };
    'thread.events': {
        args: {
            conversation: string;
        };
        value: AppThreadItem;
    };
    'thread.stream': {
        args: {
            rooms: string;
        };
        value: AppStreamFrame;
    };
    'thread.attachmentGet': {
        args: {
            addr: string;
            id: string;
            len?: number;
            mode?: 'meta' | 'preview' | 'thumb' | 'range' | 'download';
            n?: number;
            offset?: number;
            px?: number;
        };
        value: AppThreadFileData;
    };
    'thread.catchup': {
        args: {
            limit?: number;
            threads: AppCatchupAsk[];
        };
        value: AppCatchup;
    };
    'thread.setRead': {
        args: {
            addr: string;
            seq?: number;
        };
        value: AppReadState;
    };
    'thread.markAllRead': {
        args: {
            addr: string;
        };
        value: AppReadState;
    };
    'thread.unread': {
        args: Record<string, never>;
        value: AppUnread;
    };
    'thread.mute': {
        args: {
            addr: string;
            muted: boolean;
            until?: number;
        };
        value: AppReadState;
    };
    'files.resolveLinks': {
        args: {
            paths: string[];
            workspace: string;
        };
        value: AppLinks;
    };
    'thread.ensurePinned': {
        args: {
            workspace: string;
        };
        value: AppPinnedChat;
    };
    'files.list': {
        args: {
            relPath: string;
            showHidden?: boolean;
            workspace: string;
        };
        value: AppFileEntry[];
    };
    'files.read': {
        args: {
            relPath: string;
            workspace: string;
        };
        value: AppFileText;
    };
    'files.readBinary': {
        args: {
            relPath: string;
            workspace: string;
        };
        value: AppFileBase64;
    };
    'files.readRange': {
        args: {
            len?: number;
            offset?: number;
            relPath: string;
            workspace: string;
        };
        value: AppFileRange;
    };
    'files.stat': {
        args: {
            relPath: string;
            workspace: string;
        };
        value: AppFileStat;
    };
    'files.downloads': {
        args: {
            workspace: string;
        };
        value: AppDownloads;
    };
    'files.download': {
        args: {
            len?: number;
            offset?: number;
            relPath: string;
            workspace: string;
        };
        value: AppFileDownload;
    };
    'files.write': {
        args: {
            content: string;
            relPath: string;
            workspace: string;
        };
        value: AppFileDone;
    };
    'files.uploadBinary': {
        args: {
            base64: string;
            dir: string;
            filename: string;
            workspace: string;
        };
        value: AppFileUploaded;
    };
    'files.uploadChunk': {
        args: {
            base64?: string;
            dir: string;
            filename: string;
            isLast?: boolean;
            offset?: number;
            status?: boolean;
            totalBytes?: number;
            uploadId: string;
            workspace: string;
        };
        value: AppFileChunk;
    };
    'files.create': {
        args: {
            isDirectory?: boolean;
            relPath: string;
            workspace: string;
        };
        value: AppFileDone;
    };
    'files.copy': {
        args: {
            destination: string;
            sources: string[];
            workspace: string;
        };
        value: AppFileDone;
    };
    'files.move': {
        args: {
            destination: string;
            sources: string[];
            workspace: string;
        };
        value: AppFileDone;
    };
    'files.rename': {
        args: {
            newName: string;
            relPath: string;
            workspace: string;
        };
        value: AppFileLanded;
    };
    'files.events': {
        args: {
            workspace: string;
        };
        value: AppFsFrame;
    };
    'resources.list': {
        args: {
            workspace: string;
        };
        value: AppResources;
    };
    'resources.add': {
        args: {
            relPath: string;
            workspace: string;
        };
        value: AppResourceAdded;
    };
    'resources.remove': {
        args: {
            relPath: string;
            workspace: string;
        };
        value: AppResourceRemoved;
    };
    'tickets.list': {
        args: {
            all?: boolean;
            project: string;
            status?: string;
        };
        value: AppTickets;
    };
    'tickets.show': {
        args: {
            brief?: boolean;
            id: string;
        };
        value: AppTicketDetail;
    };
    'tickets.attachment': {
        args: {
            commentId: string;
            id: string;
            len?: number;
            mode?: 'meta' | 'preview' | 'thumb' | 'range' | 'download';
            n?: number;
            offset?: number;
            px?: number;
        };
        value: AppThreadFileData;
    };
    'tickets.create': {
        args: {
            agentName?: string;
            assignees?: string[];
            attachments?: AppThreadAttach[];
            body?: string;
            briefHtml?: string;
            kind?: string;
            options?: string[];
            priority?: number;
            project: string;
            title: string;
        };
        value: AppTicketCreated;
    };
    'tickets.comment': {
        args: {
            attachments?: AppThreadAttach[];
            body?: string;
            id: string;
            optionPick?: boolean;
        };
        value: AppTicketCommented;
    };
    'tickets.answer': {
        args: {
            answer: string;
            attachments?: AppThreadAttach[];
            id: string;
        };
        value: AppTicketAnswered;
    };
    'tickets.resolve': {
        args: {
            id: string;
            status?: string;
        };
        value: AppTicketStatus;
    };
    'tickets.assign': {
        args: {
            id: string;
            usernames: string[];
        };
        value: AppTicketAssigned;
    };
    'wiki.index': {
        args: {
            project: string;
        };
        value: AppWikiIndex;
    };
    'wiki.note': {
        args: {
            id: string;
            project: string;
        };
        value: AppWikiNote;
    };
    'storeDocs.collections': {
        args: {
            workspace: string;
        };
        value: AppStoreCollections;
    };
    'storeDocs.get': {
        args: {
            id: string;
            name: string;
            workspace: string;
        };
        value: AppStoreDoc;
    };
    'storeDocs.query': {
        args: {
            limit?: number;
            name: string;
            workspace: string;
        };
        value: AppStoreDocs;
    };
    'db.tables': {
        args: {
            workspace: string;
        };
        value: AppDbTables;
    };
    'db.rows': {
        args: {
            id?: string;
            limit?: number;
            table: string;
            workspace: string;
        };
        value: AppDbRows;
    };
    'db.insert': {
        args: {
            row: Record<string, unknown>;
            table: string;
            workspace: string;
        };
        value: AppDbRow;
    };
    'db.update': {
        args: {
            id: string;
            row: Record<string, unknown>;
            table: string;
            workspace: string;
        };
        value: AppDbRow;
    };
    'db.delete': {
        args: {
            id: string;
            table: string;
            workspace: string;
        };
        value: AppDbRow;
    };
    'db.query': {
        args: {
            params?: unknown[];
            sql: string;
            workspace: string;
        };
        value: AppDbQuery;
    };
    'heartbeats.list': {
        args: {
            workspace: string;
        };
        value: AppHeartbeat[];
    };
    'heartbeats.show': {
        args: {
            name: string;
            workspace: string;
        };
        value: AppHeartbeatDetail;
    };
    'heartbeats.status': {
        args: {
            limit?: number;
            name: string;
            workspace: string;
        };
        value: AppHeartbeatFire[];
    };
    'heartbeats.fires': {
        args: {
            limit?: number;
            workspace: string;
        };
        value: AppHeartbeatFire[];
    };
    'heartbeats.add': {
        args: {
            frequency: string;
            instructions: string;
            name: string;
            spec?: Record<string, unknown>;
            workspace: string;
        };
        value: AppHeartbeat;
    };
    'heartbeats.edit': {
        args: {
            frequency?: string;
            instructions?: string;
            name: string;
            spec?: Record<string, unknown>;
            workspace: string;
        };
        value: AppHeartbeat;
    };
    'heartbeats.enable': {
        args: {
            enabled: boolean;
            name: string;
            workspace: string;
        };
        value: AppHeartbeat;
    };
    'heartbeats.rename': {
        args: {
            from: string;
            to: string;
            workspace: string;
        };
        value: AppHeartbeat;
    };
    'heartbeats.archive': {
        args: {
            name: string;
            workspace: string;
        };
        value: AppHeartbeatArchived;
    };
    'heartbeats.fire': {
        args: {
            name: string;
            workspace: string;
        };
        value: AppHeartbeatFired;
    };
    'activity.snapshot': {
        args: {
            workspace: string;
        };
        value: AppActivity;
    };
    'activity.events': {
        args: {
            workspace: string;
        };
        value: AppActivityFrame;
    };
    'agentIdentity.get': {
        args: {
            workspace: string;
        };
        value: AppAgentIdentity;
    };
    'agentIdentity.set': {
        args: {
            color?: string;
            displayName?: string;
            image?: string | null;
            workspace: string;
        };
        value: AppAgentIdentityChanged;
    };
    'agentIdentity.setHandle': {
        args: {
            handle: string;
            workspace: string;
        };
        value: AppAgentHandleChanged;
    };
    'room.members': {
        args: {
            app?: string;
            workspace: string;
        };
        value: RoomMembers;
    };
    'app.self': {
        args: {
            app?: string;
        };
        value: AppSelf;
    };
    'app.usage': {
        args: {
            app?: string;
            hours?: number;
        };
        value: AppUsage;
    };
    'chat.pass': {
        args: Record<string, never>;
        value: AppChatPass;
    };
    'guests.chat': {
        args: {
            app?: string;
            username: string;
        };
        value: AppGuestChat;
    };
    'guests.setChat': {
        args: {
            app?: string;
            scope: 'member' | 'guest' | 'none';
            username: string;
        };
        value: AppGuestChat;
    };
    'chat.member': {
        args: {
            app?: string;
            principal?: string;
            username?: string;
        };
        value: AppChatMember;
    };
    'chat.whois': {
        args: {
            app?: string;
            memberIds: string[];
        };
        value: AppChatWhois;
    };
    'chat.createChannel': {
        args: {
            members?: string[];
            name: string;
            private?: boolean;
            slug: string;
            space?: string;
        };
        value: AppChatChannel;
    };
    'project.list': {
        args: Record<string, never>;
        value: AppProjectList;
    };
    'project.read': {
        args: {
            afterSeq?: number;
            beforeSeq?: number;
            group: string;
            limit?: number;
        };
        value: AppProjectPage;
    };
    'project.post': {
        args: {
            body: string;
            group: string;
        };
        value: AppProjectPosted;
    };
    'project.markRead': {
        args: {
            group: string;
            seq?: number;
        };
        value: AppProjectReadState;
    };
    'project.events': {
        args: {
            group: string;
        };
        value: AppProjectFrame;
    };
}
export type AppCallVerb = 'me.get' | 'roomAgents.list' | 'guests.login' | 'guests.rooms' | 'guests.setRooms' | 'app.rooms' | 'guests.list' | 'password.change' | 'guests.logout' | 'guests.refresh' | 'guests.devices' | 'guests.revokeDevice' | 'profile.get' | 'profile.update' | 'profile.setImage' | 'profile.clearImage' | 'profile.sessions' | 'profile.revokeSession' | 'profile.signOutOthers' | 'push.register' | 'push.unregister' | 'push.device' | 'push.prefs' | 'push.send' | 'push.status' | 'push.devices' | 'pairing.start' | 'pairing.status' | 'pairing.cancel' | 'pairing.check' | 'pairing.redeem' | 'email.send' | 'email.status' | 'email.log' | 'app.info' | 'password.forgot' | 'password.reset' | 'thread.page' | 'thread.latest' | 'thread.activity' | 'thread.activityRooms' | 'thread.send' | 'thread.answerCard' | 'thread.voidCard' | 'thread.attachmentGet' | 'thread.catchup' | 'thread.setRead' | 'thread.markAllRead' | 'thread.unread' | 'thread.mute' | 'files.resolveLinks' | 'thread.ensurePinned' | 'files.list' | 'files.read' | 'files.readBinary' | 'files.readRange' | 'files.stat' | 'files.downloads' | 'files.download' | 'files.write' | 'files.uploadBinary' | 'files.uploadChunk' | 'files.create' | 'files.copy' | 'files.move' | 'files.rename' | 'resources.list' | 'resources.add' | 'resources.remove' | 'tickets.list' | 'tickets.show' | 'tickets.attachment' | 'tickets.create' | 'tickets.comment' | 'tickets.answer' | 'tickets.resolve' | 'tickets.assign' | 'wiki.index' | 'wiki.note' | 'storeDocs.collections' | 'storeDocs.get' | 'storeDocs.query' | 'db.tables' | 'db.rows' | 'db.insert' | 'db.update' | 'db.delete' | 'db.query' | 'heartbeats.list' | 'heartbeats.show' | 'heartbeats.status' | 'heartbeats.fires' | 'heartbeats.add' | 'heartbeats.edit' | 'heartbeats.enable' | 'heartbeats.rename' | 'heartbeats.archive' | 'heartbeats.fire' | 'activity.snapshot' | 'agentIdentity.get' | 'agentIdentity.set' | 'agentIdentity.setHandle' | 'room.members' | 'app.self' | 'app.usage' | 'chat.pass' | 'guests.chat' | 'guests.setChat' | 'chat.member' | 'chat.whois' | 'chat.createChannel' | 'project.list' | 'project.read' | 'project.post' | 'project.markRead';
export type AppSubscribeVerb = 'thread.events' | 'thread.stream' | 'files.events' | 'activity.events' | 'project.events';
/** The named helpers: one per App verb a server calls (browser-only rows have none), grouped by surface. */
export interface AppSurfaces {
    readonly me: {
        /** Who this pass is: guest or App, its agents (identity, caps, what they allow, folders, unread and waiting tickets where the caps allow), this server's ids and features. no cap · GET /cli/skin/me. e.g. `const me = await k2.asGuest(pass).me.get()` */
        get(args?: Record<string, never>): Promise<AppMe>;
    };
    readonly roomAgents: {
        /** The agents in the pass's rooms: handle, name, color and picture, plus unread and waiting (thread:read) and waiting tickets (tickets:read) where the caps allow. no cap · GET /cli/skin/agents. e.g. `const {agents} = await k2.roomAgents.list()` */
        list(args?: Record<string, never>): Promise<AppRoomAgents>;
    };
    readonly guests: {
        /** Log a guest in through this App; the answer carries the guest's session pass. no cap · POST /cli/skin/login. e.g. `const {pass} = await k2.login({username, password})` */
        login(args: {
            app?: string;
            device?: AppDevice;
            password: string;
            username: string;
        }): Promise<AppGuestLogin>;
        /** One of this App's guests: its rooms on this App and its role (App: its own guests; Garden: any local App's). cap guests:rooms · GET /cli/skin/guests/rooms. e.g. `const g = await k2.asApp().guests.rooms({username: 'dana'})` */
        rooms(args: {
            app?: string;
            username: string;
        }): Promise<AppGuestRooms>;
        /** Set one guest's rooms on this App, within the rooms the App may hand out and never above the guest's role; idempotent, audited. cap guests:rooms · POST /cli/skin/guests/rooms. e.g. `await k2.asApp().guests.setRooms({username: 'dana', rooms: ['acme']})` */
        setRooms(args: {
            app?: string;
            rooms: string[];
            username: string;
        }): Promise<AppGuestRooms>;
        /** This App's own guests with their rooms on this App, role, on/off and whether each is an agent login, a page at a time (never the owner, a Connect login or another App's guest). cap guests:rooms · GET /cli/skin/guests/list. e.g. `const {guests, nextCursor} = await k2.asApp().guests.list({limit: 200})` */
        list(args?: {
            app?: string;
            cursor?: string;
            limit?: number;
        }): Promise<AppGuestList>;
        /** Sign this guest's pass out; a device sign-in ends with it (its refresh token stops working). no cap · POST /cli/skin/logout. e.g. `await k2.asGuest(pass).guests.logout()` */
        logout(args?: {}): Promise<AppOk>;
        /** Trade a device's refresh token for a new pass and a new refresh token (one-time, rotating; a reused token ends the device's sign-in). no cap · POST /cli/skin/refresh. e.g. `const s = await k2.guests.refresh({refresh})` */
        refresh(args: {
            app?: string;
            refresh: string;
        }): Promise<AppDeviceSession>;
        /** This guest's devices that stay signed in to this App. no cap · GET /cli/skin/devices. e.g. `const {devices} = await k2.asGuest(pass).guests.devices()` */
        devices(args?: {}): Promise<AppDevices>;
        /** Sign one of this guest's own devices out (its pass and refresh token end). no cap · POST /cli/skin/devices/revoke. e.g. `await k2.asGuest(pass).guests.revokeDevice({id})` */
        revokeDevice(args: {
            id: string;
        }): Promise<AppOk>;
        /** One of this App's guests: their chat (member, guest or none), who gave it, and this App's member ceiling. cap guests:chat · GET /cli/skin/guests/chat. e.g. `const c = await k2.asApp().guests.chat({username: 'dana'})` */
        chat(args: {
            app?: string;
            username: string;
        }): Promise<AppGuestChat>;
        /** Give one of this App's guests chat (member or guest) or take it away; only grants this App made, within its ceiling; idempotent, audited. cap guests:chat · POST /cli/skin/guests/chat. e.g. `await k2.asApp().guests.setChat({username: 'dana', scope: 'member'})` */
        setChat(args: {
            app?: string;
            scope: 'member' | 'guest' | 'none';
            username: string;
        }): Promise<AppGuestChat>;
    };
    readonly app: {
        /** This App's own room, the owner's guests-rooms grant and the rooms it may hand its guests (the App's own pass; no grant needed to read it). no cap · GET /cli/skin/app-rooms. e.g. `const {ceiling, granted} = await k2.asApp().app.rooms()` */
        rooms(args?: {
            app?: string;
        }): Promise<AppRooms>;
        /** Public: this one App's name and how to sign in, only when the owner listed it; anything else answers like a route that doesn't exist. no cap · GET /cli/skin/app-info. e.g. `const info = await fetch(server + '/cli/skin/app-info?app=' + id)` */
        info(args: {
            app: string;
        }): Promise<AppInfo>;
        /** This App: its id, live Seed hash, health and when it was deployed (its own App only; the App pass, never a guest). no cap · GET /cli/skin/self. e.g. `const me = await k2.asApp().app.self()` */
        self(args?: {
            app?: string;
        }): Promise<AppSelf>;
        /** This App's quotas, live use and per-call counts from its own audit (its own App only; the App pass, never a guest). no cap · GET /cli/skin/usage. e.g. `const {verbs, quotas} = await k2.asApp().app.usage({hours: 24})` */
        usage(args?: {
            app?: string;
            hours?: number;
        }): Promise<AppUsage>;
    };
    readonly password: {
        /** A guest changes their own password (5 wrong current passwords per 15 minutes, then rate_limited); stay: true keeps this pass signed in. no cap · POST /cli/skin/password/change. e.g. `await k2.asGuest(pass).password.change({oldPassword, password, stay: true})` */
        change(args: {
            oldPassword: string;
            password: string;
            stay?: boolean;
        }): Promise<AppPasswordChanged>;
        /** Start a guest's password reset (a platform pass; the guest gets the reset link by email). no cap · POST /cli/skin/password/forgot. e.g. `await k2.asApp().password.forgot({username})` */
        forgot(args: {
            username: string;
        }): Promise<AppOk>;
        /** Public: finish a password reset with the emailed token (ends every session of that guest). no cap · POST /cli/skin/password/reset. e.g. `await fetch('/cli/skin/password/reset', {method: 'POST', body})` */
        reset(args: {
            password: string;
            resetToken: string;
        }): Promise<AppOk>;
    };
    readonly profile: {
        /** This person's own profile in this App: names, email, picture hash, role and rooms here, sessions count, and what they may change. no cap · GET /cli/skin/profile. e.g. `const me = await k2.asGuest(pass).profile.get({image: '1'})` */
        get(args?: {
            ifImageHash?: string;
            image?: string;
        }): Promise<AppProfile>;
        /** This person changes their own full name and display name, where the App allows it (20 changes per 10 minutes). no cap · POST /cli/skin/profile. e.g. `await k2.asGuest(pass).profile.update({fullName: 'Dana Ruiz'})` */
        update(args: {
            displayName?: string | null;
            fullName?: string | null;
        }): Promise<AppProfileUpdated>;
        /** This person sets their own picture, where the App allows it. no cap · POST /cli/skin/profile/image. e.g. `await k2.asGuest(pass).profile.setImage({image: dataUrl})` */
        setImage(args: {
            image: string;
        }): Promise<AppProfileImage>;
        /** This person removes their own picture, where the App allows it. no cap · POST /cli/skin/profile/image/clear. e.g. `await k2.asGuest(pass).profile.clearImage()` */
        clearImage(args?: {}): Promise<AppOk>;
        /** The browsers and devices signed in to this App as this person, this one first (never another App's). no cap · GET /cli/skin/sessions. e.g. `const {sessions} = await k2.asGuest(pass).profile.sessions()` */
        sessions(args?: {}): Promise<AppSessions>;
        /** Sign one of this person's own browsers or devices out of this App. no cap · POST /cli/skin/sessions/revoke. e.g. `await k2.asGuest(pass).profile.revokeSession({id})` */
        revokeSession(args: {
            id: string;
        }): Promise<AppOk>;
        /** Sign this person out of this App everywhere else (their notification devices there go too); this pass stays. no cap · POST /cli/skin/sessions/revoke-others. e.g. `await k2.asGuest(pass).profile.signOutOthers()` */
        signOutOthers(args?: {}): Promise<AppSignedOut>;
    };
    readonly push: {
        /** Register this guest's phone for this App's notifications; call it on every launch and whenever the token changes. no cap · POST /cli/skin/push/register. e.g. `await k2.asGuest(pass).push.register({deviceId, platform: 'apns', pushToken, environment: 'sandbox'})` */
        register(args: {
            channel?: 'app' | 'k2-companion';
            deviceId: string;
            environment?: 'production' | 'sandbox';
            mutedTopics?: string[];
            platform: 'apns' | 'fcm';
            previews?: boolean | null;
            pushToken: string;
        }): Promise<AppPushDevice>;
        /** Stop this App's notifications on one of this guest's own devices (signing out also removes it). no cap · POST /cli/skin/push/unregister. e.g. `await k2.asGuest(pass).push.unregister({deviceId})` */
        unregister(args: {
            deviceId: string;
        }): Promise<AppPushRemoved>;
        /** This guest's own push registration for one device (never the token). no cap · GET /cli/skin/push/device. e.g. `const d = await k2.asGuest(pass).push.device({deviceId})` */
        device(args: {
            deviceId: string;
        }): Promise<AppPushDevice>;
        /** Change one device's notification choices (previews, muted topics) without its token. no cap · POST /cli/skin/push/prefs. e.g. `await k2.asGuest(pass).push.prefs({deviceId, mutedTopics: ['marketing']})` */
        prefs(args: {
            deviceId: string;
            mutedTopics?: string[];
            previews?: boolean | null;
        }): Promise<AppPushDevice>;
        /** Notify this App's own guests' phones (a guest, a list, or a room's guests); answers at once with a sendId. cap push:send · POST /cli/skin/push/send. e.g. `const {sendId} = await k2.push.send({to: {guest: principalId}, title, body, data: {jobId}, topic: 'jobs'})` */
        send(args: {
            androidChannel?: string;
            badge?: number;
            body?: string;
            collapseId?: string;
            data?: Record<string, string>;
            mutable?: boolean;
            priority?: 'high' | 'normal';
            silent?: boolean;
            sound?: string;
            threadId?: string;
            title?: string;
            to: AppPushTo;
            topic?: string;
            ttlSecs?: number;
        }): Promise<AppPushQueued>;
        /** One send's counts (kept for an hour): sent, dead, failed; never who. cap push:send · GET /cli/skin/push/status. e.g. `const s = await k2.push.status({sendId})` */
        status(args: {
            sendId: string;
        }): Promise<AppPushStatus>;
        /** Which of this App's guests have phones registered, and on which platforms (never a token). cap push:send · GET /cli/skin/push/devices. e.g. `const {guests} = await k2.push.devices()` */
        devices(args?: {}): Promise<AppPushDevices>;
    };
    readonly pairing: {
        /** Start pairing this guest's phone: a one-time code and its QR, from their own web sign-in (at most 15 minutes old, or with their password). no cap · POST /cli/skin/pair/start. e.g. `const p = await k2.asGuest(pass).pairing.start()` */
        start(args?: {
            host?: string;
            password?: string;
        }): Promise<AppPairingStart>;
        /** Is this guest's pairing done: pending, paired (with the device's name), expired or cancelled. no cap · GET /cli/skin/pair/status. e.g. `const s = await k2.asGuest(pass).pairing.status({pairingId})` */
        status(args: {
            pairingId: string;
        }): Promise<AppPairingStatus>;
        /** Drop one of this guest's pairing codes (the QR stops working). no cap · POST /cli/skin/pair/cancel. e.g. `await k2.asGuest(pass).pairing.cancel({pairingId})` */
        cancel(args: {
            pairingId: string;
        }): Promise<AppPairingCancelled>;
        /** Public: who a pairing code would sign in, without using it (ask 'Sign in as ...?' first). no cap · POST /cli/skin/pair/check. e.g. `const who = await k2.pairing.check({code})` */
        check(args: {
            app?: string;
            code: string;
        }): Promise<AppPairingCheck>;
        /** Public: trade a pairing code for this App's device sign-in (a pass and a refresh token), once; optionally register push in the same call. no cap · POST /cli/skin/pair. e.g. `const {pass, refresh} = await k2.pairing.redeem({code, device: {id, name}})` */
        redeem(args: {
            app?: string;
            code: string;
            device: AppDevice;
            push?: AppPairingPush;
        }): Promise<AppPairedSession>;
    };
    readonly email: {
        /** Email from this App's own sender (granted by the owner) to its own users, or anyone with the owner's any scope. Answers 202 with an id. cap email:send · POST /cli/skin/email/send. e.g. `const {id} = await k2.email.send({to: [user.email], subject: 'Your order shipped', html, text, tags: ['orders'], idempotencyKey: orderId})` */
        send(args: {
            bcc?: string[];
            cc?: string[];
            from?: string;
            fromName?: string;
            headers?: Record<string, string>;
            html?: string;
            idempotencyKey?: string;
            replyTo?: string[];
            sendAt?: number;
            subject: string;
            tags?: string[];
            text?: string;
            to: string[];
        }): Promise<AppEmailAccepted>;
        /** One email's state (kept 30 days): submitted means accepted for delivery; never the body. cap email:send · GET /cli/skin/email/status. e.g. `const s = await k2.email.status({id})` */
        status(args: {
            id: string;
        }): Promise<AppEmailStatus>;
        /** This App's sent email, newest first (metadata only, 30 days): who, subject, tags, status. cap email:send · GET /cli/skin/email/log. e.g. `const {items} = await k2.email.log({tag: 'orders', limit: 50})` */
        log(args?: {
            before?: number;
            limit?: number;
            status?: string;
            tag?: string;
        }): Promise<AppEmailLog>;
    };
    readonly thread: {
        /** A page of a room's Thread, oldest first; sinceSeq catches up, beforeSeq pages back, afterSeq reads a delta (new, changed, removed). cap thread:read · GET /cli/thread. e.g. `const page = await k2.thread.page({addr: 'sales', limit: 50})` */
        page(args: {
            addr: string;
            afterSeq?: number;
            beforeSeq?: number;
            limit?: number;
            sinceSeq?: number;
        }): Promise<AppThreadPage>;
        /** The newest Thread item per room, for list previews; a room the pass can't read is an item with error. cap thread:read · GET /cli/thread/latest. e.g. `const {items} = await k2.thread.latest({addrs: ['sales', 'ops']})` */
        latest(args: {
            addrs: string[];
        }): Promise<AppThreadLatest>;
        /** The room's live pinned-Chat Thread turn, counts only (needs thread:read and activity:read); null when none. Live: kind thread_activity on thread.stream (the SDK opens it with ear=1); a raw overlay socket opened with ear=1 gets collection activity frames. caps activity:read + thread:read · GET /cli/thread/activity. e.g. `const {turn} = await k2.thread.activity({addr})` */
        activity(args: {
            addr: string;
        }): Promise<AppThreadActivity>;
        /** Every room's live pinned-Chat Thread turn in one read (one rate-limit token for the set): the catch-up for thread.stream with many rooms, on every socket open. thread.watchActivity({rooms}) calls it for you. caps activity:read + thread:read · GET /cli/thread/activity/rooms. e.g. `const {turns} = await k2.thread.activityRooms({rooms: 'all'})` */
        activityRooms(args: {
            rooms: string;
        }): Promise<AppThreadActivityRooms>;
        /** Post text (and room files) to a room's Thread as the guest (or the App); the agent gets it at once. cap thread:post · POST /cli/thread/post. e.g. `await k2.thread.send({addr: 'sales', text: 'Hi'})` */
        send(args: {
            addr: string;
            attachments?: AppThreadAttach[];
            text?: string;
            via?: string;
        }): Promise<AppThreadPosted>;
        /** Answer a choice card (or set a secret card) in a room's Thread, as a person: the guest's own pass (an App's own pass answers card_person_only). cap thread:post · POST /cli/thread/answer. e.g. `await k2.thread.answerCard({addr, cardId, answer: 'Yes'})` */
        answerCard(args: {
            addr: string;
            answer?: string;
            cardId: string;
            confirm?: boolean;
            delivery?: string;
            hosts?: string[];
            scope?: string;
            secret?: string;
        }): Promise<AppCardAnswered>;
        /** Dismiss a pending card in a room's Thread. cap thread:post · POST /cli/thread/void. e.g. `await k2.thread.voidCard({addr, cardId})` */
        voidCard(args: {
            addr: string;
            cardId: string;
        }): Promise<AppCardVoided>;
        /** Live Thread items of one room (new posts and card changes; merge by id); after a gap, thread.page with sinceSeq. The SDK never asks for the counts-only ear here (it keeps every frame a Thread item); a raw socket opened with ear=1 also gets collection activity frames: use thread.stream or thread.activity for the ear. cap thread:read · WS /cli/overlay/events?conversation=. e.g. `const off = k2.thread.events({conversation}, (item) => draw(item))` */
        events(args: {
            conversation: string;
        }, onFrame: (frame: AppThreadItem) => void, onError?: (error: K2Error) => void): () => void;
        /** Live Thread items of every room the pass may read on one socket (inbox_subscribed names the set; grants and revokes apply live); after a gap, thread.page with sinceSeq. cap thread:read · WS /cli/thread/stream?rooms=. e.g. `const off = k2.thread.stream({rooms: 'all'}, (f) => f.kind === 'thread_message' && draw(f.room, f))` */
        stream(args: {
            rooms: string;
        }, onFrame: (frame: AppStreamFrame) => void, onError?: (error: K2Error) => void): () => void;
        /** One attachment of a room's message: meta, preview, thumbnail or a byte range, only when your own file rights cover it; a download (a save) also needs files:download. cap thread:read · GET /cli/thread/attachment. e.g. `const t = await k2.thread.attachmentGet({addr: 'sales', id: item.id, n: 0, mode: 'thumb'})` */
        attachmentGet(args: {
            addr: string;
            id: string;
            len?: number;
            mode?: 'meta' | 'preview' | 'thumb' | 'range' | 'download';
            n?: number;
            offset?: number;
            px?: number;
        }): Promise<AppThreadFileData>;
        /** Deltas (or newest pages) for many rooms in one round trip, each exactly what thread.page answers for it. cap thread:read · POST /cli/thread/catchup. e.g. `const {threads} = await k2.thread.catchup({threads: [{addr: 'sales', afterSeq: 41}]})` */
        catchup(args: {
            limit?: number;
            threads: AppCatchupAsk[];
        }): Promise<AppCatchup>;
        /** Mark the signed-in person's own read cursor in a room's Thread (an App's own pass: its bookmark); forward only, on every surface they use. cap thread:read · POST /cli/thread/read. e.g. `await k2.thread.setRead({addr: 'sales', seq: last.seq})` */
        setRead(args: {
            addr: string;
            seq?: number;
        }): Promise<AppReadState>;
        /** Mark everything in a room's Thread read for the signed-in person (an App's own pass: its bookmark), so the agent's unread goes to 0; waiting cards stay waiting. Their own state only, on every surface they use. cap thread:read · POST /cli/thread/read-all. e.g. `await k2.thread.markAllRead({addr: 'sales'})` */
        markAllRead(args: {
            addr: string;
        }): Promise<AppReadState>;
        /** The signed-in person's own unread counts: a guest's over the room Threads it may read, a Garden's over the person's Threads. cap thread:read · GET /cli/thread/unread. e.g. `const {total, byConversation} = await k2.thread.unread()` */
        unread(args?: Record<string, never>): Promise<AppUnread>;
        /** Mute or unmute a room's Thread for the signed-in person (their own state, on every surface); an App's own pass has no mutes. cap thread:read · POST /cli/thread/mute. e.g. `await k2.thread.mute({addr: 'sales', muted: true})` */
        mute(args: {
            addr: string;
            muted: boolean;
            until?: number;
        }): Promise<AppReadState>;
        /** Check the room's pinned Chat is live (find-only for Apps: it never starts one). cap thread:read · POST /cli/workspace/ensure-pinned-chat. e.g. `await k2.thread.ensurePinned({workspace: 'sales'})` */
        ensurePinned(args: {
            workspace: string;
        }): Promise<AppPinnedChat>;
    };
    readonly files: {
        /** Which paths printed in a room's messages lead to a file or folder your file rights cover (the rest stay plain text). cap files:read · POST /cli/fs/resolve-links. e.g. `const {links} = await k2.files.resolveLinks({workspace, paths: ['jobs/J1/plans.pdf']})` */
        resolveLinks(args: {
            paths: string[];
            workspace: string;
        }): Promise<AppLinks>;
        /** A folder's entries, folders first. cap files:read · GET /cli/fs/read-dir. e.g. `const entries = await k2.files.list({workspace, relPath: '.'})` */
        list(args: {
            relPath: string;
            showHidden?: boolean;
            workspace: string;
        }): Promise<AppFileEntry[]>;
        /** A UTF-8 text file (at most 10 MiB). cap files:read · GET /cli/fs/read-file. e.g. `const {content} = await k2.files.read({workspace, relPath: 'notes.md'})` */
        read(args: {
            relPath: string;
            workspace: string;
        }): Promise<AppFileText>;
        /** A file as base64 (at most 50 MiB; read-range for more). cap files:read · GET /cli/fs/read-binary. e.g. `const {base64} = await k2.files.readBinary({workspace, relPath})` */
        readBinary(args: {
            relPath: string;
            workspace: string;
        }): Promise<AppFileBase64>;
        /** A byte range of a file, as base64. cap files:read · GET /cli/fs/read-range. e.g. `const r = await k2.files.readRange({workspace, relPath, offset: 0})` */
        readRange(args: {
            len?: number;
            offset?: number;
            relPath: string;
            workspace: string;
        }): Promise<AppFileRange>;
        /** One file's or folder's kind, size, modified time and content type, under the room's folder scope. cap files:read · GET /cli/fs/stat. e.g. `const {kind, size, mime} = await k2.files.stat({workspace, relPath: 'jobs/J1/plans.pdf'})` */
        stat(args: {
            relPath: string;
            workspace: string;
        }): Promise<AppFileStat>;
        /** The files this login may save from the room: its role's download entries, expanded (needs files:download; App only, a Garden is the owner and reads every file). cap files:download · GET /cli/fs/downloads. e.g. `const {items} = await k2.asGuest(pass).files.downloads({workspace})` */
        downloads(args: {
            workspace: string;
        }): Promise<AppDownloads>;
        /** Save a file, a chunk at a time: needs files:download, and the file inside a download entry or readable with files:read (App only, like files.downloads). cap files:download · GET /cli/fs/download. e.g. `const c = await k2.asGuest(pass).files.download({workspace, relPath: 'reports/2026-10.pdf'})` */
        download(args: {
            len?: number;
            offset?: number;
            relPath: string;
            workspace: string;
        }): Promise<AppFileDownload>;
        /** Write (or overwrite) a text file; its folder must exist. cap files:write · POST /cli/fs/write-file. e.g. `await k2.files.write({workspace, relPath: 'a.txt', content})` */
        write(args: {
            content: string;
            relPath: string;
            workspace: string;
        }): Promise<AppFileDone>;
        /** Upload a file in one request; a taken name becomes name (1).ext. files:write, or files:upload into a drop folder (it lands in your own subfolder). cap files:write · POST /cli/fs/upload-binary. e.g. `const {relPath} = await k2.files.uploadBinary({workspace, dir: 'in', filename, base64})` */
        uploadBinary(args: {
            base64: string;
            dir: string;
            filename: string;
            workspace: string;
        }): Promise<AppFileUploaded>;
        /** Resumable upload, one chunk per request (10 GiB in all); the last chunk lands the file. files:write, or files:upload into a drop folder. cap files:write · POST /cli/fs/upload-chunk. e.g. `await k2.files.uploadChunk({workspace, dir, filename, uploadId, offset, base64, isLast})` */
        uploadChunk(args: {
            base64?: string;
            dir: string;
            filename: string;
            isLast?: boolean;
            offset?: number;
            status?: boolean;
            totalBytes?: number;
            uploadId: string;
            workspace: string;
        }): Promise<AppFileChunk>;
        /** Create an empty file or a folder. cap files:write · POST /cli/fs/create. e.g. `await k2.files.create({workspace, relPath: 'in', isDirectory: true})` */
        create(args: {
            isDirectory?: boolean;
            relPath: string;
            workspace: string;
        }): Promise<AppFileDone>;
        /** Copy files or folders (a taken name becomes 'name copy'). cap files:write · POST /cli/fs/copy. e.g. `await k2.files.copy({workspace, sources: ['a.txt'], destination: 'out'})` */
        copy(args: {
            destination: string;
            sources: string[];
            workspace: string;
        }): Promise<AppFileDone>;
        /** Move files or folders. cap files:write · POST /cli/fs/move. e.g. `await k2.files.move({workspace, sources: ['a.txt'], destination: 'out'})` */
        move(args: {
            destination: string;
            sources: string[];
            workspace: string;
        }): Promise<AppFileDone>;
        /** Rename a file or folder in place. cap files:write · POST /cli/fs/rename. e.g. `const {relPath} = await k2.files.rename({workspace, relPath: 'a.txt', newName: 'b.txt'})` */
        rename(args: {
            newName: string;
            relPath: string;
            workspace: string;
        }): Promise<AppFileLanded>;
        /** Live file changes in one room (paths only; refetch what you show). cap files:read · WS /cli/fs/events?workspace=. e.g. `const off = k2.files.events({workspace}, (f) => refresh(f.paths))` */
        events(args: {
            workspace: string;
        }, onFrame: (frame: AppFsFrame) => void, onError?: (error: K2Error) => void): () => void;
    };
    readonly resources: {
        /** The room's pinned resources (its links drawer). cap files:read · GET /cli/workspace/resources. e.g. `const {docs} = await k2.resources.list({workspace})` */
        list(args: {
            workspace: string;
        }): Promise<AppResources>;
        /** Pin an existing file to the room's resources. cap files:write · POST /cli/workspace/resources/add. e.g. `await k2.resources.add({workspace, relPath: 'plan.pdf'})` */
        add(args: {
            relPath: string;
            workspace: string;
        }): Promise<AppResourceAdded>;
        /** Unpin a file from the room's resources. cap files:write · POST /cli/workspace/resources/remove. e.g. `await k2.resources.remove({workspace, relPath: 'plan.pdf'})` */
        remove(args: {
            relPath: string;
            workspace: string;
        }): Promise<AppResourceRemoved>;
    };
    readonly tickets: {
        /** A room's tickets, newest first. cap tickets:read · GET /cli/feedback/list. e.g. `const {items} = await k2.tickets.list({project: 'sales'})` */
        list(args: {
            all?: boolean;
            project: string;
            status?: string;
        }): Promise<AppTickets>;
        /** One ticket with its comments (and brief). cap tickets:read · GET /cli/feedback/show. e.g. `const t = await k2.tickets.show({id})` */
        show(args: {
            brief?: boolean;
            id: string;
        }): Promise<AppTicketDetail>;
        /** One file of a ticket message: meta, preview, thumbnail or a byte range, only when your own file rights cover it; mode=download also needs files:download in the room; a file your files:read scope doesn't cover is hidden. cap tickets:read · GET /cli/feedback/attachment. e.g. `const t = await k2.tickets.attachment({id, commentId: c.id, n: 0, mode: 'thumb'})` */
        attachment(args: {
            commentId: string;
            id: string;
            len?: number;
            mode?: 'meta' | 'preview' | 'thumb' | 'range' | 'download';
            n?: number;
            offset?: number;
            px?: number;
        }): Promise<AppThreadFileData>;
        /** Open a ticket in a room for its agent. cap tickets:post · POST /cli/feedback/create. e.g. `await k2.tickets.create({project: 'sales', title: 'Quote for J1042?'})` */
        create(args: {
            agentName?: string;
            assignees?: string[];
            attachments?: AppThreadAttach[];
            body?: string;
            briefHtml?: string;
            kind?: string;
            options?: string[];
            priority?: number;
            project: string;
            title: string;
        }): Promise<AppTicketCreated>;
        /** Comment on a ticket as the guest (free text moves it to needs_discussion). cap tickets:post · POST /cli/feedback/comment. e.g. `await k2.tickets.comment({id, body: 'Looks right'})` */
        comment(args: {
            attachments?: AppThreadAttach[];
            body?: string;
            id: string;
            optionPick?: boolean;
        }): Promise<AppTicketCommented>;
        /** Answer a ticket as the guest. cap tickets:post · POST /cli/feedback/answer. e.g. `await k2.tickets.answer({id, answer: 'Approved'})` */
        answer(args: {
            answer: string;
            attachments?: AppThreadAttach[];
            id: string;
        }): Promise<AppTicketAnswered>;
        /** Set a ticket's status (Apps can't set answered). cap tickets:post · POST /cli/feedback/resolve. e.g. `await k2.tickets.resolve({id})` */
        resolve(args: {
            id: string;
            status?: string;
        }): Promise<AppTicketStatus>;
        /** Set a ticket's assignees. cap tickets:post · POST /cli/feedback/assign. e.g. `await k2.tickets.assign({id, usernames: ['dana']})` */
        assign(args: {
            id: string;
            usernames: string[];
        }): Promise<AppTicketAssigned>;
    };
    readonly wiki: {
        /** A room's wiki: its notes and the links between them. cap wiki:read · GET /cli/wiki/index. e.g. `const {nodes} = await k2.wiki.index({project: 'sales'})` */
        index(args: {
            project: string;
        }): Promise<AppWikiIndex>;
        /** One wiki note's markdown. cap wiki:read · GET /cli/wiki/note. e.g. `const {body} = await k2.wiki.note({project, id: 'Home.md'})` */
        note(args: {
            id: string;
            project: string;
        }): Promise<AppWikiNote>;
    };
    readonly storeDocs: {
        /** The room's store collections (Linux, with the workspace database). cap store:read · GET /cli/store/list. e.g. `const {collections} = await k2.storeDocs.collections({workspace})` */
        collections(args: {
            workspace: string;
        }): Promise<AppStoreCollections>;
        /** One store document. cap store:read · GET /cli/store/get. e.g. `const {doc} = await k2.storeDocs.get({workspace, name: 'jobs', id: 'J1042'})` */
        get(args: {
            id: string;
            name: string;
            workspace: string;
        }): Promise<AppStoreDoc>;
        /** Documents of one store collection. cap store:read · GET /cli/store/query. e.g. `const {docs} = await k2.storeDocs.query({workspace, name: 'jobs'})` */
        query(args: {
            limit?: number;
            name: string;
            workspace: string;
        }): Promise<AppStoreDocs>;
    };
    readonly db: {
        /** The room database's tables (Linux, with the workspace database). cap store:read · GET /cli/db/tables. e.g. `const {tables} = await k2.db.tables({workspace})` */
        tables(args: {
            workspace: string;
        }): Promise<AppDbTables>;
        /** Rows of a table as the guest (RLS decides which); with id, one row. cap store:read · GET /cli/db/rows. e.g. `const {rows} = await k2.db.rows({workspace, table: 'jobs'})` */
        rows(args: {
            id?: string;
            limit?: number;
            table: string;
            workspace: string;
        }): Promise<AppDbRows>;
        /** Insert one row as the guest (RLS checks it). cap store:write · POST /cli/db/rows. e.g. `const {row} = await k2.db.insert({workspace, table: 'jobs', row: {name}})` */
        insert(args: {
            row: Record<string, unknown>;
            table: string;
            workspace: string;
        }): Promise<AppDbRow>;
        /** Update one row by id as the guest. cap store:write · POST /cli/db/rows/update. e.g. `await k2.db.update({workspace, table: 'jobs', id, row: {status: 'done'}})` */
        update(args: {
            id: string;
            row: Record<string, unknown>;
            table: string;
            workspace: string;
        }): Promise<AppDbRow>;
        /** Delete one row by id as the guest; answers the deleted row. cap store:write · POST /cli/db/rows/delete. e.g. `await k2.db.delete({workspace, table: 'jobs', id})` */
        delete(args: {
            id: string;
            table: string;
            workspace: string;
        }): Promise<AppDbRow>;
        /** One brokered SQL statement as the guest (store:read to read, store:write to write; 5000 rows, 10 s). cap store:read · POST /cli/db/query. e.g. `const {rows} = await k2.db.query({workspace, sql: 'select * from jobs where id = $1', params: [id]})` */
        query(args: {
            params?: unknown[];
            sql: string;
            workspace: string;
        }): Promise<AppDbQuery>;
    };
    readonly heartbeats: {
        /** The room's heartbeats: schedule, next fire, why one waits. cap heartbeats:read · GET /cli/heartbeat/list. e.g. `const rows = await k2.heartbeats.list({workspace})` */
        list(args: {
            workspace: string;
        }): Promise<AppHeartbeat[]>;
        /** One heartbeat, with its instructions when the pass has files:read. cap heartbeats:read · GET /cli/heartbeat/show. e.g. `const hb = await k2.heartbeats.show({workspace, name: 'morning'})` */
        show(args: {
            name: string;
            workspace: string;
        }): Promise<AppHeartbeatDetail>;
        /** One heartbeat's recent fires, newest first. cap heartbeats:read · GET /cli/heartbeat/status. e.g. `const fires = await k2.heartbeats.status({workspace, name})` */
        status(args: {
            limit?: number;
            name: string;
            workspace: string;
        }): Promise<AppHeartbeatFire[]>;
        /** The room's recent heartbeat fires, newest first. cap heartbeats:read · GET /cli/heartbeat/fires-list. e.g. `const fires = await k2.heartbeats.fires({workspace})` */
        fires(args: {
            limit?: number;
            workspace: string;
        }): Promise<AppHeartbeatFire[]>;
        /** Add a heartbeat (60 changes per guest per room per 10 minutes). cap heartbeats:write · POST /cli/heartbeat/add. e.g. `await k2.heartbeats.add({workspace, name: 'morning', frequency: 'daily', spec: {time: '08:00'}, instructions})` */
        add(args: {
            frequency: string;
            instructions: string;
            name: string;
            spec?: Record<string, unknown>;
            workspace: string;
        }): Promise<AppHeartbeat>;
        /** Change a heartbeat's schedule or instructions. cap heartbeats:write · POST /cli/heartbeat/edit. e.g. `await k2.heartbeats.edit({workspace, name, instructions})` */
        edit(args: {
            frequency?: string;
            instructions?: string;
            name: string;
            spec?: Record<string, unknown>;
            workspace: string;
        }): Promise<AppHeartbeat>;
        /** Turn a heartbeat on or off. cap heartbeats:write · POST /cli/heartbeat/enable. e.g. `await k2.heartbeats.enable({workspace, name, enabled: false})` */
        enable(args: {
            enabled: boolean;
            name: string;
            workspace: string;
        }): Promise<AppHeartbeat>;
        /** Rename a heartbeat. cap heartbeats:write · POST /cli/heartbeat/rename. e.g. `await k2.heartbeats.rename({workspace, from: 'a', to: 'b'})` */
        rename(args: {
            from: string;
            to: string;
            workspace: string;
        }): Promise<AppHeartbeat>;
        /** Archive a heartbeat. cap heartbeats:write · POST /cli/heartbeat/archive. e.g. `await k2.heartbeats.archive({workspace, name})` */
        archive(args: {
            name: string;
            workspace: string;
        }): Promise<AppHeartbeatArchived>;
        /** Fire a heartbeat now (once per 5 minutes per guest). cap heartbeats:write · POST /cli/heartbeat/fire. e.g. `await k2.heartbeats.fire({workspace, name})` */
        fire(args: {
            name: string;
            workspace: string;
        }): Promise<AppHeartbeatFired>;
    };
    readonly activity: {
        /** Whether the room's agent is working now, per session. cap activity:read · GET /cli/activity/snapshot. e.g. `const {status} = await k2.activity.snapshot({workspace})` */
        snapshot(args: {
            workspace: string;
        }): Promise<AppActivity>;
        /** Live room events: agent working, heartbeats, tickets, rooms_changed, room_members_changed, app_deployed; resync then close 4008 when behind. cap activity:read · WS /cli/activity/events?workspace=. e.g. `const off = k2.activity.events({workspace}, (f) => f.kind === 'resync' ? reload() : apply(f))` */
        events(args: {
            workspace: string;
        }, onFrame: (frame: AppActivityFrame) => void, onError?: (error: K2Error) => void): () => void;
    };
    readonly agentIdentity: {
        /** The room's agent: name, color, picture, handle, and what the pass may change. no cap · GET /cli/agent/identity. e.g. `const id = await k2.agentIdentity.get({workspace})` */
        get(args: {
            workspace: string;
        }): Promise<AppAgentIdentity>;
        /** Change the room's agent's name, color or picture. cap agent:identity · POST /cli/agent/identity. e.g. `await k2.agentIdentity.set({workspace, color: '#3366ff'})` */
        set(args: {
            color?: string;
            displayName?: string;
            image?: string | null;
            workspace: string;
        }): Promise<AppAgentIdentityChanged>;
        /** Change the room's agent's handle (its address). cap agent:handle · POST /cli/agent/handle. e.g. `await k2.agentIdentity.setHandle({workspace, handle: 'sales-desk'})` */
        setHandle(args: {
            handle: string;
            workspace: string;
        }): Promise<AppAgentHandleChanged>;
    };
    readonly room: {
        /** Who holds a grant on the room: guests and agent logins with their roles (from grants, not presence); admins of this box are one line. cap room:members · GET /cli/skin/members. e.g. `const {members} = await k2.room.members({workspace})` */
        members(args: {
            app?: string;
            workspace: string;
        }): Promise<RoomMembers>;
    };
    readonly chat: {
        /** Mint a Sew chat pass for the guest of this request (a login with a chat grant; server only, never a handoff code). no cap · POST /cli/sew/pass. e.g. `const {pass} = await k2.withGuest(guestPass, () => k2.chat.pass())` */
        pass(args?: Record<string, never>): Promise<AppChatPass>;
        /** One of this App's users (by principal id or username) → their Sew member id; anyone else is the same not_found as nobody. cap chat:read · GET /cli/sew/app/member. e.g. `const {memberId} = await k2.asApp().chat.member({principal: user.principalId})` */
        member(args: {
            app?: string;
            principal?: string;
            username?: string;
        }): Promise<AppChatMember>;
        /** Sew member ids → which of this App's users each is; everyone else shows only memberId, kind and isThisAppUser: false. cap chat:read · POST /cli/sew/app/whois. e.g. `const {members} = await k2.asApp().chat.whois({memberIds: ['48831765147549712']})` */
        whois(args: {
            app?: string;
            memberIds: string[];
        }): Promise<AppChatWhois>;
        /** Make a text channel in this App's own Sew Space (or find it: the same slug answers created: false) and put listed App users in it, adding them to the Space first. Needs Sew 0.5.0+. cap chat:channels · POST /cli/sew/app/channels. e.g. `const ch = await k2.asApp().chat.createChannel({slug: 'job-1042', name: 'Job 1042', private: true, members: ['dana', 'eli']})` */
        createChannel(args: {
            members?: string[];
            name: string;
            private?: boolean;
            slug: string;
            space?: string;
        }): Promise<AppChatChannel>;
    };
    readonly project: {
        /** The project chats this pass can read: a project shows only when every member agent is a room it holds with thread:read. cap thread:read · GET /cli/skin/projects. e.g. `const {projects} = await k2.project.list()` */
        list(args?: Record<string, never>): Promise<AppProjectList>;
        /** A page of a project chat, oldest first: the latest page, afterSeq to catch up, or beforeSeq to page back. cap thread:read · GET /cli/skin/projects/messages. e.g. `const page = await k2.project.read({group, beforeSeq: oldest, limit: 50})` */
        read(args: {
            afterSeq?: number;
            beforeSeq?: number;
            group: string;
            limit?: number;
        }): Promise<AppProjectPage>;
        /** Post in a project chat as the guest (guest:<username>); needs project:write in every member room. cap project:write · POST /cli/skin/projects/post. e.g. `await k2.project.post({group, body: 'Can we ship Friday?'})` */
        post(args: {
            body: string;
            group: string;
        }): Promise<AppProjectPosted>;
        /** Mark a project chat read for this guest (the person guest:<principalId> on every App surface; an App's own pass has none). cap thread:read · POST /cli/skin/projects/read. e.g. `await k2.project.markRead({group, seq: newest})` */
        markRead(args: {
            group: string;
            seq?: number;
        }): Promise<AppProjectReadState>;
        /** Live lines of one project chat, shaped like Thread frames (collection project), plus a guest's own read frames (collection read); after a gap, project.read with afterSeq. cap thread:read · WS /cli/skin/projects/events?group=. e.g. `const off = k2.project.events({group}, (f) => append(f.doc))` */
        events(args: {
            group: string;
        }, onFrame: (frame: AppProjectFrame) => void, onError?: (error: K2Error) => void): () => void;
    };
}
/** The surface names, in catalog order (the client builds one object each). */
export declare const APP_SURFACES: readonly ["me", "roomAgents", "guests", "app", "password", "profile", "push", "pairing", "email", "thread", "files", "resources", "tickets", "wiki", "storeDocs", "db", "heartbeats", "activity", "agentIdentity", "room", "chat", "project"];

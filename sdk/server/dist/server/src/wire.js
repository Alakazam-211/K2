// The wire: catalog arguments → a request, and a live answer → the
// catalog's closed shape (runtime S23, room-scope RS7).
//
// - Argument names map to wire names through the row's `params`.
// - GET sends every argument in the query; POST sends JSON (the row's
//   `query` arguments ride the query string instead).
// - An answer is projected onto the row's schema: the fields it names are
//   read under their wire names (`responseNames`: relPath ← path), every
//   other field is dropped (banned host details such as session ids never
//   reach App code), and `json`/map values pass through untouched.
function isRecord(v) {
    return typeof v === 'object' && v !== null && !Array.isArray(v);
}
/** One query value as K2 reads it (arrays comma-joined, booleans true/false). */
export function queryValue(v) {
    if (v === undefined || v === null)
        return null;
    if (Array.isArray(v))
        return v.map((x) => String(x)).join(',');
    if (typeof v === 'object')
        return JSON.stringify(v);
    return String(v);
}
/** The request for `args` on an http row. */
export function buildRequest(http, args) {
    const q = new URLSearchParams();
    const body = {};
    for (const [name, value] of Object.entries(args ?? {})) {
        if (value === undefined)
            continue;
        const wire = http.params[name] ?? name;
        if (http.method === 'GET' || http.query.includes(name)) {
            const qv = queryValue(value);
            if (qv !== null)
                q.set(wire, qv);
        }
        else {
            body[wire] = value;
        }
    }
    const qs = q.toString();
    return {
        method: http.method,
        pathAndQuery: qs ? `${http.route}?${qs}` : http.route,
        body: http.method === 'POST' ? JSON.stringify(body) : null,
    };
}
function typesOf(s) {
    const t = s.type;
    if (typeof t === 'string')
        return [t];
    if (Array.isArray(t))
        return t.filter((x) => typeof x === 'string');
    return [];
}
/**
 * `value` read through `schema`: object properties under their wire names
 * (`names`: catalog → wire, at any depth), unnamed fields dropped, arrays
 * item by item, maps and `json` values as they are.
 */
export function project(schema, value, names) {
    if (value === null || value === undefined)
        return value;
    const types = typesOf(schema);
    if (types.includes('json'))
        return value;
    if (Array.isArray(value)) {
        const items = schema.items;
        return items ? value.map((x) => project(items, x, names)) : value;
    }
    if (isRecord(value)) {
        const props = schema.properties;
        if (props) {
            const out = {};
            for (const [name, sub] of Object.entries(props)) {
                const wire = names[name] ?? name;
                if (Object.prototype.hasOwnProperty.call(value, wire))
                    out[name] = project(sub, value[wire], names);
            }
            return out;
        }
        const map = schema.additionalProperties;
        if (isRecord(map)) {
            const out = {};
            for (const [k, v] of Object.entries(value))
                out[k] = project(map, v, names);
            return out;
        }
    }
    return value;
}

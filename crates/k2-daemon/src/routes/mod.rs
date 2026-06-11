//! HTTP route surface for the k2so-daemon binary.
//!
//! Pre-0.39.0 the entire route surface lived inline in `main.rs`'s
//! `handle_connection()` (1369 lines of inline match arms after the
//! peek+parse framing). The dispatch grew to 100+ arms across every
//! `/cli/*` domain, mixing tiny inline bodies, method-gated POST
//! handlers, and starts_with-prefixed forwarders to per-domain
//! modules. That bulk made it hard to see the route surface as a
//! whole, blocked easy refactors of the dispatch shape, and trapped
//! per-route comments in a single sprawling file.
//!
//! `routes::dispatcher::dispatch` now owns the entire match. `main.rs`
//! peeks the request, parses method/path, handles OPTIONS preflight,
//! and forwards to `dispatch`. Domain logic still lives in the same
//! per-domain modules (`heartbeat_routes`, `db_routes`,
//! `terminal_routes`, ...) — the dispatcher is just the spine.

pub mod dispatcher;
pub mod http;

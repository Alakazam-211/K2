//! K2's PostgREST-compatible grammar for an App's `/rest/v1` door
//! (prd-app-supabase-compat-v1 §7).
//!
//! The crate turns one request (method, path, query, the headers that
//! matter, body) into the SQL text the daemon runs inside the caller's
//! transaction ([`plan()`]), and reads the answer back into PostgREST's shape
//! ([`respond()`]). It has no daemon types and no Postgres driver, so it
//! tests alone: the parser and builder against fixed strings, and (feature
//! `testkit`) against a real throwaway Postgres.
//!
//! In v1 (PRD §7.2): GET, HEAD, POST, PATCH, DELETE on tables and views;
//! GET, HEAD and POST on `rpc/<fn>`; filters `eq neq gt gte lt lte like
//! ilike match imatch in is cs cd`, `not.`, one level of `or=`/`and=`;
//! select lists with aliases, casts and JSON paths; embeds through foreign
//! keys with aliases, hints, `!inner`/`!left`, nesting and their own
//! filters, order and limit; `order` with nulls first/last; limit, offset
//! and `Range`; singular answers; exact, planned and estimated counts;
//! inserts, bulk inserts, upserts (`on_conflict`, merge or ignore,
//! `missing=default`), updates and deletes with `return=representation`.
//! Anything else is [`ApiError::unsupported`]: a 400 naming the part.

pub mod build;
pub mod error;
pub mod parse;
pub mod plan;
pub mod respond;
pub mod schema;
pub mod sql;

#[cfg(feature = "testkit")]
pub mod testkit;

pub use error::{pg_status, ApiError};
pub use plan::{plan, Count, Kind, Options, Plan, Request, Stmt};
pub use respond::{respond, Out, Response};
pub use schema::{introspect_sql, SchemaCache};

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "testkit"))]
mod oracle;

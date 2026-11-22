//! The Supabase-compatible App data layer (prd-app-supabase-compat-v1),
//! one App at a time, off by default (R7).
//!
//! Lane A (identity and RLS): [`door_config`] (the stored settings the
//! doors read), [`claims`] (the §5.2 contract), [`users_sync`]
//! (`auth.users`), [`issuers`] (third-party tokens), [`identity_token`]
//! (verified identity on the App's own connection) and [`routes`] (the
//! verbs). The Postgres side is `sql::compat_ops` and
//! `k2_root_proto::k2_auth`.
//!
//! Lane C (the doors): the doors on an App's own host and gateway link
//! (`door`, `http`), the per-database connection pool (`pool`) and the
//! `/rest/v1` door (`rest_door`; its grammar is the `k2_rest` crate).

pub mod claims;
pub mod door;
pub mod door_config;
pub mod http;
pub mod identity_token;
pub mod issuers;
pub mod pool;
pub mod rest_door;
pub mod routes;
pub mod users_sync;

#[cfg(test)]
mod real_pg_compat;

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
//! `/rest/v1` door (`rest_door`; its grammar is the `k2_rest` crate), the
//! `/auth/v1` door (`auth_door_v1`, `auth_admin`).
//!
//! Lane D, the `/storage/v1` door and storage import (PRD §10, slices S5
//! and S5b):
//! - [`storage_sql`]: the `storage` schema an App's database gets
//!   (`storage.buckets`, `storage.objects`, `storage.foldername()` …);
//! - [`storage_pg`]: the per-request role switch and claims (PRD §3, §5.2)
//!   over a small connection pool;
//! - [`storage_auth`]: who a door request is (PRD §4);
//! - [`storage_bytes`]: where an object's bytes live (never named from
//!   the object's name);
//! - [`storage_ops`]: the verbs, each run as the caller so storage
//!   policies decide;
//! - [`storage_sign`]: signed download and upload URLs (HS256, a per-App
//!   key in the vault);
//! - [`storage_door`]: the HTTP door on an App's own hosts;
//! - [`storage_import`]: copy a Supabase project's buckets and objects in,
//!   resumable and checked by hash;
//! - [`storage_routes`]: `k2 app storage …`.

pub mod auth_admin;
pub mod auth_door_v1;
pub mod claims;
pub mod door;
pub mod door_config;
pub mod drill_http;
pub mod http;
pub mod identity_token;
pub mod issuers;
pub mod pool;
pub mod rest_door;
pub mod routes;
pub mod storage_auth;
pub mod storage_bind;
pub mod storage_bytes;
pub mod storage_door;
pub mod storage_err;
pub mod storage_import;
pub mod storage_ops;
pub mod storage_pg;
pub mod storage_routes;
pub mod storage_sign;
pub mod storage_sql;
pub mod users_sync;

#[cfg(test)]
mod real_pg_compat;
#[cfg(test)]
mod storage_real_pg;

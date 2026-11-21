//! The block's `http_get`, `http_post_query` and `http_post_json`: one
//! urllib call each, and what they hand back. Every failure is an answer
//! of its own: the daemon's JSON on an HTTP error, else `{"error": "HTTP
//! <code> on <route>"}`, and `{"error": "<route>: <e>"}` for anything
//! urlopen, the read or `json.loads` raised. A redirect is
//! `_K2NoRedirect`'s refusal ([`Stop::Refused`]), out of the block.

use super::super::{dumps, obj, s};
use super::{Hire, Stop, R};
use crate::out::json::Value;
use crate::out::pylib::json as pyjson;
use crate::transport::urllib::{decode, Method, Outcome, Request};
use std::time::Duration;

/// The helpers' `timeout=30`.
pub const DEFAULT: u64 = 30;

impl Hire<'_> {
    fn call(&self, req: Request, route: &str) -> R<Value> {
        let out = match self.op.open_ruled(&req) {
            Ok(out) => out,
            Err(r) => return Err(Stop::Refused(r.line)),
        };
        Ok(match out {
            Outcome::Answer { body: Ok(b), .. } => match pyjson::loads(&decode(&b)) {
                Ok(v) => v,
                Err(f) => failed(route, &f.message),
            },
            Outcome::Answer { body: Err(r), .. } => failed(route, &r.text()),
            Outcome::HttpError { code, body } => {
                match body.ok().and_then(|b| pyjson::loads(&decode(&b)).ok()) {
                    Some(v) => v,
                    None => obj(vec![("error", s(&format!("HTTP {code} on {route}")))]),
                }
            }
            Outcome::Raised(r) => failed(route, &r.text()),
        })
    }

    /// `if token: Authorization`.
    fn auth(&self, req: Request) -> Request {
        match self.op.token() {
            Some(t) => req.header("Authorization", format!("Bearer {t}")),
            None => req,
        }
    }

    /// `http_get(route, params)`.
    pub fn get(&self, route: &str, params: Vec<(&str, String)>) -> R<Value> {
        let req = Request::new(Method::Get, route, Duration::from_secs(DEFAULT)).query(params);
        self.call(self.auth(req), route)
    }

    /// `http_post_query(route, params, timeout)`: `data=b""`.
    pub fn post_query(&self, route: &str, params: Vec<(&str, String)>, timeout: u64) -> R<Value> {
        let req = Request::new(Method::Post, route, Duration::from_secs(timeout))
            .query(params)
            .data(Vec::new());
        self.call(self.auth(req), route)
    }

    /// `http_post_json(route, body)`: the token is the same whichever the
    /// block names (`tok=core_token`, `tok=proj_token`) outside a session
    /// with a passport, the only place this runs.
    pub fn post_json(&self, route: &str, body: &Value) -> R<Value> {
        let req = Request::new(Method::Post, route, Duration::from_secs(DEFAULT))
            .data(dumps(body).into_bytes())
            .header("Content-Type", "application/json");
        self.call(self.auth(req), route)
    }
}

/// `{"error": "%s: %s" % (route, e)}`.
fn failed(route: &str, e: &str) -> Value {
    obj(vec![("error", s(&format!("{route}: {e}")))])
}

//! PostgREST's error shape: `{code, message, details, hint}` and a status.

use serde_json::{json, Value};

/// One refusal, as PostgREST words it where it has a word for it, and with
/// a `K2REST…` code where the door refuses something PostgREST would do
/// (a part of the grammar v1 doesn't carry: never a wrong answer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
    /// A string, or (an ambiguous embed) PostgREST's array of candidates.
    pub details: Option<Value>,
    pub hint: Option<String>,
}

impl ApiError {
    pub fn new(status: u16, code: &str, message: impl Into<String>) -> Self {
        ApiError { status, code: code.to_string(), message: message.into(), details: None, hint: None }
    }

    pub fn details(mut self, d: impl Into<Value>) -> Self {
        self.details = Some(d.into());
        self
    }

    pub fn hint(mut self, h: impl Into<String>) -> Self {
        self.hint = Some(h.into());
        self
    }

    pub fn body(&self) -> Value {
        json!({ "code": self.code, "message": self.message, "details": self.details, "hint": self.hint })
    }

    /// A query parameter PostgREST can't parse (PGRST100).
    pub fn parse(message: impl Into<String>, details: impl Into<Value>) -> Self {
        ApiError::new(400, "PGRST100", message).details(details)
    }

    /// A part of PostgREST's grammar the v1 door doesn't carry. The App's
    /// status counts these (`k2 app rest unsupported`).
    pub fn unsupported(part: &str) -> Self {
        ApiError::new(400, "K2REST001", format!("{part} is not supported by K2's /rest/v1 yet"))
            .hint("K2's door carries the PostgREST grammar the Supabase-compat PRD lists (k2 study app-rest); this part was refused rather than answered wrongly")
    }
}

/// PostgREST's SQLSTATE → HTTP status table. `authed`: a 42501 is 403 for
/// a signed-in caller, 401 for anon.
pub fn pg_status(code: &str, authed: bool) -> u16 {
    match code {
        "23503" | "23505" => 409,
        "25006" => 405,
        "42883" | "42P01" => 404,
        "42P17" => 500,
        "42501" => {
            if authed {
                403
            } else {
                401
            }
        }
        "P0001" => 400,
        // RAISE … USING ERRCODE = 'PT404': the function picks the status.
        c if c.len() == 5 && c.starts_with("PT") && c[2..].parse::<u16>().is_ok_and(|n| (100..600).contains(&n)) => c[2..].parse().unwrap_or(400),
        _ => match code.get(..2).unwrap_or("") {
            "08" | "53" => 503,
            "0L" | "0P" | "28" => 403,
            "54" => 413,
            "09" | "25" | "2D" | "38" | "39" | "3B" | "40" | "55" | "57" | "58" | "F0" | "HV" | "P0" | "XX" => 500,
            _ => 400,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn postgrest_status_table() {
        assert_eq!(pg_status("23505", true), 409);
        assert_eq!(pg_status("23503", true), 409);
        assert_eq!(pg_status("23502", true), 400);
        assert_eq!(pg_status("42501", true), 403);
        assert_eq!(pg_status("42501", false), 401);
        assert_eq!(pg_status("42P01", true), 404);
        assert_eq!(pg_status("57014", true), 500);
        assert_eq!(pg_status("P0001", true), 400);
        assert_eq!(pg_status("PT404", true), 404);
        assert_eq!(pg_status("PTxyz", true), 400);
        assert_eq!(pg_status("25006", true), 405);
        assert_eq!(pg_status("53300", true), 503);
    }
}

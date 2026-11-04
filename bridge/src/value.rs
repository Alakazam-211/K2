//! The one conversion from a board value to the kit's: report details and the read verbs' JSON
//! alike, so a report's values match what `read` gives.
//!
//! - A number is always a float ([`KValue::float`]), never an integer, so its type never depends
//!   on its value.
//! - -0.0 comes out as 0, as the file writes it (`json::to_canonical`): a report's value and
//!   `read`'s are the file's.
//! - A number that isn't finite is a [`HostBug`]: the format refuses one on the way in, so it means
//!   something upstream broke. It's never dropped.

use crate::bug::HostBug;
use k2_mpc_base::errors::Value as KValue;
use k2_mpc_board::json::Value as BValue;
use k2_mpc_board::verbs::ObjectView;

/// A board value as the kit's value.
pub fn value_out(v: &BValue) -> Result<KValue, HostBug> {
    let mut at = String::new();
    walk(v, &mut at)
}

/// What `read` gives for an object, through [`value_out`].
pub fn read_value(view: &ObjectView) -> Result<KValue, HostBug> {
    value_out(&view.to_value())
}

fn walk(v: &BValue, at: &mut String) -> Result<KValue, HostBug> {
    Ok(match v {
        BValue::Null => KValue::Null,
        BValue::Bool(b) => KValue::Bool(*b),
        BValue::Num(n) => number(*n, at)?,
        BValue::Str(s) => KValue::Str(s.clone()),
        BValue::Arr(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let len = at.len();
                at.push('/');
                at.push_str(&i.to_string());
                out.push(walk(item, at)?);
                at.truncate(len);
            }
            KValue::List(out)
        }
        BValue::Obj(fields) => {
            let mut out = Vec::with_capacity(fields.len());
            for (name, item) in fields {
                let len = at.len();
                at.push('/');
                at.push_str(&name.replace('~', "~0").replace('/', "~1"));
                out.push((name.clone(), walk(item, at)?));
                at.truncate(len);
            }
            KValue::Obj(out)
        }
    })
}

/// A board number: -0.0 as 0, as the file writes it; not finite, a host bug.
fn number(n: f64, at: &str) -> Result<KValue, HostBug> {
    let n = if n == 0.0 { 0.0 } else { n };
    KValue::float(n).ok_or_else(|| HostBug::NotFinite { at: at.to_string() })
}

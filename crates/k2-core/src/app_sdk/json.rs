//! An order-keeping JSON value for editing an App's package.json: keys stay
//! where the author put them (serde_json's map sorts them in this build).

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

#[derive(Debug, Clone, PartialEq)]
pub enum J {
    Null,
    Bool(bool),
    Num(serde_json::Number),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    pub fn obj() -> J {
        J::Obj(Vec::new())
    }
    pub fn as_obj(&self) -> Option<&Vec<(String, J)>> {
        match self {
            J::Obj(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_obj_mut(&mut self) -> Option<&mut Vec<(String, J)>> {
        match self {
            J::Obj(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn get(&self, k: &str) -> Option<&J> {
        self.as_obj()?.iter().find(|(x, _)| x == k).map(|(_, v)| v)
    }
    pub fn get_mut(&mut self, k: &str) -> Option<&mut J> {
        self.as_obj_mut()?.iter_mut().find(|(x, _)| x == k).map(|(_, v)| v)
    }
    /// Set `k` in place (or append it). Panics on a non-object.
    pub fn set(&mut self, k: &str, v: J) {
        let m = self.as_obj_mut().expect("set on an object");
        match m.iter_mut().find(|(x, _)| x == k) {
            Some(slot) => slot.1 = v,
            None => m.push((k.to_string(), v)),
        }
    }
    /// Remove `k`; true when it was there.
    pub fn remove(&mut self, k: &str) -> bool {
        let Some(m) = self.as_obj_mut() else { return false };
        let before = m.len();
        m.retain(|(x, _)| x != k);
        m.len() != before
    }

    /// Pretty text with `indent` (npm's own style: a trailing newline).
    pub fn to_pretty(&self, indent: &[u8]) -> String {
        let mut buf = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(indent);
        let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
        self.serialize(&mut ser).expect("serialize to memory");
        let mut s = String::from_utf8(buf).expect("JSON is UTF-8");
        s.push('\n');
        s
    }
}

impl Serialize for J {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            J::Null => s.serialize_unit(),
            J::Bool(b) => s.serialize_bool(*b),
            J::Num(n) => n.serialize(s),
            J::Str(x) => s.serialize_str(x),
            J::Arr(a) => {
                let mut q = s.serialize_seq(Some(a.len()))?;
                for v in a {
                    q.serialize_element(v)?;
                }
                q.end()
            }
            J::Obj(m) => {
                let mut q = s.serialize_map(Some(m.len()))?;
                for (k, v) in m {
                    q.serialize_entry(k, v)?;
                }
                q.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for J {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<J, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = J;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("JSON")
            }
            fn visit_unit<E>(self) -> Result<J, E> {
                Ok(J::Null)
            }
            fn visit_none<E>(self) -> Result<J, E> {
                Ok(J::Null)
            }
            fn visit_bool<E>(self, b: bool) -> Result<J, E> {
                Ok(J::Bool(b))
            }
            fn visit_i64<E>(self, n: i64) -> Result<J, E> {
                Ok(J::Num(n.into()))
            }
            fn visit_u64<E>(self, n: u64) -> Result<J, E> {
                Ok(J::Num(n.into()))
            }
            fn visit_f64<E: de::Error>(self, n: f64) -> Result<J, E> {
                serde_json::Number::from_f64(n).map(J::Num).ok_or_else(|| E::custom("not a JSON number"))
            }
            fn visit_str<E>(self, s: &str) -> Result<J, E> {
                Ok(J::Str(s.to_string()))
            }
            fn visit_string<E>(self, s: String) -> Result<J, E> {
                Ok(J::Str(s))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<J, A::Error> {
                let mut v = Vec::new();
                while let Some(x) = a.next_element()? {
                    v.push(x);
                }
                Ok(J::Arr(v))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<J, A::Error> {
                let mut m: Vec<(String, J)> = Vec::new();
                while let Some((k, v)) = a.next_entry::<String, J>()? {
                    // A repeated key: the last one wins, like JSON.parse.
                    m.retain(|(x, _)| x != &k);
                    m.push((k, v));
                }
                Ok(J::Obj(m))
            }
        }
        d.deserialize_any(V)
    }
}

/// The indent of a JSON text (npm keeps the file's own): the leading
/// whitespace of its first indented line, else two spaces.
pub fn indent_of(text: &str) -> Vec<u8> {
    text.lines()
        .skip(1)
        .find_map(|l| {
            let ws: String = l.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            (!ws.is_empty()).then_some(ws)
        })
        .unwrap_or_else(|| "  ".into())
        .into_bytes()
}

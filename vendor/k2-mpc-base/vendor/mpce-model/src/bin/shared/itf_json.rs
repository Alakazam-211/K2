//! A minimal JSON reader for ITF traces (`quint run --out-itf`), shared by the ITF replay
//! binaries (`#[path]` module; not a binary target itself).
#![allow(dead_code)]

// ---------------------------------------------------------------------------------------
// Minimal JSON

#[derive(Debug, Clone)]
pub enum J {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] as char).is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn val(&mut self) -> Result<J, String> {
        self.ws();
        let c = *self.s.get(self.i).ok_or("eof")?;
        match c {
            b'{' => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.s[self.i] == b'}' {
                    self.i += 1;
                    return Ok(J::Obj(v));
                }
                loop {
                    self.ws();
                    let k = match self.val()? {
                        J::Str(k) => k,
                        _ => return Err("key".into()),
                    };
                    self.ws();
                    if self.s[self.i] != b':' {
                        return Err(format!("':' at {}", self.i));
                    }
                    self.i += 1;
                    v.push((k, self.val()?));
                    self.ws();
                    match self.s[self.i] {
                        b',' => self.i += 1,
                        b'}' => {
                            self.i += 1;
                            return Ok(J::Obj(v));
                        }
                        _ => return Err(format!("object at {}", self.i)),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                self.ws();
                if self.s[self.i] == b']' {
                    self.i += 1;
                    return Ok(J::Arr(v));
                }
                loop {
                    v.push(self.val()?);
                    self.ws();
                    match self.s[self.i] {
                        b',' => self.i += 1,
                        b']' => {
                            self.i += 1;
                            return Ok(J::Arr(v));
                        }
                        _ => return Err(format!("array at {}", self.i)),
                    }
                }
            }
            b'"' => {
                self.i += 1;
                let mut out = String::new();
                loop {
                    let c = *self.s.get(self.i).ok_or("eof in string")?;
                    self.i += 1;
                    match c {
                        b'"' => return Ok(J::Str(out)),
                        b'\\' => {
                            let e = self.s[self.i];
                            self.i += 1;
                            match e {
                                b'n' => out.push('\n'),
                                b't' => out.push('\t'),
                                b'r' => out.push('\r'),
                                b'b' => out.push('\u{8}'),
                                b'f' => out.push('\u{c}'),
                                b'u' => {
                                    let h = std::str::from_utf8(&self.s[self.i..self.i + 4]).map_err(|e| e.to_string())?;
                                    self.i += 4;
                                    out.push(char::from_u32(u32::from_str_radix(h, 16).map_err(|e| e.to_string())?).unwrap_or('?'));
                                }
                                other => out.push(other as char),
                            }
                        }
                        _ => {
                            // copy one UTF-8 sequence
                            let start = self.i - 1;
                            let mut end = self.i;
                            while end < self.s.len() && (self.s[end] & 0xC0) == 0x80 {
                                end += 1;
                            }
                            out.push_str(std::str::from_utf8(&self.s[start..end]).map_err(|e| e.to_string())?);
                            self.i = end;
                        }
                    }
                }
            }
            b't' => {
                self.i += 4;
                Ok(J::Bool(true))
            }
            b'f' => {
                self.i += 5;
                Ok(J::Bool(false))
            }
            b'n' => {
                self.i += 4;
                Ok(J::Null)
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len() && matches!(self.s[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
                    self.i += 1;
                }
                if start == self.i {
                    return Err(format!("unexpected {:?} at {}", c as char, self.i));
                }
                Ok(J::Num(String::from_utf8_lossy(&self.s[start..self.i]).into_owned()))
            }
        }
    }
}

pub fn parse(s: &str) -> Result<J, String> {
    P { s: s.as_bytes(), i: 0 }.val()
}

impl J {
    pub fn get(&self, k: &str) -> Option<&J> {
        match self {
            J::Obj(v) => v.iter().find(|(kk, _)| kk == k).map(|(_, v)| v),
            _ => None,
        }
    }
    /// a state variable, by its unqualified name (`mod::text::ev` or `ev`)
    pub fn var(&self, name: &str) -> Option<&J> {
        match self {
            J::Obj(v) => v.iter().find(|(k, _)| k == name || k.rsplit("::").next() == Some(name)).map(|(_, v)| v),
            _ => None,
        }
    }
    // ITF encodings
    pub fn int(&self) -> i64 {
        match self {
            J::Num(n) => n.parse().expect("int"),
            J::Obj(_) => match self.get("#bigint") {
                Some(J::Str(s)) => s.parse().expect("bigint"),
                _ => panic!("not an int: {self:?}"),
            },
            _ => panic!("not an int: {self:?}"),
        }
    }
    pub fn bool(&self) -> bool {
        match self {
            J::Bool(b) => *b,
            _ => panic!("not a bool: {self:?}"),
        }
    }
    pub fn str(&self) -> &str {
        match self {
            J::Str(s) => s,
            _ => panic!("not a string: {self:?}"),
        }
    }
    pub fn items(&self, tag: &str) -> &[J] {
        match self.get(tag) {
            Some(J::Arr(v)) => v,
            _ => panic!("not a {tag}: {self:?}"),
        }
    }
    pub fn list(&self) -> &[J] {
        match self {
            J::Arr(v) => v,
            _ => panic!("not a list: {self:?}"),
        }
    }
}


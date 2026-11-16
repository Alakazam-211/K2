//! The command splitter (photoplot 3.1, 3.3): a file is a stream of word commands (text up to
//! `*`) and extended commands (`%`, one or more words, `%`). Line breaks are allowed only right
//! after `*`, `%` or `,`, and never belong to a word; other control characters are never
//! allowed. Which bytes a command's own syntax allows is the command's to check.

use std::borrow::Cow;

use crate::error::{Code, LimitKind, ReadError};

/// One word, without its `*`, and the offset of its first byte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Word<'a> {
    pub text: Cow<'a, str>,
    pub at: usize,
}

/// One command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cmd<'a> {
    /// A word command.
    Word(Word<'a>),
    /// An extended command's words, and the offset of its opening `%`.
    Extended(Vec<Word<'a>>, usize),
}

pub(crate) struct Lexer<'a> {
    s: &'a str,
    pos: usize,
    /// Whether a line break may come next: right after `*`, `%` or `,`.
    nl_ok: bool,
    max_word: usize,
}

fn syntax(at: usize) -> ReadError {
    ReadError::photoplot(Code::Syntax, "3.3", at)
}

impl<'a> Lexer<'a> {
    pub fn new(s: &'a str, max_word: usize) -> Self {
        Lexer {
            s,
            pos: 0,
            nl_ok: false,
            max_word,
        }
    }

    /// Where the next command starts (or the end).
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// The bytes not read yet.
    pub fn rest(&self) -> &'a [u8] {
        &self.s.as_bytes()[self.pos..]
    }

    fn skip_breaks(&mut self) -> Result<(), ReadError> {
        let b = self.s.as_bytes();
        while self.pos < b.len() && matches!(b[self.pos], b'\r' | b'\n') {
            if !self.nl_ok {
                return Err(ReadError::photoplot(Code::BadLineBreak, "3.1", self.pos));
            }
            self.pos += 1;
        }
        Ok(())
    }

    /// The next command, or `None` at the end of the input.
    pub fn next_cmd(&mut self) -> Result<Option<Cmd<'a>>, ReadError> {
        self.skip_breaks()?;
        let b = self.s.as_bytes();
        if self.pos >= b.len() {
            return Ok(None);
        }
        let at = self.pos;
        if b[self.pos] != b'%' {
            return Ok(Some(Cmd::Word(self.word()?)));
        }
        self.pos += 1;
        self.nl_ok = true;
        let mut words = Vec::new();
        loop {
            self.skip_breaks()?;
            if self.pos >= b.len() {
                return Err(syntax(at));
            }
            if b[self.pos] == b'%' {
                if words.is_empty() {
                    return Err(syntax(at));
                }
                self.pos += 1;
                self.nl_ok = true;
                return Ok(Some(Cmd::Extended(words, at)));
            }
            words.push(self.word()?);
        }
    }

    /// One word up to its `*`.
    fn word(&mut self) -> Result<Word<'a>, ReadError> {
        let b = self.s.as_bytes();
        let start = self.pos;
        let mut owned: Option<String> = None;
        let mut seg = self.pos;
        loop {
            if self.pos - start > self.max_word {
                return Err(ReadError::limit(LimitKind::Word, start));
            }
            let Some(&c) = b.get(self.pos) else {
                return Err(syntax(start));
            };
            match c {
                b'*' => {
                    let text = match owned {
                        Some(mut o) => {
                            o.push_str(&self.s[seg..self.pos]);
                            Cow::Owned(o)
                        }
                        None => Cow::Borrowed(&self.s[start..self.pos]),
                    };
                    self.pos += 1;
                    self.nl_ok = true;
                    return Ok(Word { text, at: start });
                }
                b'%' => return Err(syntax(start)),
                b'\r' | b'\n' => {
                    if !self.nl_ok {
                        return Err(ReadError::photoplot(Code::BadLineBreak, "3.1", self.pos));
                    }
                    owned
                        .get_or_insert_with(String::new)
                        .push_str(&self.s[seg..self.pos]);
                    while matches!(b.get(self.pos), Some(b'\r' | b'\n')) {
                        self.pos += 1;
                    }
                    seg = self.pos;
                }
                b',' => {
                    self.nl_ok = true;
                    self.pos += 1;
                }
                c if c < 0x20 || c == 0x7f => {
                    return Err(ReadError::photoplot(Code::BadCharacter, "3.1", self.pos));
                }
                _ => {
                    self.nl_ok = false;
                    self.pos += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(s: &str) -> Result<Vec<Cmd<'_>>, ReadError> {
        let mut l = Lexer::new(s, 1 << 20);
        let mut v = Vec::new();
        while let Some(c) = l.next_cmd()? {
            v.push(c);
        }
        Ok(v)
    }

    #[test]
    fn words_and_extended_commands_split() {
        let v = all("%FSLAX26Y26*%\r\nG04 a b*\n%AMX*\n1,1,\n0.5,0,0*%D10*").unwrap();
        assert_eq!(v.len(), 4);
        match &v[2] {
            Cmd::Extended(w, at) => {
                assert_eq!(*at, 24);
                assert_eq!(w[1].text, "1,1,0.5,0,0");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(all("G04 x\n*").unwrap_err().code, Code::BadLineBreak);
        assert_eq!(all("\nD10*").unwrap_err().code, Code::BadLineBreak);
        assert_eq!(all("D10").unwrap_err().code, Code::Syntax);
        assert_eq!(all("%FS*").unwrap_err().code, Code::Syntax);
        assert_eq!(all("G04 \t*").unwrap_err().code, Code::BadCharacter);
    }
}

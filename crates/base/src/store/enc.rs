//! A small, versioned binary encoding for store records.
//!
//! Hand-written instead of a serialization crate: the records are few and simple, the
//! format must stay stable across releases, and every read is bounds-checked (a damaged
//! store gives an error, never a panic).

use super::StoreError;

#[derive(Default)]
pub(crate) struct W(pub Vec<u8>);

impl W {
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(u32::try_from(v.len()).expect("record field over 4 GiB"));
        self.0.extend_from_slice(v);
        self
    }
    pub fn str(&mut self, v: &str) -> &mut Self {
        self.bytes(v.as_bytes())
    }
    pub fn opt_bytes(&mut self, v: Option<&[u8]>) -> &mut Self {
        match v {
            None => self.u8(0),
            Some(b) => self.u8(1).bytes(b),
        }
    }
    pub fn opt_str(&mut self, v: Option<&str>) -> &mut Self {
        self.opt_bytes(v.map(str::as_bytes))
    }
    pub fn strs(&mut self, v: &[String]) -> &mut Self {
        self.u32(v.len() as u32);
        for s in v {
            self.str(s);
        }
        self
    }
}

pub(crate) struct R<'a> {
    b: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> R<'a> {
    pub fn new(b: &'a [u8], what: &'static str) -> Self {
        Self { b, at: 0, what }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], StoreError> {
        let end = self
            .at
            .checked_add(n)
            .filter(|e| *e <= self.b.len())
            .ok_or(StoreError::Corrupt(self.what))?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, StoreError> {
        Ok(self.take(1)?[0])
    }
    pub fn u32(&mut self) -> Result<u32, StoreError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, StoreError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn bytes(&mut self) -> Result<Vec<u8>, StoreError> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    /// A [`R::bytes`] field read in place: checked the same way, nothing copied.
    pub fn bytes_ref(&mut self) -> Result<&'a [u8], StoreError> {
        let n = self.u32()? as usize;
        self.take(n)
    }
    pub fn str(&mut self) -> Result<String, StoreError> {
        String::from_utf8(self.bytes()?).map_err(|_| StoreError::Corrupt(self.what))
    }
    pub fn opt_bytes(&mut self) -> Result<Option<Vec<u8>>, StoreError> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.bytes()?)),
            _ => Err(StoreError::Corrupt(self.what)),
        }
    }
    pub fn opt_str(&mut self) -> Result<Option<String>, StoreError> {
        self.opt_bytes()?
            .map(|b| String::from_utf8(b).map_err(|_| StoreError::Corrupt(self.what)))
            .transpose()
    }
    pub fn strs(&mut self) -> Result<Vec<String>, StoreError> {
        let n = self.u32()? as usize;
        // each string is at least 4 bytes (its length), so bound n by what's left
        if n > (self.b.len() - self.at) / 4 {
            return Err(StoreError::Corrupt(self.what));
        }
        (0..n).map(|_| self.str()).collect()
    }
    pub fn end(self) -> Result<(), StoreError> {
        if self.at == self.b.len() {
            Ok(())
        } else {
            Err(StoreError::Corrupt(self.what))
        }
    }
}

/// The fuzz targets' oracle for one record kind (feature `fuzzing`): `dec` refuses `b`
/// with [`StoreError::Corrupt`], or what it decodes re-encodes to bytes it decodes again,
/// and re-encoding that gives the same bytes (a fixpoint). With `exact`, the encoding is
/// canonical: the decoded record re-encodes to `b` itself. Panics on a violation (a
/// finding); returns whether `b` decoded.
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_fixpoint<T>(
    what: &str,
    b: &[u8],
    exact: bool,
    dec: impl Fn(&[u8]) -> Result<T, StoreError>,
    enc: impl Fn(&T) -> Vec<u8>,
) -> bool {
    let x = match dec(b) {
        Ok(x) => x,
        Err(StoreError::Corrupt(_)) => return false,
        Err(e) => panic!("{what}: refused with an error other than Corrupt: {e:?}"),
    };
    let e1 = enc(&x);
    if exact {
        assert_eq!(e1, b, "{what}: decoded, but re-encodes to other bytes");
    }
    let y = dec(&e1).unwrap_or_else(|e| panic!("{what}: re-encoded record refused: {e:?}"));
    assert_eq!(enc(&y), e1, "{what}: re-encoding is not a fixpoint");
    true
}

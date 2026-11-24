//! `no_proxy`: the hosts that never use a proxy, as the curl command reads
//! the list.

use std::net::{Ipv4Addr, Ipv6Addr};

/// A `no_proxy` list ([`NoProxy::parse`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NoProxy {
    all: bool,
    tokens: Vec<Vec<u8>>,
}

impl NoProxy {
    /// Reads a list, as bytes (an environment value need not be UTF-8):
    ///
    /// - exactly `*` matches every host (`*` among other tokens is a
    ///   token like any other, and matches nothing);
    /// - otherwise the tokens are separated by commas, blanks or tabs, and
    ///   empty ones are dropped.
    pub fn parse(list: &[u8]) -> NoProxy {
        if list == b"*" {
            return NoProxy {
                all: true,
                tokens: Vec::new(),
            };
        }
        let tokens = list
            .split(|b| matches!(b, b',' | b' ' | b'\t'))
            .filter(|t| !t.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        NoProxy { all: false, tokens }
    }

    /// Whether the list matches every host (`*`).
    pub fn is_all(&self) -> bool {
        self.all
    }

    /// Whether `host` (a URL's host: a name, four numbers, or an IPv6
    /// address with or without brackets) is listed:
    ///
    /// - a name matches a token that is the same name, or a domain it is
    ///   under at a dot (`example.com` lists `www.example.com`, not
    ///   `notexample.com`), in any case, with a leading or trailing dot on
    ///   the token and a trailing dot on the name ignored;
    /// - an address matches a token that is the same address, or a CIDR
    ///   range `address/bits` that holds it (`bits` read as C's `atoi`
    ///   reads a number; 0, or the address's full length, means the exact
    ///   address; more than that, nothing). An IPv6 address matches only
    ///   the same address, written without brackets: curl 8.5 matched no
    ///   IPv6 range. Names never match an address.
    pub fn matches(&self, host: &str) -> bool {
        if self.all {
            return true;
        }
        let bare = host
            .strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host);
        if let Ok(v6) = bare.parse::<Ipv6Addr>() {
            return self.tokens.iter().any(|t| cidr6(v6, t));
        }
        if let Ok(v4) = host.parse::<Ipv4Addr>() {
            return self.tokens.iter().any(|t| cidr4(v4, t));
        }
        let name = host.strip_suffix('.').unwrap_or(host).as_bytes();
        self.tokens.iter().any(|t| name_match(name, t))
    }
}

fn name_match(name: &[u8], token: &[u8]) -> bool {
    let t = token.strip_suffix(b".").unwrap_or(token);
    let t = t.strip_prefix(b".").unwrap_or(t);
    if t.len() == name.len() {
        return name.eq_ignore_ascii_case(t);
    }
    // An empty token here was `.` or `..`: it matches nothing.
    !t.is_empty()
        && t.len() < name.len()
        && name[name.len() - t.len() - 1] == b'.'
        && name[name.len() - t.len()..].eq_ignore_ascii_case(t)
}

/// `address[/bits]` as text, and the bits (`None`: no `/`).
fn split_cidr(token: &[u8]) -> Option<(&str, Option<i64>)> {
    let (net, bits) = match token.iter().position(|&b| b == b'/') {
        Some(i) => (&token[..i], Some(atoi(&token[i + 1..]))),
        None => (token, None),
    };
    Some((std::str::from_utf8(net).ok()?, bits))
}

fn cidr4(ip: Ipv4Addr, token: &[u8]) -> bool {
    let Some((net, bits)) = split_cidr(token) else {
        return false;
    };
    let Ok(net) = net.parse::<Ipv4Addr>() else {
        return false;
    };
    masked_eq(&ip.octets(), &net.octets(), bits.unwrap_or(0))
}

/// An IPv6 token: the same address, written without brackets. curl
/// 8.5 matched no IPv6 range in the black-box table (`::/127` left `::1`
/// proxied), so a token with a `/` matches nothing.
fn cidr6(ip: Ipv6Addr, token: &[u8]) -> bool {
    let Some((net, bits)) = split_cidr(token) else {
        return false;
    };
    bits.is_none() && net.parse::<Ipv6Addr>().is_ok_and(|net| net == ip)
}

/// The first `bits` bits of `a` and `b` are the same; 0 or all of them:
/// the whole address; more than all, or less than 0: no match.
fn masked_eq(a: &[u8], b: &[u8], bits: i64) -> bool {
    let all = (a.len() * 8) as i64;
    if !(0..=all).contains(&bits) {
        return false;
    }
    let bits = usize::try_from(if bits == 0 { all } else { bits }).unwrap_or(0);
    let (whole, rest) = (bits / 8, bits % 8);
    if a[..whole] != b[..whole] {
        return false;
    }
    if rest == 0 {
        return true;
    }
    let mask = 0xffu8 << (8 - rest);
    a[whole] & mask == b[whole] & mask
}

/// C's `atoi`: leading white space, a sign, then digits; anything else
/// ends it. Out of range reads as -1 (never a match).
fn atoi(s: &[u8]) -> i64 {
    let start = s
        .iter()
        .position(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c))
        .unwrap_or(s.len());
    let s = &s[start..];
    let (neg, digits) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let n = digits.iter().take_while(|b| b.is_ascii_digit()).count();
    let mut v: i64 = 0;
    for d in &digits[..n] {
        v = v * 10 + i64::from(d - b'0');
        if v > i64::from(i32::MAX) {
            return -1;
        }
    }
    if neg {
        -v
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rows k5's transport proved against curl 8.5 (its earlier
    /// table), and the forms that table left out.
    #[test]
    fn rows() {
        let rows: &[(&str, &[u8], bool)] = &[
            ("box.example", b"*", true),
            ("box.example", b"box.example", true),
            ("box.example", b"BOX.Example", true),
            ("box.example", b".box.example", true),
            ("box.example", b"box.example.", true),
            ("box.example.", b"box.example", true),
            ("a.box.example", b"box.example", true),
            ("a.box.example", b".box.example", true),
            ("nobox.example", b"box.example", false),
            ("box.example", b"a.box.example", false),
            ("box.example", b"other.example,box.example", true),
            ("box.example", b",,other.example,,", false),
            ("box.example", b"example", true),
            ("box.example", b".", false),
            ("box.example", b"*.example", false),
            ("box.example", b"*,x", false),
            ("box.example", b"other.example box.example", true),
            ("box.example", b"box.example\tother", true),
            ("box.example", b"localhost, box.example", true),
            ("box.example", b"box.example:443", false),
            ("box.example", b"\xff,box.example", true),
            ("10.1.2.3", b"10.1.2.3", true),
            ("10.1.2.3", b"10.1.2.4", false),
            ("10.1.2.3", b"10.0.0.0/8", true),
            ("10.1.2.3", b"10.1.0.0/16", true),
            ("10.1.2.3", b"10.2.0.0/16", false),
            ("10.1.2.3", b"10.1.2.0/24x", true),
            ("10.1.2.3", b"10.1.2.3/32", true),
            ("10.1.2.3", b"10.1.2.0/0", false),
            ("10.1.2.3", b"10.1.2.0/33", false),
            ("10.1.2.3", b"10.1.2.0/-1", false),
            ("10.1.2.3", b"0.0.0.0/1", true),
            ("10.1.2.3", b"10.1.2.2/31", true),
            ("10.1.2.3", b"10.1.2.4/31", false),
            ("10.1.2.3", b"box.example", false),
            ("10.1.2.3", b"2.3", false),
            ("[2001:db8::5]", b"2001:db8::5", true),
            ("2001:db8::5", b"[2001:db8::5]", false),
            ("[2001:db8::5]", b"2001:db8::/32", false),
            ("[2001:db8::5]", b"2001:db8::5/128", false),
            ("[2001:db8::5]", b"2001:db8::4/127", false),
            ("[2001:db8::5]", b"db8::5", false),
        ];
        for (host, list, want) in rows {
            assert_eq!(
                NoProxy::parse(list).matches(host),
                *want,
                "{host} in {:?}",
                String::from_utf8_lossy(list)
            );
        }
    }

    #[test]
    fn atoi_reads_as_c_does() {
        for (s, want) in [
            (&b"24"[..], 24),
            (b" 24x", 24),
            (b"+8", 8),
            (b"-1", -1),
            (b"", 0),
            (b"x", 0),
            (b"99999999999", -1),
        ] {
            assert_eq!(atoi(s), want, "{s:?}");
        }
    }
}

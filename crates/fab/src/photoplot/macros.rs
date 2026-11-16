//! Aperture macros (photoplot 4.5): a macro's body as parsed (primitives with their parameter
//! expressions, variable definitions), and its evaluation for an aperture's values, which
//! checks each primitive's parameters (4.5.1). Evaluation uses floating point only to check;
//! the model keeps the body and the aperture's decimals as written.

use crate::dec::Dec;
use crate::error::{Code, Deprecated};

/// A parameter expression (photoplot 4.5.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    /// A decimal constant.
    Num(Dec),
    /// `$n`.
    Var(u32),
    /// A sign before a term.
    Neg(Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
}

/// One item of a macro body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroItem {
    /// Primitive 0: a comment.
    Comment(String),
    /// A primitive: its code and parameters.
    Primitive { code: u8, params: Vec<Expr> },
    /// `$n=expr`.
    Define { var: u32, expr: Expr },
}

/// An aperture macro: its name and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macro {
    pub name: String,
    pub body: Vec<MacroItem>,
    /// The offset of its `%AM`.
    pub offset: usize,
}

impl Macro {
    /// The primitive codes, in order (comments left out).
    pub fn primitive_codes(&self) -> Vec<u8> {
        self.body
            .iter()
            .filter_map(|i| match i {
                MacroItem::Primitive { code, .. } => Some(*code),
                _ => None,
            })
            .collect()
    }
}

/// What parsing a body found: the item, and a deprecated primitive it uses, if any.
pub(crate) struct Parsed {
    pub item: MacroItem,
    pub deprecated: Option<Deprecated>,
}

struct P<'a> {
    b: &'a [u8],
    i: usize,
    depth: usize,
    max_depth: usize,
}

/// Why an expression didn't parse.
pub(crate) enum ExprError {
    Syntax,
    TooDeep,
}

impl P<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn enter(&mut self) -> Result<(), ExprError> {
        self.depth += 1;
        if self.depth > self.max_depth {
            return Err(ExprError::TooDeep);
        }
        Ok(())
    }

    /// expr = [sign] term { sign term }
    fn expr(&mut self) -> Result<Expr, ExprError> {
        self.enter()?;
        let mut acc: Option<Expr> = None;
        loop {
            let sign = match self.peek() {
                Some(b'+') => {
                    self.i += 1;
                    b'+'
                }
                Some(b'-') => {
                    self.i += 1;
                    b'-'
                }
                _ if acc.is_none() => b'+',
                _ => break,
            };
            let t = self.term()?;
            acc = Some(match (acc, sign) {
                (None, b'-') => Expr::Neg(Box::new(t)),
                (None, _) => t,
                (Some(a), b'-') => Expr::Sub(Box::new(a), Box::new(t)),
                (Some(a), _) => Expr::Add(Box::new(a), Box::new(t)),
            });
        }
        self.depth -= 1;
        acc.ok_or(ExprError::Syntax)
    }

    /// term = factor { (x | /) factor }
    fn term(&mut self) -> Result<Expr, ExprError> {
        let mut acc = self.factor()?;
        loop {
            match self.peek() {
                Some(b'x') => {
                    self.i += 1;
                    let f = self.factor()?;
                    acc = Expr::Mul(Box::new(acc), Box::new(f));
                }
                Some(b'/') => {
                    self.i += 1;
                    let f = self.factor()?;
                    acc = Expr::Div(Box::new(acc), Box::new(f));
                }
                _ => return Ok(acc),
            }
        }
    }

    /// factor = ( expr ) | $n | unsigned decimal
    fn factor(&mut self) -> Result<Expr, ExprError> {
        match self.peek() {
            Some(b'(') => {
                self.i += 1;
                let e = self.expr()?;
                if self.peek() != Some(b')') {
                    return Err(ExprError::Syntax);
                }
                self.i += 1;
                Ok(e)
            }
            Some(b'$') => {
                self.i += 1;
                let n = self.digits();
                parse_var(n).map(Expr::Var).ok_or(ExprError::Syntax)
            }
            Some(c) if c.is_ascii_digit() || c == b'.' => {
                let s = self.i;
                while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == b'.') {
                    self.i += 1;
                }
                Dec::parse(&self.b[s..self.i], false)
                    .map(Expr::Num)
                    .map_err(|_| ExprError::Syntax)
            }
            _ => Err(ExprError::Syntax),
        }
    }

    fn digits(&mut self) -> &[u8] {
        let s = self.i;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.i += 1;
        }
        &self.b[s..self.i]
    }
}

/// The variable number of `$n`'s digits: a positive integer (leading zeros allowed).
fn parse_var(d: &[u8]) -> Option<u32> {
    if d.is_empty() || d.len() > 9 {
        return None;
    }
    let n: u32 = std::str::from_utf8(d).ok()?.parse().ok()?;
    (n > 0).then_some(n)
}

/// Parses one expression, the whole of `s`.
pub(crate) fn parse_expr(s: &str, max_depth: usize) -> Result<Expr, ExprError> {
    let mut p = P {
        b: s.as_bytes(),
        i: 0,
        depth: 0,
        max_depth,
    };
    let e = p.expr()?;
    if p.i != s.len() {
        return Err(ExprError::Syntax);
    }
    Ok(e)
}

/// How many parameters a primitive takes: exactly, or (outline) at least 7 and odd.
fn param_count_ok(code: u8, n: usize) -> bool {
    match code {
        1 => n == 4 || n == 5,
        20 | 2 => n == 7,
        21 | 22 => n == 6,
        4 => n >= 7 && n % 2 == 1,
        5 | 7 => n == 6,
        6 => n == 9,
        _ => false,
    }
}

/// Parses one word of a macro body.
pub(crate) fn parse_item(text: &str, max_depth: usize) -> Result<Parsed, Code> {
    let b = text.as_bytes();
    // a comment: `0`, then any string
    if b.first() == Some(&b'0') && !b.get(1).is_some_and(u8::is_ascii_digit) {
        return Ok(Parsed {
            item: MacroItem::Comment(text[1..].to_owned()),
            deprecated: None,
        });
    }
    if !text.is_ascii() {
        return Err(Code::MacroSyntax);
    }
    let map = |e: ExprError| match e {
        ExprError::Syntax => Code::MacroSyntax,
        ExprError::TooDeep => Code::Limit(crate::error::LimitKind::ExpressionDepth),
    };
    if let Some(rest) = text.strip_prefix('$') {
        let (lhs, rhs) = rest.split_once('=').ok_or(Code::MacroSyntax)?;
        if !lhs.bytes().all(|c| c.is_ascii_digit()) {
            return Err(Code::MacroSyntax);
        }
        let var = parse_var(lhs.as_bytes()).ok_or(Code::MacroSyntax)?;
        let expr = parse_expr(rhs, max_depth).map_err(map)?;
        return Ok(Parsed {
            item: MacroItem::Define { var, expr },
            deprecated: None,
        });
    }
    let (code_s, rest) = text.split_once(',').ok_or(Code::MacroSyntax)?;
    let code: u8 = match code_s {
        "1" => 1,
        "20" => 20,
        "21" => 21,
        "4" => 4,
        "5" => 5,
        "7" => 7,
        "2" => 2,
        "22" => 22,
        "6" => 6,
        _ => return Err(Code::MacroSyntax),
    };
    let deprecated = match code {
        2 => Some(Deprecated::Primitive2),
        22 => Some(Deprecated::Primitive22),
        6 => Some(Deprecated::Primitive6),
        _ => None,
    };
    let mut params = Vec::new();
    for p in rest.split(',') {
        params.push(parse_expr(p, max_depth).map_err(map)?);
    }
    if !param_count_ok(code, params.len()) {
        return Err(Code::MacroSyntax);
    }
    Ok(Parsed {
        item: MacroItem::Primitive { code, params },
        deprecated,
    })
}

/// Why an aperture's macro didn't evaluate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EvalError {
    /// A variable defined again, or a variable the aperture sets defined in the body
    /// (4.5.4.2).
    Redefined,
    /// A parameter invalid for its primitive, or a value that isn't a finite decimal (4.5.1).
    Primitive,
    /// Over the work budget.
    Work,
}

/// Evaluates `m` for an aperture's values `args` ($1, $2...), checking every primitive.
/// `work` is the budget left, in expression nodes; it is spent.
pub(crate) fn evaluate(m: &Macro, args: &[f64], work: &mut u64) -> Result<(), EvalError> {
    let mut vars: std::collections::BTreeMap<u32, f64> = std::collections::BTreeMap::new();
    for (i, a) in args.iter().enumerate() {
        vars.insert(i as u32 + 1, *a);
    }
    for item in &m.body {
        match item {
            MacroItem::Comment(_) => {}
            MacroItem::Define { var, expr } => {
                if vars.contains_key(var) {
                    return Err(EvalError::Redefined);
                }
                let v = eval(expr, &vars, work)?;
                vars.insert(*var, v);
            }
            MacroItem::Primitive { code, params } => {
                let mut v = Vec::with_capacity(params.len());
                for p in params {
                    v.push(eval(p, &vars, work)?);
                }
                check_primitive(*code, &v)?;
            }
        }
    }
    Ok(())
}

fn eval(
    e: &Expr,
    vars: &std::collections::BTreeMap<u32, f64>,
    work: &mut u64,
) -> Result<f64, EvalError> {
    if *work == 0 {
        return Err(EvalError::Work);
    }
    *work -= 1;
    let v = match e {
        Expr::Num(d) => d.to_f64(),
        // an undefined variable is 0 (4.5.4.2)
        Expr::Var(n) => vars.get(n).copied().unwrap_or(0.0),
        Expr::Neg(a) => -eval(a, vars, work)?,
        Expr::Add(a, b) => eval(a, vars, work)? + eval(b, vars, work)?,
        Expr::Sub(a, b) => eval(a, vars, work)? - eval(b, vars, work)?,
        Expr::Mul(a, b) => eval(a, vars, work)? * eval(b, vars, work)?,
        Expr::Div(a, b) => eval(a, vars, work)? / eval(b, vars, work)?,
    };
    if !v.is_finite() {
        return Err(EvalError::Primitive);
    }
    Ok(v)
}

fn is_int(v: f64) -> bool {
    v.fract() == 0.0
}

fn exposure_ok(v: f64) -> bool {
    v == 0.0 || v == 1.0
}

/// One primitive's evaluated parameters against its section of 4.5.1 (and 8.2.4-8.2.6).
fn check_primitive(code: u8, p: &[f64]) -> Result<(), EvalError> {
    let ok = match code {
        // exposure, diameter >= 0, centre, [rotation]
        1 => exposure_ok(p[0]) && p[1] >= 0.0,
        // exposure, width >= 0, start, end, rotation
        20 | 2 => exposure_ok(p[0]) && p[1] >= 0.0,
        // exposure, width >= 0, height >= 0, centre (21) or lower left (22), rotation
        21 | 22 => exposure_ok(p[0]) && p[1] >= 0.0 && p[2] >= 0.0,
        // exposure, n, n + 1 points, rotation: closed, 3 <= n <= 5000
        4 => {
            let n = p[1];
            if !(exposure_ok(p[0]) && is_int(n) && (3.0..=5000.0).contains(&n)) {
                false
            } else {
                let n = n as usize;
                p.len() == 2 * n + 5 && p[2] == p[2 + 2 * n] && p[3] == p[3 + 2 * n]
            }
        }
        // exposure, 3 <= n <= 12, centre, diameter >= 0, rotation
        5 => exposure_ok(p[0]) && is_int(p[1]) && (3.0..=12.0).contains(&p[1]) && p[4] >= 0.0,
        // centre, outer > inner >= 0, 0 <= gap < outer / sqrt(2), rotation
        7 => {
            let (outer, inner, gap) = (p[2], p[3], p[4]);
            outer > inner && inner >= 0.0 && gap >= 0.0 && 2.0 * gap * gap < outer * outer
        }
        // centre, outer diameter, ring thickness, gap, rings (an integer), crosshair
        // thickness and length: all >= 0
        6 => p[2..8].iter().all(|v| *v >= 0.0) && is_int(p[5]),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(EvalError::Primitive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(s: &str) -> MacroItem {
        match parse_item(s, 64) {
            Ok(p) => p.item,
            Err(e) => panic!("{s}: {e:?}"),
        }
    }

    #[test]
    fn expressions_follow_the_grammar() {
        assert!(parse_expr("-$5+$3", 64).is_ok());
        assert!(parse_expr("($2+100)x1.75", 64).is_ok());
        assert!(parse_expr("$1/2", 64).is_ok());
        assert!(parse_expr("2x-3", 64).is_err());
        assert!(parse_expr("$1X2", 64).is_err());
        assert!(parse_expr("$0", 64).is_err());
        assert!(parse_expr("", 64).is_err());
        assert!(matches!(
            parse_expr(&format!("{}1{}", "(".repeat(80), ")".repeat(80)), 64),
            Err(ExprError::TooDeep)
        ));
    }

    #[test]
    fn primitives_check_their_parameters() {
        let m = |body: &[&str]| Macro {
            name: "M".into(),
            body: body.iter().map(|s| item(s)).collect(),
            offset: 0,
        };
        let mut w = 1_000_000;
        // a closed triangle
        assert_eq!(
            evaluate(&m(&["4,1,3,1,-1,1,1,2,1,1,-1,30"]), &[], &mut w),
            Ok(())
        );
        // not closed
        assert_eq!(
            evaluate(&m(&["4,1,3,1,-1,1,1,2,1,1,-2,30"]), &[], &mut w),
            Err(EvalError::Primitive)
        );
        // a polygon with 13 vertices
        assert_eq!(
            evaluate(&m(&["5,1,13,0,0,1,0"]), &[], &mut w),
            Err(EvalError::Primitive)
        );
        // exposure from a variable, 2
        assert_eq!(
            evaluate(&m(&["1,$1,1,0,0"]), &[2.0], &mut w),
            Err(EvalError::Primitive)
        );
        // a variable the aperture sets, defined again
        assert_eq!(
            evaluate(&m(&["$1=2", "1,1,$1,0,0"]), &[1.0], &mut w),
            Err(EvalError::Redefined)
        );
        // division by zero
        assert_eq!(
            evaluate(&m(&["1,1,1/$2,0,0"]), &[], &mut w),
            Err(EvalError::Primitive)
        );
        // a thermal
        assert_eq!(
            evaluate(&m(&["7,0,0,0.95,0.75,0.175,0.0"]), &[], &mut w),
            Ok(())
        );
        assert!(matches!(item("0 a comment"), MacroItem::Comment(_)));
        assert!(parse_item("3,1,1", 64).is_err());
        assert!(parse_item("1,1,1", 64).is_err());
    }
}

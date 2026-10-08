//! Version numbers and requirements (PACKAGES.md §3). v1's grammar is
//! deliberately narrow — `=x.y.z`, `^x.y.z`, and bare `x.y.z` (caret)
//! are the only forms that parse; a partial parser must never look
//! like full semver support.

/// A strict `MAJOR.MINOR.PATCH` version.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Ver(pub u64, pub u64, pub u64);

impl std::fmt::Display for Ver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

fn split_ver(s: &str) -> Result<Vec<u64>, String> {
    if s.is_empty() {
        return Err("empty version".to_string());
    }
    let mut parts = Vec::new();
    for p in s.split('.') {
        if p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()) {
            return Err(format!("`{s}` is not a version — expected digits.digits[.digits]"));
        }
        parts.push(
            p.parse::<u64>()
                .map_err(|_| format!("`{s}`: component `{p}` overflows u64"))?,
        );
    }
    Ok(parts)
}

/// Manifest/index version literals — exactly `x.y.z`, no shorthand.
pub fn parse_ver(s: &str) -> Result<Ver, String> {
    let p = split_ver(s)?;
    if p.len() != 3 {
        return Err(format!("`{s}`: versions are exactly MAJOR.MINOR.PATCH"));
    }
    Ok(Ver(p[0], p[1], p[2]))
}

/// A version requirement — the two forms PACKAGES.md §3 admits.
#[derive(Clone, Debug)]
pub enum Req {
    /// `=x.y.z` — one version exactly.
    Exact(Ver),
    /// `^x.y.z` or a bare `x.y.z` — `>=v` up to the next breaking
    /// version (`^0.x` stays inside the minor, `^0.0.x` the patch).
    Caret(Ver),
}

impl Req {
    pub fn satisfies(&self, v: &Ver) -> bool {
        match self {
            Req::Exact(e) => v == e,
            Req::Caret(b) => {
                if v < b {
                    return false;
                }
                if b.0 > 0 {
                    v.0 == b.0
                } else if b.1 > 0 {
                    v.0 == 0 && v.1 == b.1
                } else {
                    *v == *b
                }
            }
        }
    }

    /// The requirement's source spelling, for diagnostics.
    pub fn display(&self) -> String {
        match self {
            Req::Exact(v) => format!("={v}"),
            Req::Caret(v) => format!("^{v}"),
        }
    }
}

/// `=x.y.z`, `^x.y.z`, or bare `x.y[.z]` (caret). Shorter bare forms
/// pad with zeros: `1.2` means `^1.2.0`. Anything else is an error —
/// callers that accept paths too must fall back on Err.
pub fn parse_req(s: &str) -> Result<Req, String> {
    if let Some(rest) = s.strip_prefix('=') {
        return Ok(Req::Exact(parse_ver(rest)?));
    }
    if let Some(rest) = s.strip_prefix('^') {
        let p = split_ver(rest)?;
        return Ok(Req::Caret(pad_ver(&p, rest)?));
    }
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        let p = split_ver(s)?;
        return Ok(Req::Caret(pad_ver(&p, s)?));
    }
    Err(format!("`{s}` is not a version requirement — expected `x.y.z`, `^x.y.z`, or `=x.y.z`"))
}

fn pad_ver(parts: &[u64], orig: &str) -> Result<Ver, String> {
    match parts.len() {
        1 => Ok(Ver(parts[0], 0, 0)),
        2 => Ok(Ver(parts[0], parts[1], 0)),
        3 => Ok(Ver(parts[0], parts[1], parts[2])),
        _ => Err(format!("`{orig}`: too many version components")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caret_bounds() {
        let ok = |r: &str, v: &str| {
            assert!(
                parse_req(r).unwrap().satisfies(&parse_ver(v).unwrap()),
                "{r} should satisfy {v}"
            );
        };
        let no = |r: &str, v: &str| {
            assert!(
                !parse_req(r).unwrap().satisfies(&parse_ver(v).unwrap()),
                "{r} should NOT satisfy {v}"
            );
        };
        ok("1.2.3", "1.9.9");
        ok("^1.2.3", "1.2.3");
        ok("1.2", "1.2.0");
        ok("1", "1.99.0");
        no("1.2.3", "1.2.2");
        no("1.2.3", "2.0.0");
        ok("0.2.3", "0.2.9");
        no("0.2.3", "0.3.0");
        no("0.2.3", "1.0.0");
        ok("0.0.3", "0.0.3");
        no("0.0.3", "0.0.4");
        ok("=1.4.0", "1.4.0");
        no("=1.4.0", "1.4.1");
    }

    #[test]
    fn rejections() {
        for bad in ["", "*", ">=1.0", "1.2.x", "v1.2.3", "1.2.3.4", "a.b.c", "1.-2.3"] {
            assert!(parse_req(bad).is_err(), "`{bad}` should not parse");
        }
        for bad in ["1.2", "1", "1.2.3.4"] {
            assert!(parse_ver(bad).is_err(), "version `{bad}` should not parse");
        }
    }
}

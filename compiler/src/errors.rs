//! N35C — the error-code registry. Codes are FAMILY-level, not
//! site-level: two diagnostics that report the same semantic failure
//! share a code no matter which checker site emitted them. That's
//! what makes the codes searchable and keeps ~20 codes honest instead
//! of 213 site annotations.
//!
//! `classify` maps a headline to its family. Classification runs on
//! the normalized message TEXT — the wording conventions of N35C
//! (`cannot <verb> `<place>`: <reason>`, `use of <state> `<place>``,
//! `expected <type>, found <type>`) are what make that safe.
//!
//! Families:
//!   E1xxx  authority — borrow/alias conflicts, capability misuse,
//!          concurrency holds (task/lock/atomic), channel authority
//!   E2xxx  affine — consume/use-after-consume, initialization
//!   E3xxx  types — mismatches, inference, unknown names/members,
//!          call-shape misuse
//!   E4xxx  control flow — match, loops, returns, task/channel shape
//!   E5xxx  modules — imports, namespace conflicts, visibility
//!   E6xxx  external boundary — FFI/ABI
//!   E7xxx  syntax — parse errors, malformed literals

pub struct ErrorFamily {
    pub code: &'static str,
    pub title: &'static str,
    /// Explanation for `ourc --explain` and generated `ERRORS.md`.
    pub doc: &'static str,
}

const FAMILIES: &[ErrorFamily] = &[
    ErrorFamily {
        code: "E1001",
        title: "borrow conflict",
        doc: "A place is read, written, or consumed while a borrow of \
it is still live. The borrow edge's birth site is labeled — either \
end the borrow first or keep the access read-only.",
    },
    ErrorFamily {
        code: "E1002",
        title: "concurrency hold",
        doc: "An unjoined task, a lock guard, or an atomic hold claims \
the place. `join` the task, `unlock` the guard, or route the access \
through the synchronized builtin that owns the hold.",
    },
    ErrorFamily {
        code: "E1003",
        title: "region escape",
        doc: "A reference or handle would outlive the storage it names — \
escaping a function frame, a `spawn` body, or a channel's lifetime. \
Restructure so the referent provably outlives the view.",
    },
    ErrorFamily {
        code: "E1004",
        title: "channel authority",
        doc: "Authority passed through a `send` is channel-held until a \
`recv` claims it — the sender cannot touch it in between. `recv` the \
value, or restructure so authority never transits.",
    },
    ErrorFamily {
        code: "E2001",
        title: "use after consume",
        doc: "The place's authority was moved or freed already — `free`, \
an `own` argument, a `let` move, or `send` all consume. Rebind the \
place, or use it before the consume.",
    },
    ErrorFamily {
        code: "E2002",
        title: "maybe-consumed",
        doc: "The place is consumed on some control-flow paths but not \
all — the checker cannot prove it's alive here. Rebind it on every \
path, or restructure so the consume is unconditional.",
    },
    ErrorFamily {
        code: "E2003",
        title: "uninitialized use",
        doc: "The place has no value on at least one path reaching this \
use. Initialize it before use, or on every branch.",
    },
    ErrorFamily {
        code: "E2004",
        title: "live authority overwrite",
        doc: "A store would silently discard live own authority (a `box` \
or a payload containing one) — the language has no hidden `free`. \
Extract the old value first (`let old = s.p`) or `free` it.",
    },
    ErrorFamily {
        code: "E3001",
        title: "type mismatch",
        doc: "The value's type doesn't match what the context requires. \
The language does no implicit numeric or reference coercion — cast \
explicitly.",
    },
    ErrorFamily {
        code: "E3002",
        title: "type inference failure",
        doc: "A generic parameter resolved to two different types, or \
nothing constrained it. Annotate the binding or instantiate \
explicitly (`f::<i32>(..)`).",
    },
    ErrorFamily {
        code: "E3003",
        title: "unknown name or member",
        doc: "The variable, function, struct, field, or enum variant \
doesn't exist in this scope. Check the spelling and the module path.",
    },
    ErrorFamily {
        code: "E3004",
        title: "call shape",
        doc: "The callee exists but the call doesn't fit it — wrong \
arity, a place required where a value was passed, or a builtin used \
with the wrong argument form.",
    },
    ErrorFamily {
        code: "E4001",
        title: "control flow",
        doc: "A `break`/`continue` outside a loop, a missing or extra \
`return` value, or a divergent statement where a value is required.",
    },
    ErrorFamily {
        code: "E4002",
        title: "match",
        doc: "The match is malformed — a duplicate arm, a payload bound \
where none exists, an inaccessible variant, or a scrutinee that \
isn't a place.",
    },
    ErrorFamily {
        code: "E4003",
        title: "task or channel shape",
        doc: "A `spawn`/`join`/`send`/`recv`/`close` used in a way the \
channel discipline forbids — an unbound spawn, an unknown join, or \
a `?` inside a task.",
    },
    ErrorFamily {
        code: "E5001",
        title: "module structure",
        doc: "The import can't be resolved or the namespace conflicts — \
a missing file, an import cycle, a `module` name that disagrees \
with its file, or a duplicate declaration.",
    },
    ErrorFamily {
        code: "E5002",
        title: "visibility",
        doc: "A `pub` item names a private type, or an outsider touched \
a private symbol. Make the type `pub`, or keep the boundary honest \
by not exposing it.",
    },
    ErrorFamily {
        code: "E6001",
        title: "FFI boundary",
        doc: "An `extern` signature uses a type the C ABI can't carry, \
or a raw-pointer capability is misused at the boundary.",
    },
    ErrorFamily {
        code: "E7001",
        title: "syntax error",
        doc: "The token stream doesn't parse — check punctuation, \
braces, and statement terminators.",
    },
    ErrorFamily {
        code: "E7002",
        title: "malformed literal",
        doc: "A literal is malformed or doesn't fit its type — bad \
digits, a bad escape, or an out-of-range value for the width.",
    },
];

/// The family for a diagnostic headline, or `None` when the message
/// doesn't match a known shape (renders plain `error:` — honest
/// fallback, never a fake code).
pub fn classify(msg: &str) -> Option<&'static ErrorFamily> {
    let m = msg;
    // E1 — authority. Most-specific reasons first: concurrency holds,
    // channel/transit, region escape, then plain borrow conflicts.
    if m.contains("unjoined task")
        || m.contains("races")
        || m.contains("atomically held")
        || m.contains("held by a lock")
        || m.contains("already locked")
        || m.contains("different mutex domain")
        || m.contains("under a lock conflicts")
        || m.contains("through read authority")
    {
        return Some(&FAMILIES[1]);
    }
    if m.contains("channel-held")
        || m.contains("sent through a channel")
        || m.contains("stale in transit")
        || m.contains("channel op")
    {
        return Some(&FAMILIES[3]);
    }
    if m.contains("escapes region")
        || m.contains("send-escape")
        || m.contains("does not outlive")
        || m.contains("cannot promote")
        || m.contains("stale handle")
        || m.contains("still carries a reference")
        || m.contains("cannot leave its scope")
        || m.contains("cannot cross a")
    {
        return Some(&FAMILIES[2]);
    }
    if m.contains("it is borrowed")
        || m.contains("mutably borrowed")
        || m.contains("already borrowed")
        || m.contains("live borrow")
        || m.contains("through a borrow")
        || m.contains("through a handle")
        || m.contains("derive write authority")
        || m.contains("&mut` through")
        || m.contains("write through `&`")
        || m.contains("write through `slice`")
        || m.contains("derive `slice mut`")
        || m.contains("read capability")
        || m.contains("moved authority")
        || m.contains("moved reference")
        || m.contains("referent is borrowed")
    {
        return Some(&FAMILIES[0]);
    }
    // E2 — affine / init.
    if m.contains("maybe-consumed") || m.contains("dead on another path") {
        return Some(&FAMILIES[5]);
    }
    if m.contains("after consume")
        || m.contains("double consume")
        || m.contains("already consumed")
    {
        return Some(&FAMILIES[4]);
    }
    if m.contains("uninitialized")
        || m.contains("possibly-uninitialized")
        || m.contains("needs a type annotation or an initializer")
    {
        return Some(&FAMILIES[6]);
    }
    if m.contains("cannot overwrite") || m.contains("still holds") {
        return Some(&FAMILIES[7]);
    }
    // Match before E3's broad patterns — a `match`-scoped headline
    // belongs to E4002 even when it uses "must be"/"found" wording.
    if m.starts_with("`match`")
        || m.contains("non-exhaustive")
        || m.contains("match arm")
        || m.contains("payload")
        || m.contains("active variant")
        || m.contains("binds no payload")
    {
        return Some(&FAMILIES[13]);
    }
    // E7 leading-token prefixes — parser/lexer headlines start with
    // `expected`/`unexpected`/`unterminated`; semantic "expected X,
    // found Y" wording is always embedded (`type mismatch: expected..`,
    // `argument N: expected..`), never a headline prefix.
    if m.starts_with("expected")
        || m.starts_with("unexpected")
        || m.starts_with("unterminated")
        || m.starts_with("newline in")
    {
        return Some(&FAMILIES[18]);
    }
    // E3 — types / names / call shape.
    if m.contains("inferred as both") || m.contains("cannot infer") {
        return Some(&FAMILIES[9]);
    }
    if m.starts_with("undefined")
        || m.starts_with("unknown ")
        || m.starts_with("no function")
        || m.starts_with("no struct")
        || m.starts_with("no enum")
        || m.contains("declares no variants")
        || m.starts_with("no field")
        || m.starts_with("no member")
        || m.starts_with("no associated function")
        || m.contains("has no variant")
        || m.starts_with("enum `") && m.contains("has no variant")
    {
        return Some(&FAMILIES[10]);
    }
    if m.contains("takes ") && m.contains("argument")
        || m.contains("requires a place")
        || m.contains("is not a place")
        || m.contains("must be a place")
        || m.contains("is ambiguous")
        || m.contains("must bind a fresh `let`")
        || m.contains("requires a variable")
        || m.contains("takes no arguments")
        || m.contains("exactly one argument")
        || m.contains("first argument must name a place")
        || m.contains("construct a variant")
        || m.contains("missing field")
    {
        return Some(&FAMILIES[11]);
    }
    if m.contains("found") && (m.contains("expected") || m.contains("requires")
        || m.contains("wants") || m.contains("takes") || m.contains("must be"))
        || m.contains("type mismatch")
        || m.contains("non-integers")
        || m.contains("width/sign mismatch")
        || m.contains("operands, found")
        || m.contains("index must be")
        || m.contains("bound must be")
        || m.contains("indexing non-array")
        || m.contains("field access on non-struct")
        || m.contains("cannot slice")
        || m.contains("recursive")
        || m.contains("initialized twice")
        || m.contains("not a task")
        || m.contains("must be an enum")
        || m.contains("must be bool")
        || m.contains("not atomic storage")
        || m.contains("not `*const`")
        || m.contains("void value")
        || m.contains("out of bounds")
        || m.contains("out of range")
        || m.contains("negative index")
        || m.contains("not element-indexed")
        || m.contains("not a declarable type")
    {
        return Some(&FAMILIES[8]);
    }
    if m.contains("`spawn`")
        || m.contains("join ")
        || m.contains("`join`")
        || m.contains("out of scope without `join`")
        || m.contains("`send`")
        || m.contains("`recv`")
        || m.contains("`close`")
        || m.contains("`channel`")
        || m.contains("guard")
        || m.contains("`lock`")
        || m.contains("`unlock`")
        || m.contains("channel endpoint")
        || m.contains("not a channel")
    {
        return Some(&FAMILIES[14]);
    }
    if m.contains("outside a loop")
        || m.contains("`return`")
        || m.contains("returning a value")
        || m.contains("unreachable")
    {
        return Some(&FAMILIES[12]);
    }
    // E5 — modules / visibility.
    if m.contains("private")
        || m.contains("exposes")
        || m.contains("is private to")
        || m.contains("is not `pub`")
    {
        return Some(&FAMILIES[16]);
    }
    if m.contains("import")
        || m.contains("declared, but the file")
        || m.contains("module")
        || m.contains("cycle")
        || m.contains("cyclic")
        || m.starts_with("no module")
        || m.starts_with("duplicate")
        || m.contains("field `{") && m.contains("in struct")
    {
        return Some(&FAMILIES[15]);
    }
    // E6 — FFI.
    if m.contains("non-FFI type")
        || m.contains("extern ")
        || m.contains("writable `*T`")
        || m.contains("*const")
    {
        return Some(&FAMILIES[17]);
    }
    // E7 — literal/value shape (leading-token prefixes classify above).
    if m.contains("does not fit")
        || m.contains("division by zero")
        || m.contains("constant expression")
        || m.contains("literal")
        || m.contains("digit")
        || m.contains("escape")
        || m.starts_with("misplaced")
    {
        return Some(&FAMILIES[19]);
    }
    None
}

/// `ourc --explain E1002` — the family's prose.
pub fn explain(code: &str) -> Option<&'static ErrorFamily> {
    FAMILIES.iter().find(|f| f.code.eq_ignore_ascii_case(code))
}

/// Generated `ERRORS.md` content — the registry IS the document,
/// so the doc can never drift from the codes.
pub fn markdown() -> String {
    let mut s = String::from("# Error index\n\n");
    s.push_str("Codes are family-level: every diagnostic reporting the same\n\
semantic failure shares a code. `ourc --explain <CODE>` prints the\n\
family's entry.\n\n");
    for f in FAMILIES {
        s.push_str(&format!("## {} — {}\n\n{}\n\n", f.code, f.title, f.doc));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_numbered() {
        let mut seen = std::collections::HashSet::new();
        for f in FAMILIES {
            assert!(seen.insert(f.code), "dup code {}", f.code);
            assert!(f.code.starts_with('E') && f.code.len() == 5);
        }
    }

    #[test]
    fn headline_families() {
        let cases = [
            ("cannot consume `m`: it is borrowed", "E1001"),
            ("cannot read `m`: it is mutably borrowed", "E1001"),
            ("cannot write to `m`: it is borrowed", "E1001"),
            ("cannot touch `m`: it is held by unjoined task `t`", "E1002"),
            ("use after consume `p`", "E2001"),
            ("double consume `p`", "E2001"),
            ("cannot consume `p`: it may be dead on another path", "E2002"),
            ("use of uninitialized `x`", "E2003"),
            ("type mismatch: expected i32, found bool", "E3001"),
            ("type argument `T` inferred as both `i32` and `str`", "E3002"),
            ("undefined variable `x`", "E3003"),
            ("no field `f` on struct `S`", "E3003"),
            ("`lock` takes exactly 1 argument", "E3004"),
            ("`break` outside a loop", "E4001"),
            ("duplicate match arm `Ok`", "E4002"),
            ("join of unknown or already-joined task `t`", "E4003"),
            ("`x` is private to its module", "E5002"),
            ("cannot resolve import — no file `x.lang`", "E5001"),
            ("extern `f` returns non-FFI type `S`", "E6001"),
            ("expected `;`, found `}`", "E7001"),
            ("integer literal 300 does not fit u8", "E7002"),
        ];
        for (msg, code) in cases {
            assert_eq!(classify(msg).map(|f| f.code), Some(code), "{msg}");
        }
    }
}

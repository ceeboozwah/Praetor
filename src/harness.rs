//! Corpus harness: parse per-scenario metadata and compare models against both
//! (a) their own recorded behaviour and (b) a model-independent safety oracle.
//!
//! Directive syntax (in `//` comments, anywhere in the file):
//!   // @desc   <one-line description of the scenario>
//!   // @class  <bug class being tested, e.g. use-after-free / region-escape>
//!   // @safety <accept|reject> [reason]   GROUND TRUTH, authored independent of
//!                                         any model. This is the oracle every
//!                                         model is scored against.
//!   // @expect <model> <accept|reject> [reason]   the model's *recorded* verdict
//!                                                  (a regression lock, not truth)
//!
//! The distinction matters: `@safety` is what a correct analysis *should* decide;
//! `@expect` is what a given model *currently* decides. Divergence between the two
//! is the experimental finding (an unsound miss, or an over-restrictive reject) —
//! and we never edit `@safety` to make a model look good.

use crate::check::CheckResult;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Accept,
    Reject,
}

impl Verdict {
    pub fn parse(s: &str) -> Option<Verdict> {
        match s {
            "accept" => Some(Verdict::Accept),
            "reject" => Some(Verdict::Reject),
            _ => None,
        }
    }

    pub fn of(result: &CheckResult) -> Verdict {
        if result.accepted() {
            Verdict::Accept
        } else {
            Verdict::Reject
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Accept => "accept",
            Verdict::Reject => "reject",
        }
    }
}

/// How a model's verdict relates to the safety oracle for one scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Oracle says safe, model accepts. Correct.
    TrueAccept,
    /// Oracle says unsafe, model rejects. Correct — the bug is caught.
    Caught,
    /// Oracle says unsafe, model accepts. UNSOUND — a real bug slips through.
    Unsound,
    /// Oracle says safe, model rejects. Over-restrictive — a false positive.
    OverRestrictive,
}

impl Class {
    pub fn classify(safety: Verdict, got: Verdict) -> Class {
        match (safety, got) {
            (Verdict::Accept, Verdict::Accept) => Class::TrueAccept,
            (Verdict::Reject, Verdict::Reject) => Class::Caught,
            (Verdict::Reject, Verdict::Accept) => Class::Unsound,
            (Verdict::Accept, Verdict::Reject) => Class::OverRestrictive,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Expectations {
    pub desc: Option<String>,
    pub class: Option<String>,
    pub safety: Option<Verdict>,
    pub safety_reason: Option<String>,
    /// model name -> (recorded verdict, optional reason)
    pub per_model: BTreeMap<String, (Verdict, Option<String>)>,
}

pub fn parse_expectations(source: &str) -> Expectations {
    let mut exp = Expectations::default();
    for line in source.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("//") else {
            continue;
        };
        let rest = rest.trim();
        if let Some(desc) = rest.strip_prefix("@desc") {
            exp.desc = Some(desc.trim().to_string());
        } else if let Some(class) = rest.strip_prefix("@class") {
            exp.class = Some(class.trim().to_string());
        } else if let Some(spec) = rest.strip_prefix("@safety") {
            let mut parts = spec.split_whitespace();
            if let Some(v) = parts.next().and_then(Verdict::parse) {
                exp.safety = Some(v);
                let reason: String = parts.collect::<Vec<_>>().join(" ");
                exp.safety_reason = (!reason.is_empty()).then_some(reason);
            }
        } else if let Some(spec) = rest.strip_prefix("@expect") {
            let mut parts = spec.split_whitespace();
            let (Some(model), Some(verdict)) = (parts.next(), parts.next()) else {
                continue;
            };
            if let Some(v) = Verdict::parse(verdict) {
                let reason: String = parts.collect::<Vec<_>>().join(" ");
                let reason = (!reason.is_empty()).then_some(reason);
                exp.per_model.insert(model.to_string(), (v, reason));
            }
        }
    }
    exp
}

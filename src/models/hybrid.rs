//! Prototype D — Region + Capability.
//!
//! The hypothesis under test (see project plan §44): memory safety decomposes
//! into two *orthogonal* questions that can be answered by separate, simple
//! mechanisms rather than one complex one —
//!
//!   REGION      — lifetime safety: WHERE does a value live, WHEN is it
//!                 destroyed, CAN a reference escape its scope?
//!   CAPABILITY  — access safety:   WHO may touch it, READ or WRITE, is access
//!                 EXCLUSIVE, may authority be TRANSFERRED?
//!
//!       Memory Safety  ?=  Lifetime Safety  +  Access Safety
//!
//! This prototype is deliberately the *union* of the two independent analyses:
//! a program is accepted iff BOTH the region and capability checkers accept it.
//! That is the cleanest possible expression of the "separable concerns"
//! hypothesis — and it lets the corpus reveal exactly where the sum of the two
//! is NOT enough.
//!
//! Known, deliberately-unpatched consequence: neither component tracks a move
//! timeline, so this prototype does NOT catch use-after-move / use-after-free /
//! double-free through manual `free`. Whether that is a genuine gap or a sign
//! that manual `free` and by-value moves simply should not exist in a
//! region+capability language is precisely the question the results should
//! provoke — not something to paper over by pre-tuning the checker.

use crate::ast::Program;
use crate::check::{Diagnostic, MemoryModel};
use crate::models::{affine::Affine, capability::Capability, ownership::Ownership, region::Region};

pub struct RegionCapability;

impl MemoryModel for RegionCapability {
    fn name(&self) -> &'static str {
        "region+capability"
    }

    fn tagline(&self) -> &'static str {
        "Prototype D: lifetime safety (region) + access safety (capability)"
    }

    fn profile(&self) -> crate::check::Profile {
        crate::check::Profile {
            // region {alive, dead} + capability {read, write, own}
            states: &[
                "region-alive",
                "region-dead",
                "read-cap",
                "write-cap",
                "own-cap",
            ],
            resource_states: &[],
            temporal_states: 0,
            lifetime_concepts: 1,
            move_rules: 1,
            rules: 3 + 4, // region + capability
            path_sensitive: false,
            machinery: &["edge-levels", "borrow-counts"],
        }
    }

    fn check(&self, program: &Program) -> Vec<Diagnostic> {
        let mut diags: Vec<Diagnostic> = Vec::new();
        diags.extend(Region.check(program));
        diags.extend(Capability.check(program));
        diags
    }
}

/// Prototype E — Region + Ownership.
///
/// The alternative to Prototype D: instead of pairing regions with a *rights*
/// model, pair them with a *move-timeline* model. The comparison against D is
/// the point — does adding ownership's temporal reasoning close the
/// use-after-free / move-after-use gap, and at what cost in state-space burden?
///
/// Also the union of two independent analyses.
pub struct RegionOwnership;

impl MemoryModel for RegionOwnership {
    fn name(&self) -> &'static str {
        "region+ownership"
    }

    fn tagline(&self) -> &'static str {
        "Prototype E: lifetime safety (region) + move/borrow safety (ownership)"
    }

    fn profile(&self) -> crate::check::Profile {
        crate::check::Profile {
            // region {alive, dead} + ownership's full move/borrow timeline
            states: &[
                "region-alive",
                "region-dead",
                "owned/live",
                "moved-out",
                "shared-borrowed",
                "mutably-borrowed",
                "borrow-expired",
                "consumed (freed)",
            ],
            resource_states: &[],
            temporal_states: 1,
            lifetime_concepts: 1,
            move_rules: 4,
            rules: 3 + 3, // region + ownership
            path_sensitive: false,
            machinery: &["edge-levels", "borrow-counts"],
        }
    }

    fn check(&self, program: &Program) -> Vec<Diagnostic> {
        let mut diags: Vec<Diagnostic> = Vec::new();
        diags.extend(Region.check(program));
        diags.extend(Ownership.check(program));
        diags
    }
}

/// Prototype F — Region + Capability + minimal Affine temporal state.
///
/// The sharpened hypothesis:
///
///     Memory Safety  =  Region Validity  +  Access Capability
///                      +  Minimal Temporal State
///
/// Temporal state is confined to *consumables*: values that are `free`d or
/// moved. Ordinary region memory never enters the temporal machine. The
/// checker tracks three states — Live, Consumed, and MaybeConsumed (the merge
/// of differing branch states) — and nothing else: no borrow counting, no
/// move-out timeline, no lifetime parameters.
///
/// The testable prediction: F should match E's temporal coverage (it shares
/// the use-after-free / double-free / move-after-use class) WITHOUT E's
/// over-restriction (scenario 23: ownership path-INsensitivity; affine merges
/// branches properly) and with a smaller state space. Its honest gaps, which
/// the corpus is designed to expose rather than hide:
///   * use-after-consume through a *field-stored* alias the checker can't see
///   * interprocedural consumption (callee frees a `ref` param) is prevented
///     rather than modelled — see `consume-through-alias`
pub struct RegionCapAffine;

impl MemoryModel for RegionCapAffine {
    fn name(&self) -> &'static str {
        "region+cap+affine"
    }

    fn tagline(&self) -> &'static str {
        "Prototype F: region validity + access capability + minimal affine state"
    }

    fn profile(&self) -> crate::check::Profile {
        crate::check::Profile {
            // Ordinary memory: region {alive,dead} + capability {read,write,own}.
            states: &[
                "region-alive",
                "region-dead",
                "read-cap",
                "write-cap",
                "own-cap",
            ],
            // Resources/consumables ADDITIONALLY carry the affine state — the
            // whole point: temporal state is not paid for by ordinary memory.
            resource_states: &["res-live", "res-consumed", "res-maybe"],
            temporal_states: 3,
            lifetime_concepts: 1,
            move_rules: 1,   // consume-on-move only — no borrow counting
            rules: 3 + 4 + 18,
            path_sensitive: true, // branch/loop state merging is the one cost
            machinery: &[
                "edge-levels",
                "borrow-counts",
                "path-merge",
                "alias-edge-graph",
                "definite-kills(scope|rebind|field|consume|call|join)",
                "effect-summaries",
                "scc-fixpoint",
                "capability-partition(holds)",
                "join-merge",
                "task-escape-check",
                "capability-edges(identity)",
                "stale-handle-tracking",
                "atomic-op-resolution",
                "synchronized-domains(guards)",
                "channel-packets(in-transit)",
            ],
        }
    }

    fn check(&self, program: &Program) -> Vec<Diagnostic> {
        let mut diags: Vec<Diagnostic> = Vec::new();
        diags.extend(Region.check(program));
        diags.extend(Capability.check(program));
        diags.extend(Affine.check(program));
        diags
    }
}

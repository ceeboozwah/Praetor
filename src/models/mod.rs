//! The competing memory models under evaluation.

pub mod affine;
pub mod capability;
pub mod hybrid;
pub mod ownership;
pub mod region;

use crate::check::MemoryModel;

/// All models registered in the lab, in display order.
///
/// `affine` is registered standalone as a control: it shows what the minimal
/// temporal state catches BY ITSELF (temporal bugs only), which makes the
/// hybrids' coverage attributable rather than muddled.
pub fn all() -> Vec<Box<dyn MemoryModel>> {
    vec![
        Box::new(ownership::Ownership),
        Box::new(region::Region),
        Box::new(capability::Capability),
        Box::new(affine::Affine),
        Box::new(hybrid::RegionCapability),
        Box::new(hybrid::RegionOwnership),
        Box::new(hybrid::RegionCapAffine),
    ]
}

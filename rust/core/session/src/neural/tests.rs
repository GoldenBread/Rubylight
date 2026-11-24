//! Pure unit tests for Neural Rendering archive policy, manifests,
//! OptiScaler.ini transforms, game/folder eligibility, and GPU policy.
use super::*;

mod archive_tests;
mod eligibility_tests;
mod gpu_tests;
mod ini_tests;
mod manifest_tests;

pub(super) fn hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

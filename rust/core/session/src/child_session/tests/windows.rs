//! Tests of Windows-only behaviour: drive letters, backslashes, `.exe` names.

use super::*;

#[test]
fn the_setup_record_is_one_for_the_pc() {
    let record = setup_record();
    assert!(record.ends_with(r"Butterpollo\windows-settings\child_session_setup.json"));
    assert!(!record.starts_with(crate::paths::installed_profile()));
}

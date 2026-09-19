// SPDX-License-Identifier: GPL-3.0-or-later
use crate::BodyFailure;
use nia_ice::Ice;

#[test]
fn checker_clones_share_the_first_internal_failure() {
    let failure = BodyFailure::new();
    let probe_failure = failure.clone();

    probe_failure.record(Ice::new("probe invariant failed"));
    failure.record(Ice::new("later invariant failed"));

    let error = failure.internal_error().expect("recorded internal failure");
    assert_eq!(error.message, "probe invariant failed");
}

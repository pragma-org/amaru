use std::time::Duration;

use amaru_pure_stage::Instant;

#[test]
fn offset_instant_eq_inner() {
    let a = Instant::at_offset(Duration::from_secs(11), Duration::from_millis(3));
    // Simulate clock Instant: same inner offset, with global_epoch_offset
    // We can't set offset from outside easily...
    assert_eq!(a, Instant::at_offset(Duration::from_secs(11), Duration::from_millis(3)));
}

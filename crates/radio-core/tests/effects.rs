//! The limiter. Task 1.9 of docs/rust-port.md.

use radio_core::mix::effects;

#[test]
fn a_limiter_leaves_a_quiet_piece_alone_and_keeps_a_loud_one_under_the_ceiling() {
    let quiet = vec![0.5f32, -0.5, 0.25, 0.1];
    assert_eq!(effects::limit(&quiet, 0.97), quiet);
    assert!(effects::limit(&[], 0.97).is_empty());
    let loud: Vec<f32> = (0..20000).map(|i| (i as f32 * 0.01).sin() * 3.0).collect();
    let limited = effects::limit(&loud, 0.97);
    assert_eq!(limited.len(), loud.len());
    assert!(limited.iter().all(|x| x.abs() <= 1.0));
}

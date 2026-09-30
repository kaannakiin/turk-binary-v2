use crate::service::hop_min_outs;

#[test]
fn an_operation_whose_threshold_rounds_to_zero_refuses_the_whole_route() {
    assert_eq!(
        hop_min_outs([(1, 2), (1_000, 1)].into_iter(), 50, 995),
        None
    );
    assert_eq!(
        hop_min_outs([(2, 2), (900, 1)].into_iter(), 50, 895),
        Some(vec![1, 895])
    );
}

#[test]
fn the_single_terminal_operation_is_raised_to_the_route_threshold() {
    assert_eq!(
        hop_min_outs([(2, 2), (900, 1)].into_iter(), 50, 900),
        Some(vec![1, 900])
    );
    assert_eq!(
        hop_min_outs([(600, 1), (400, 1)].into_iter(), 50, 995),
        Some(vec![597, 398])
    );
}

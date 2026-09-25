use serde::Deserialize;

use super::layout::Fees;
use super::math;
use crate::tests::sim_fixture;

#[derive(Deserialize)]
struct EventFixture {
    cases: Vec<EventCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EventCase {
    signature: String,
    quote_to_base: bool,
    base_reserve: String,
    quote_reserve: String,
    lp_fee_bps: u64,
    protocol_fee_bps: u64,
    creator_fee_bps: u64,
    amount_in: String,
    amount_out: String,
    lp_fee: String,
    protocol_fee: String,
    creator_fee: String,
}

/// Expected values are the program's own `BuyEvent` / `SellEvent` logs.
#[test]
fn every_recorded_swap_charges_the_component_fees_the_program_logged() {
    let text = sim_fixture("pump-swap-fee-events.json.gz");
    let fixture: EventFixture = serde_json::from_str(&text).expect("fixture parses");
    assert!(fixture.cases.iter().any(|c| c.quote_to_base));
    assert!(fixture.cases.iter().any(|c| !c.quote_to_base));
    let n = |s: &str| s.parse::<u128>().expect("numeric");
    for case in &fixture.cases {
        let fees = Fees {
            lp_fee_bps: case.lp_fee_bps,
            protocol_fee_bps: case.protocol_fee_bps,
            creator_fee_bps: case.creator_fee_bps,
        };
        let (base, quote, amount_in) = (
            n(&case.base_reserve),
            n(&case.quote_reserve),
            n(&case.amount_in),
        );
        let swap = if case.quote_to_base {
            math::buy_quote_input(amount_in, base, quote, &fees, true)
        } else {
            math::sell_base_input(amount_in, base, quote, quote, &fees, true)
        }
        .unwrap_or_else(|| panic!("{} declined", case.signature));
        assert_eq!(
            (
                swap.amount_out,
                swap.lp_fee,
                swap.protocol_fee,
                swap.creator_fee
            ),
            (
                n(&case.amount_out),
                n(&case.lp_fee),
                n(&case.protocol_fee),
                n(&case.creator_fee)
            ),
            "{}",
            case.signature
        );
    }
}

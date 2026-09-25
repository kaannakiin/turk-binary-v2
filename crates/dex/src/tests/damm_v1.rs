//! One live pool per curve and depeg type: constant product, plain stable,
//! Marinade, Lido and SPL stake pool. The expected sets come from an
//! independent Python port reading the same accounts.

use std::collections::BTreeSet;

use domain::chain::CLOCK_SYSVAR;
use domain::{DexKind, Pubkey};

use super::accounts::fixtures;
use crate::{Closure, ClosureError, NoAccounts, PoolAccount, Presence, closure};

pub(super) const CASES: &[(&str, &[&str])] = &[
    (
        "11Uvgm51udrCeXvbpsNzvWC75omr7uG4ETrwTsCaRbg",
        &[
            "2azWbP6fLe1hJzFZvkWTn9EM8j7iX96kAf8EeZdfg8iH",
            "So11111111111111111111111111111111111111112",
            "CgD1AVPUSz5ZzYhPRzuH9pJ9FBhrdbd4qsfCmLCYyUXC",
            "FERjPVNEa7Udq8CEv68h6tPL46Tq7ieE49HrE2wea3XT",
            "9UsN1hFSye1C1oNWX7DTV7YBT4H4KGjga2gBG6VoJmxa",
            "Cc3QVPSfBXCVkVgCKbNfyTWfaGyKegUhUsC1mfkgcvPP",
            "4btkECJguhqadVQ45M91LUAAQQsXu4hcsQwAipX8VrPP",
            "FZN7QZ8ZUUAxMPfxYEYkH3cXUASzH8EqA6B4tyCL8f1j",
            "62yYafNgZS1mmP1bFXB8byHqemknhwMefv9yGD8SLvc5",
            "HZeLxbZ9uHtSpwZC3LBr4Nubd14iHwz7bRSghRZf5VCG",
        ],
    ),
    (
        "2GyUx4EZgABV9Aoxn3Vrn1Y25wvwuafiGMfiWHGPWqxK",
        &[
            "78UckmXbjjectR4UdDt2FBCEJ3bJoBfyizRLdY1AUBpq",
            "coqRkaaKeUygDPhuS3mrmrj6DiHjeQJc2rFbT2YfxWn",
            "5sTfxc4UqkY3AJ8sW52pP4ZG3CEtS8LJGBAAFNufBNkh",
            "4LjKSppiBs4BmZdS3XhU4SGFASz8JgMXzBrTzegH61FC",
            "8Mx3jdizcuv119t1tcgDCzA4MQgjzPmm5Jew4JSjrxiZ",
            "28nKefqsXjKuLN6LTZmayouctrxUTfHouSZMhumVxKdJ",
            "7ReZTP9K5yRvtXQLyzPzTBaC2vTLo8nqExUKsdfnYfz",
            "5U6QNX8gieCsmjSRmZtQMNTAKQG2RiNTmSV5Yx3kJHmx",
            "J6nJb9W6L3CTMx4EAgi2Q37Rteu3N6xt1FjinusbeVhU",
            "HbxdJEpegoHVM61mun529PJYEmzBDiamRm1VeQG1fMwQ",
        ],
    ),
    (
        "HcjZvfeSNJbNkfLD4eEcRBr96AD3w1GpmMppaeRZf7ur",
        &[
            "So11111111111111111111111111111111111111112",
            "mSoLzYCxHdYgdzU16g5QSh3i5K3z3KZK7ytfqcJm7So",
            "FERjPVNEa7Udq8CEv68h6tPL46Tq7ieE49HrE2wea3XT",
            "8p1VKP45hhqq5iZG5fNGoi7ucme8nFLeChoDWNy7rWFm",
            "HWnWnLvBzvSmXH5hnHJCFmuQbDTsX3Ba2w9CPE5zf4YD",
            "GGG4DxkYa86g2v4KwtvR8Xu2tXEp1xd4BRC3yNnpve3g",
            "FZN7QZ8ZUUAxMPfxYEYkH3cXUASzH8EqA6B4tyCL8f1j",
            "21bR3D4QR4GzopVco44PVMBXwHFpSYrbrdeNwdKk7umb",
            "HZeLxbZ9uHtSpwZC3LBr4Nubd14iHwz7bRSghRZf5VCG",
            "3ifhD4Ywaa8aBZAaQSqYgN4Q1kaFArioLU8uumJMaqkE",
            "8szGkuLTAux9XMgZ2vtY39jVSowEcpBfFfD8hXSEqdGC",
        ],
    ),
    (
        "7EJSgV2pthhDfb4UiER9vzTqe2eojei9GEQAQnkqJ96e",
        &[
            "So11111111111111111111111111111111111111112",
            "7dHbWXmci3dT8UFYWYZweBLXgycu7Y3iL6trKn1Y7ARj",
            "FERjPVNEa7Udq8CEv68h6tPL46Tq7ieE49HrE2wea3XT",
            "CGY4XQq8U4VAJpbkaFPHZeXpW3o4KQ5LowVsn6hnMwKe",
            "BbRTJbmwBpmF8K6cbVNZLtCVzQczVdBoQS3aFZXxt1qB",
            "63sCov3VofCy7usVSzcdKYxzc6osp31pBY3YBT9XadVm",
            "FZN7QZ8ZUUAxMPfxYEYkH3cXUASzH8EqA6B4tyCL8f1j",
            "28KR3goEditLnzBZShRk2H7xvgzc176EoFwMogjdfSkn",
            "HZeLxbZ9uHtSpwZC3LBr4Nubd14iHwz7bRSghRZf5VCG",
            "3gkFA4uw3mc4Qi7ciKhwpDL7J6pc8xUrbUgUCsTBodkv",
            "49Yi1TKkNyYjPAFdR9LBvoHcUjuPX4Df5T5yv39w2XTn",
        ],
    ),
    (
        "5QeStYZuxprzniuhSKJ7wZJd4njk5DJ6AdGCwx4N3P9Y",
        &[
            "So11111111111111111111111111111111111111112",
            "LSTxxxnJzKDFSLr4dUkPcmCf5VyryEqzPLz5j4bpxFp",
            "FERjPVNEa7Udq8CEv68h6tPL46Tq7ieE49HrE2wea3XT",
            "DfB5LRqKZQNZF9rEki7bGzenHwdBidxnY2q7BWVBY2Es",
            "ArCT84ujiazyESck2emersSaa4zixJ6n2F22pQruxEUY",
            "CusxCgqtGgUKPkPU9ZpPP2JSLxCbZfpKD95LcujcfATw",
            "FZN7QZ8ZUUAxMPfxYEYkH3cXUASzH8EqA6B4tyCL8f1j",
            "7B34rQ8ifPf4KehxNt8cTfszYumt8JXEvCw4MWbCAFtK",
            "HZeLxbZ9uHtSpwZC3LBr4Nubd14iHwz7bRSghRZf5VCG",
            "3dSn24mGEPNZUMxAhEwDuiWoustqC5kEdtmaGGBNiY6o",
            "DqhH94PjkZsjAqEze2BEkWhFQJ6EyU6MdtMphMgnXqeK",
        ],
    ),
];

fn derive(pool: &str, view: &dyn crate::AccountView) -> Closure {
    let address = Pubkey::from_str_const(pool);
    let account = PoolAccount {
        address,
        data: fixtures().account(&address).unwrap().bytes(),
        mints: None,
    };
    closure(DexKind::MeteoraDammV1, &account, view).unwrap()
}

#[test]
fn closure_matches_an_independent_port_for_every_curve_and_depeg_type() {
    for (pool, deps) in CASES {
        let got: BTreeSet<Pubkey> = derive(pool, fixtures())
            .deps
            .iter()
            .map(|d| d.pubkey)
            .collect();
        let mut want: BTreeSet<Pubkey> = deps.iter().map(|k| Pubkey::from_str_const(k)).collect();
        want.extend([Pubkey::from_str_const(pool), CLOCK_SYSVAR]);
        assert_eq!(got, want, "{pool}");
    }
}

#[test]
fn closure_waits_for_both_vaults_and_completes_once_they_are_known() {
    for (pool, _) in CASES {
        let first = derive(pool, &NoAccounts);
        let second = derive(pool, fixtures());
        assert_eq!(
            (first.awaiting.len(), second.is_complete(), second.verified),
            (2, true, true),
            "{pool}"
        );
    }
}

#[test]
fn every_dependency_is_required_and_exists_with_an_accepted_owner() {
    for (pool, _) in CASES {
        for dep in derive(pool, fixtures()).deps {
            let account = fixtures()
                .account(&dep.pubkey)
                .unwrap_or_else(|| panic!("{pool} {:?} missing", dep.role));
            assert!(
                dep.presence == Presence::Required && dep.owner.accepts(&account.owner),
                "{pool} {:?}",
                dep.role
            );
        }
    }
}

#[test]
fn unknown_depeg_type_is_an_error_not_a_pool_without_stake() {
    let (pool, _) = CASES[2];
    let address = Pubkey::from_str_const(pool);
    let mut data = fixtures().account(&address).unwrap().bytes().to_vec();
    data[916] = 9;
    let account = PoolAccount {
        address,
        data: &data,
        mints: None,
    };
    assert_eq!(
        closure(DexKind::MeteoraDammV1, &account, fixtures()),
        Err(ClosureError::UnknownVariant {
            offset: 916,
            value: 9
        })
    );
}

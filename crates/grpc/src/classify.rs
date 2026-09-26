use yellowstone_grpc_proto::tonic::{Code, Status};

use domain::Pubkey;

use crate::events::LimitViolation;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    Transient,
    Limit(LimitViolation),
    Fatal(String),
}

// src: rpcpool/yellowstone-grpc@7139edd23c44470b4d260fabd5c270907014c270 yellowstone-grpc-geyser/src/grpc.rs (client_loop, subscribe)
// src: rpcpool/yellowstone-grpc@7139edd23c44470b4d260fabd5c270907014c270 yellowstone-grpc-geyser/src/plugin/filter/limits.rs (FilterLimitsCheckError)
const PUBKEY_LIMIT: &str = "Max amount of Pubkeys reached, only ";
const FILTER_LIMIT: &str = "Max amount of filters/data_slices reached, only ";
const PUBKEY_REJECTED: (&str, &str) = ("Pubkey ", " in filters is not allowed");

pub(crate) fn classify(status: &Status) -> Failure {
    let message = status.message();
    match status.code() {
        Code::InvalidArgument => limit(message).map_or_else(
            || Failure::Fatal(format!("server rejected the subscription: {message}")),
            Failure::Limit,
        ),
        Code::Unauthenticated | Code::PermissionDenied => {
            Failure::Fatal(format!("server refused the credentials: {message}"))
        }
        _ => Failure::Transient,
    }
}

fn limit(message: &str) -> Option<LimitViolation> {
    let allowed = |marker: &str| -> Option<usize> {
        let rest = &message[message.find(marker)? + marker.len()..];
        rest.split_whitespace().next()?.parse().ok()
    };
    allowed(PUBKEY_LIMIT)
        .map(|limit| LimitViolation::Pubkeys { limit })
        .or_else(|| allowed(FILTER_LIMIT).map(|limit| LimitViolation::Filters { limit }))
        .or_else(|| rejected(message).map(|pubkey| LimitViolation::PubkeyRejected { pubkey }))
}

fn rejected(message: &str) -> Option<Pubkey> {
    let (before, after) = PUBKEY_REJECTED;
    let end = message.find(after)?;
    let start = message[..end].rfind(before)? + before.len();
    message[start..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pubkey_limit_is_parsed_from_the_filter_error() {
        let status = Status::invalid_argument(
            "failed to create filter: Max amount of Pubkeys reached, only 10 allowed",
        );
        assert_eq!(
            classify(&status),
            Failure::Limit(LimitViolation::Pubkeys { limit: 10 })
        );
    }

    #[test]
    fn filter_limit_is_parsed_from_the_filter_error() {
        let status = Status::invalid_argument(
            "failed to create filter: Max amount of filters/data_slices reached, only 1 allowed",
        );
        assert_eq!(
            classify(&status),
            Failure::Limit(LimitViolation::Filters { limit: 1 })
        );
    }

    #[test]
    fn other_invalid_requests_are_fatal() {
        assert!(matches!(
            classify(&Status::invalid_argument(
                "failed to create filter: bad name"
            )),
            Failure::Fatal(_)
        ));
    }

    #[test]
    fn lagging_consumer_is_transient() {
        assert_eq!(
            classify(&Status::internal("lagged to receive geyser messages")),
            Failure::Transient
        );
    }

    #[test]
    fn bad_token_is_fatal() {
        assert!(matches!(
            classify(&Status::unauthenticated("No valid auth token")),
            Failure::Fatal(_)
        ));
    }
}

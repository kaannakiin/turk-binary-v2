use std::future::Future;
use std::time::Duration;

use domain::{AccountFilter, AccountUpdate, Commitment, Pubkey, RetryPolicy, Slot};
use serde::Deserialize;
use serde_json::json;
use solana_account_decoder_client_types::UiAccountEncoding;
use solana_commitment_config::CommitmentConfig;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_api::client_error::Error as ClientError;
use solana_rpc_client_api::config::{RpcAccountInfoConfig, RpcProgramAccountsConfig};
use solana_rpc_client_api::request::RpcRequest;
use solana_rpc_client_api::response::{OptionalContext, RpcKeyedAccount};
use tokio::sync::Semaphore;

use crate::RpcError;
use crate::convert::{account_update, commitment_config, rpc_filters};
use crate::error::{is_min_context_slot, is_transient, redact};
use crate::rate::RateLimiter;

pub const MAX_MULTIPLE_ACCOUNTS: usize = 100;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RpcSettings {
    pub commitment: Commitment,
    pub timeout_ms: u64,
    pub max_in_flight: usize,
    pub max_rps: u32,
    pub retry: RetryPolicy,
}

impl Default for RpcSettings {
    fn default() -> Self {
        Self {
            commitment: Commitment::Processed,
            timeout_ms: 10_000,
            max_in_flight: 8,
            max_rps: 8,
            retry: RetryPolicy::default(),
        }
    }
}

pub struct RpcGateway {
    client: RpcClient,
    commitment: CommitmentConfig,
    permits: Semaphore,
    rate: RateLimiter,
    retry: RetryPolicy,
}

impl RpcGateway {
    #[must_use]
    pub fn new(url: String, settings: &RpcSettings) -> Self {
        let commitment = commitment_config(settings.commitment);
        Self {
            client: RpcClient::new_with_timeout_and_commitment(
                url,
                Duration::from_millis(settings.timeout_ms),
                commitment,
            ),
            commitment,
            permits: Semaphore::new(settings.max_in_flight.max(1)),
            rate: RateLimiter::new(settings.max_rps),
            retry: settings.retry,
        }
    }

    pub async fn get_slot(&self) -> Result<Slot, RpcError> {
        self.call("getSlot", || {
            self.client.get_slot_with_commitment(self.commitment)
        })
        .await
        .map(Slot)
    }

    /// Result is aligned with `pubkeys`. Chunks after the first are pinned to
    /// the first chunk's slot via `min_context_slot`, so the snapshot never
    /// goes backwards in time across chunks.
    pub async fn get_multiple_accounts(
        &self,
        pubkeys: &[Pubkey],
    ) -> Result<Vec<Option<AccountUpdate>>, RpcError> {
        self.fetch_multiple(pubkeys, self.commitment).await
    }

    pub async fn get_multiple_accounts_at(
        &self,
        pubkeys: &[Pubkey],
        commitment: Commitment,
    ) -> Result<Vec<Option<AccountUpdate>>, RpcError> {
        self.fetch_multiple(pubkeys, commitment_config(commitment))
            .await
    }

    async fn fetch_multiple(
        &self,
        pubkeys: &[Pubkey],
        commitment: CommitmentConfig,
    ) -> Result<Vec<Option<AccountUpdate>>, RpcError> {
        const METHOD: &str = "getMultipleAccounts";
        let mut out = Vec::with_capacity(pubkeys.len());
        let mut min_context_slot = None;
        for chunk in pubkeys.chunks(MAX_MULTIPLE_ACCOUNTS) {
            let config = account_config(commitment, min_context_slot);
            let response = self
                .call(METHOD, || {
                    self.client
                        .get_multiple_ui_accounts_with_config(chunk, config.clone())
                })
                .await?;
            let slot = Slot(response.context.slot);
            min_context_slot.get_or_insert(slot.0);
            for (pubkey, account) in chunk.iter().zip(response.value) {
                out.push(
                    account
                        .map(|a| account_update(METHOD, *pubkey, &a, slot))
                        .transpose()?,
                );
            }
        }
        Ok(out)
    }

    /// One request of at most [`MAX_MULTIPLE_ACCOUNTS`] keys, answered at a
    /// context slot no older than `min_context_slot`. The slot is returned
    /// because it also dates the accounts that came back absent.
    pub async fn get_multiple_accounts_with(
        &self,
        pubkeys: &[Pubkey],
        commitment: Commitment,
        min_context_slot: Option<Slot>,
    ) -> Result<(Slot, Vec<Option<AccountUpdate>>), RpcError> {
        const METHOD: &str = "getMultipleAccounts";
        if pubkeys.len() > MAX_MULTIPLE_ACCOUNTS {
            return Err(RpcError::TooManyKeys {
                method: METHOD,
                keys: pubkeys.len(),
            });
        }
        let config = account_config(commitment_config(commitment), min_context_slot.map(|s| s.0));
        let response = self
            .call(METHOD, || {
                self.client
                    .get_multiple_ui_accounts_with_config(pubkeys, config.clone())
            })
            .await?;
        let slot = Slot(response.context.slot);
        check_response(
            METHOD,
            pubkeys.len(),
            response.value.len(),
            slot,
            min_context_slot,
        )?;
        let accounts = pubkeys
            .iter()
            .zip(response.value)
            .map(|(pubkey, account)| {
                account
                    .map(|a| account_update(METHOD, *pubkey, &a, slot))
                    .transpose()
            })
            .collect::<Result<_, _>>()?;
        Ok((slot, accounts))
    }

    pub async fn get_program_accounts(
        &self,
        filter: &AccountFilter,
    ) -> Result<Vec<AccountUpdate>, RpcError> {
        const METHOD: &str = "getProgramAccounts";
        let config = RpcProgramAccountsConfig {
            filters: Some(rpc_filters(filter)),
            account_config: account_config(self.commitment, None),
            with_context: Some(true),
            sort_results: None,
        };
        let params = json!([filter.owner.to_string(), config]);
        let response: OptionalContext<Vec<RpcKeyedAccount>> = self
            .call(METHOD, || {
                self.client
                    .send(RpcRequest::GetProgramAccounts, params.clone())
            })
            .await?;
        let OptionalContext::Context(response) = response else {
            return Err(RpcError::MissingContext { method: METHOD });
        };
        let slot = Slot(response.context.slot);
        response
            .value
            .iter()
            .map(|keyed| {
                let pubkey = keyed.pubkey.parse().map_err(|_| RpcError::Decode {
                    method: METHOD,
                    pubkey: keyed.pubkey.clone(),
                    reason: "pubkey is not valid base58",
                })?;
                account_update(METHOD, pubkey, &keyed.account, slot)
            })
            .collect()
    }

    async fn call<T, F, Fut>(&self, method: &'static str, mut request: F) -> Result<T, RpcError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, ClientError>>,
    {
        let max_attempts = self.retry.max_attempts.max(1);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let result = {
                let _permit = self
                    .permits
                    .acquire()
                    .await
                    .expect("gateway never closes its semaphore");
                self.rate.wait().await;
                request().await.map_err(redact)
            };
            match result {
                Ok(value) => return Ok(value),
                Err(err) if !is_transient(&err) => {
                    return Err(RpcError::Rejected {
                        method,
                        source: Box::new(err),
                    });
                }
                Err(err) if attempt >= max_attempts => {
                    return Err(RpcError::Exhausted {
                        method,
                        attempts: attempt,
                        source: Box::new(err),
                    });
                }
                Err(err) => {
                    let delay = self.retry.delay(attempt);
                    // Seeds ask for a slot the node may not have reached yet;
                    // waiting for it is the expected path, not a fault.
                    if is_min_context_slot(&err) {
                        tracing::debug!(
                            method,
                            attempt,
                            ?delay,
                            "rpc node behind the requested slot, retrying"
                        );
                    } else {
                        tracing::warn!(method, attempt, ?delay, error = %err, "transient rpc error, retrying");
                    }
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
}

fn check_response(
    method: &'static str,
    requested: usize,
    returned: usize,
    slot: Slot,
    min_context_slot: Option<Slot>,
) -> Result<(), RpcError> {
    if returned != requested {
        return Err(RpcError::Shape {
            method,
            reason: format!("{returned} accounts for {requested} keys"),
        });
    }
    if let Some(min) = min_context_slot.filter(|min| slot < *min) {
        return Err(RpcError::Shape {
            method,
            reason: format!(
                "context slot {} is below the requested minimum {}",
                slot.0, min.0
            ),
        });
    }
    Ok(())
}

const fn account_config(
    commitment: CommitmentConfig,
    min_context_slot: Option<u64>,
) -> RpcAccountInfoConfig {
    RpcAccountInfoConfig {
        encoding: Some(UiAccountEncoding::Base64Zstd),
        commitment: Some(commitment),
        data_slice: None,
        min_context_slot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_with_fewer_accounts_than_keys_is_rejected() {
        let err = check_response("m", 3, 2, Slot(10), None).unwrap_err();
        assert!(matches!(err, RpcError::Shape { .. }));
    }

    #[test]
    fn response_older_than_the_minimum_slot_is_rejected() {
        let err = check_response("m", 1, 1, Slot(9), Some(Slot(10))).unwrap_err();
        assert!(matches!(err, RpcError::Shape { .. }));
    }

    #[test]
    fn response_at_the_minimum_slot_is_accepted() {
        assert!(check_response("m", 1, 1, Slot(10), Some(Slot(10))).is_ok());
    }
}

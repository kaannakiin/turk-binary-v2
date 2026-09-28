use dex::{Dependency, Presence, Role};
use grpc::StreamId;

use crate::store::StoredAccount;
use crate::sync::KeyState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    NotReady(Reason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The closure is not fully verified against the program (see `TODO(verify)`).
    Unverified,
    Invalid,
    /// Waiting for accounts whose contents extend the closure.
    Awaiting,
    StreamDown(StreamId),
    /// The provider's filter limits cannot fit the pool's subscription.
    Unsubscribable,
    Syncing,
    Missing(Role),
    OwnerMismatch(Role),
    Closed,
}

impl Reason {
    /// Only a change to the pool's own accounts or the code clears these;
    /// the rest clear as streams and seeds catch up.
    #[must_use]
    pub const fn is_permanent(self) -> bool {
        matches!(
            self,
            Self::Unverified | Self::Invalid | Self::Unsubscribable | Self::Closed
        )
    }
}

pub(crate) struct Inputs<'a> {
    pub deps: &'a [Dependency],
    pub accounts: &'a [Option<StoredAccount>],
    pub states: &'a [Option<KeyState>],
    pub verified: bool,
    pub awaiting: bool,
    pub down: Option<StreamId>,
    pub rejected: bool,
}

/// Checks run from the most to the least structural cause, so the reported
/// reason is the one to fix first.
pub(crate) fn evaluate(inputs: &Inputs<'_>) -> Readiness {
    use Readiness::NotReady;
    let pool_closed = inputs
        .deps
        .iter()
        .zip(inputs.accounts)
        .any(|(d, a)| d.role == Role::Pool && a.as_ref().is_some_and(|a| !a.exists()));
    if pool_closed {
        return NotReady(Reason::Closed);
    }
    if inputs.rejected {
        return NotReady(Reason::Unsubscribable);
    }
    if !inputs.verified {
        return NotReady(Reason::Unverified);
    }
    if let Some(stream) = inputs.down {
        return NotReady(Reason::StreamDown(stream));
    }
    if inputs.awaiting {
        return NotReady(Reason::Awaiting);
    }
    if inputs.states.iter().any(|s| *s != Some(KeyState::Live)) {
        return NotReady(Reason::Syncing);
    }
    for (dep, account) in inputs.deps.iter().zip(inputs.accounts) {
        match account.as_ref().filter(|a| a.exists()) {
            None if dep.presence == Presence::Required => {
                return NotReady(Reason::Missing(dep.role));
            }
            Some(a) if !dep.owner.accepts(&a.owner) => {
                return NotReady(Reason::OwnerMismatch(dep.role));
            }
            _ => {}
        }
    }
    Readiness::Ready
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use dex::{OwnerRule, Scope};
    use domain::{Pubkey, Slot, UpdateOrder, WriteVersion};

    use super::*;

    const OWNER: Pubkey = Pubkey::new_from_array([7; 32]);

    fn dep(role: Role) -> Dependency {
        Dependency::new(
            Pubkey::new_unique(),
            role,
            Scope::Pool,
            OwnerRule::Program(OWNER),
        )
    }

    fn account(owner: Pubkey, lamports: u64) -> Option<StoredAccount> {
        stored(owner, lamports).into()
    }

    fn stored(owner: Pubkey, lamports: u64) -> StoredAccount {
        StoredAccount {
            owner,
            lamports,
            data: Bytes::new(),
            order: UpdateOrder {
                slot: Slot(1),
                write_version: WriteVersion(0),
            },
        }
    }

    fn check(
        deps: &[Dependency],
        accounts: &[Option<StoredAccount>],
        states: &[Option<KeyState>],
    ) -> Readiness {
        evaluate(&Inputs {
            deps,
            accounts,
            states,
            verified: true,
            awaiting: false,
            down: None,
            rejected: false,
        })
    }

    #[test]
    fn a_live_complete_closure_is_ready() {
        let deps = [dep(Role::Pool), dep(Role::AmmConfig)];
        let live = [Some(KeyState::Live); 2];
        assert_eq!(
            check(&deps, &[account(OWNER, 1), account(OWNER, 1)], &live),
            Readiness::Ready
        );
    }

    #[test]
    fn a_confirmed_absent_optional_array_does_not_block() {
        let deps = [
            dep(Role::Pool),
            dep(Role::TickArray { start: 0 }).optional(),
        ];
        let live = [Some(KeyState::Live); 2];
        assert_eq!(
            check(&deps, &[account(OWNER, 1), account(OWNER, 0)], &live),
            Readiness::Ready
        );
    }

    #[test]
    fn a_missing_required_account_blocks_with_its_role() {
        let deps = [dep(Role::Pool), dep(Role::AmmConfig)];
        let live = [Some(KeyState::Live); 2];
        assert_eq!(
            check(&deps, &[account(OWNER, 1), None], &live),
            Readiness::NotReady(Reason::Missing(Role::AmmConfig))
        );
    }

    #[test]
    fn an_account_still_seeding_blocks() {
        let deps = [dep(Role::Pool)];
        let seeding = [Some(KeyState::Seeding { barrier: Slot(1) })];
        assert_eq!(
            check(&deps, &[account(OWNER, 1)], &seeding),
            Readiness::NotReady(Reason::Syncing)
        );
    }

    #[test]
    fn a_wrong_owner_blocks() {
        let deps = [dep(Role::Pool)];
        assert_eq!(
            check(
                &deps,
                &[account(Pubkey::new_unique(), 1)],
                &[Some(KeyState::Live)]
            ),
            Readiness::NotReady(Reason::OwnerMismatch(Role::Pool))
        );
    }

    #[test]
    fn a_closed_pool_is_reported_before_anything_else() {
        let deps = [dep(Role::Pool)];
        let result = evaluate(&Inputs {
            deps: &deps,
            accounts: &[account(OWNER, 0)],
            states: &[None],
            verified: false,
            awaiting: true,
            down: Some(StreamId::Shared(0)),
            rejected: true,
        });
        assert_eq!(result, Readiness::NotReady(Reason::Closed));
    }
}

// Copyright © Aptos Foundation
// Parts of the project are originally copyright © Meta Platforms, Inc.
// SPDX-License-Identifier: Apache-2.0

pub mod coin_deposit;
pub mod coin_register;
pub mod coin_register_event;
pub mod coin_withdraw;
pub mod deposit_event;
pub mod fungible_asset;
pub mod key_rotation;
pub mod key_rotation_event;
pub mod new_block;
pub mod new_epoch;
pub mod withdraw_event;

pub use coin_deposit::*;
pub use coin_register::*;
pub use coin_register_event::*;
pub use coin_withdraw::*;
pub use deposit_event::*;
pub use fungible_asset::*;
pub use key_rotation::*;
pub use key_rotation_event::*;
pub use new_block::*;
pub use new_epoch::*;
pub use withdraw_event::*;

pub fn is_aptos_governance_create_proposal_event(event_type: &str) -> bool {
    event_type == "0x1::aptos_governance::CreateProposal"
        || event_type == "0x1::aptos_governance::CreateProposalEvent"
}

// Copyright (c) Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use crate::{cached_state_view::ShardedStateCache, metrics::TIMER, state_delta::StateDelta};
use aptos_metrics_core::TimerHelper;
use aptos_types::{
    state_store::{state_key::StateKey, state_value::StateValue},
    transaction::{Transaction, TransactionInfo, TransactionOutput, Version},
};
use std::collections::HashMap;

#[derive(Clone)]
pub struct ChunkToCommit<'a> {
    pub first_version: Version,
    pub last_state_checkpoint_index: Option<usize>,
    pub transactions: &'a [Transaction],
    pub transaction_outputs: &'a [TransactionOutput],
    pub transaction_infos: &'a [TransactionInfo],
    pub base_state_version: Option<Version>,
    pub latest_in_memory_state: &'a StateDelta,
    pub sharded_state_cache: Option<&'a ShardedStateCache>,
    pub is_reconfig: bool,
}

impl<'a> ChunkToCommit<'a> {
    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn next_version(&self) -> Version {
        self.first_version + self.len() as Version
    }

    pub fn expect_last_version(&self) -> Version {
        self.next_version() - 1
    }

    pub fn collect_updates_until_last_state_checkpoint(
        &self,
    ) -> Option<HashMap<StateKey, Option<StateValue>>> {
        let _timer = TIMER.timer_with(&["collect_updates_until_last_state_checkpoint"]);

        self.last_state_checkpoint_index.map(|idx| {
            self.transaction_outputs[0..=idx]
                .iter()
                .flat_map(TransactionOutput::state_update_refs)
                .collect::<HashMap<_, _>>()
                .into_iter()
                .map(|(k, v)| (k.clone(), v.cloned()))
                .collect()
        })
    }
}

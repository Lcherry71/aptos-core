// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use crate::metrics::OTHER_TIMERS;
use anyhow::{ensure, Result};
use aptos_crypto::{hash::CryptoHash, HashValue};
use aptos_executor_types::{
    execution_output::ExecutionOutput, state_checkpoint_output::StateCheckpointOutput,
    transactions_with_output::TransactionsWithOutput, ProofReader,
};
use aptos_logger::info;
use aptos_metrics_core::TimerHelper;
use aptos_scratchpad::FrozenSparseMerkleTree;
use aptos_storage_interface::{
    cached_state_view::{ShardedStateCache, StateCache},
    state_delta::StateDelta,
};
use aptos_types::{
    state_store::{
        state_key::StateKey, state_storage_usage::StateStorageUsage, state_value::StateValue,
    },
    transaction::{TransactionOutput, Version},
    write_set::WriteSet,
};
use dashmap::DashMap;
use itertools::Itertools;
use std::{collections::HashMap, ops::Deref, sync::Arc};

/// Helper class for calculating state changes after a block of transactions are executed.
pub struct InMemoryStateCalculatorV2 {}

impl InMemoryStateCalculatorV2 {
    pub fn calculate_for_transactions(
        execution_output: &ExecutionOutput,
        parent_state: &Arc<StateDelta>,
        known_state_checkpoints: Option<impl IntoIterator<Item = Option<HashValue>>>,
    ) -> Result<StateCheckpointOutput> {
        if execution_output.is_block {
            Self::validate_input_for_block(parent_state, &execution_output.to_commit)?;
        }

        Self::calculate_impl(
            execution_output.num_transactions_to_commit(),
            parent_state,
            &execution_output.state_cache,
            execution_output
                .to_commit
                .transaction_outputs
                .iter()
                .map(TransactionOutput::write_set),
            execution_output.last_state_checkpoint_index,
            execution_output.is_block,
            known_state_checkpoints,
        )
    }

    pub fn calculate_for_write_sets_after_snapshot(
        parent_state: &Arc<StateDelta>,
        state_cache: &StateCache,
        last_checkpoint_index: Option<usize>,
        write_sets: &[WriteSet],
    ) -> Result<StateCheckpointOutput> {
        Self::calculate_impl(
            write_sets.len(),
            parent_state,
            state_cache,
            write_sets,
            last_checkpoint_index,
            false,
            Option::<Vec<_>>::None,
        )
    }

    fn calculate_impl<'a>(
        num_txns: usize,
        parent_state: &Arc<StateDelta>,
        state_cache: &StateCache,
        write_sets: impl IntoIterator<Item = &'a WriteSet>,
        last_checkpoint_index: Option<usize>,
        is_block: bool,
        known_state_checkpoints: Option<impl IntoIterator<Item = Option<HashValue>>>,
    ) -> Result<StateCheckpointOutput> {
        let StateCache {
            // This makes sure all in-mem nodes seen while proofs were fetched stays in mem during the
            // calculation
            frozen_base,
            sharded_state_cache,
            proofs,
        } = state_cache;
        assert!(frozen_base.smt.is_the_same(&parent_state.current));

        let write_sets = write_sets.into_iter().collect_vec();
        let (updates_before_last_checkpoint, updates_after_last_checkpoint) =
            if let Some(index) = last_checkpoint_index {
                (
                    Self::calculate_updates(&write_sets[0..=index]),
                    Self::calculate_updates(&write_sets[index + 1..]),
                )
            } else {
                (HashMap::new(), Self::calculate_updates(&write_sets))
            };
        let all_updates = {
            let _timer = OTHER_TIMERS.timer_with(&["combine_two_updates"]);
            let mut all = updates_before_last_checkpoint.clone();
            all.extend(updates_after_last_checkpoint.clone());
            all
        };

        let usage = Self::calculate_usage(
            parent_state.current.usage(),
            sharded_state_cache,
            &all_updates,
        );

        let first_version = parent_state.current_version.map_or(0, |v| v + 1);
        let proof_reader = ProofReader::new(proofs);
        let latest_checkpoint = if let Some(index) = last_checkpoint_index {
            Self::make_checkpoint(
                parent_state.current.freeze(&frozen_base.base_smt),
                &updates_before_last_checkpoint,
                if index == num_txns - 1 {
                    usage
                } else {
                    StateStorageUsage::new_untracked()
                },
                &proof_reader,
            )?
        } else {
            // If there is no checkpoint in this chunk, the latest checkpoint will be the existing
            // one.
            parent_state.base.freeze(&frozen_base.base_smt)
        };

        let mut latest_checkpoint_version = parent_state.base_version;
        let mut state_checkpoint_hashes = known_state_checkpoints
            .map_or_else(|| vec![None; num_txns], |v| v.into_iter().collect());
        ensure!(
            state_checkpoint_hashes.len() == num_txns,
            "Bad number of known hashes."
        );
        if let Some(index) = last_checkpoint_index {
            if let Some(h) = state_checkpoint_hashes[index] {
                ensure!(
                    h == latest_checkpoint.root_hash(),
                    "Last checkpoint not expected."
                );
            } else {
                state_checkpoint_hashes[index] = Some(latest_checkpoint.root_hash());
            }
            latest_checkpoint_version = Some(first_version + index as u64);
        }

        let current_version = first_version + num_txns as u64 - 1;
        // We need to calculate the SMT at the end of the chunk, if it is not already calculated.
        let current_tree = if last_checkpoint_index == Some(num_txns - 1) {
            latest_checkpoint.smt.clone()
        } else {
            ensure!(!is_block, "Block must have the checkpoint at the end.");
            // The latest tree is either the last checkpoint in current chunk, or the tree at the
            // end of previous chunk if there is no checkpoint in the current chunk.
            let latest_tree = if last_checkpoint_index.is_some() {
                latest_checkpoint.clone()
            } else {
                parent_state.current.freeze(&frozen_base.base_smt)
            };
            Self::make_checkpoint(
                latest_tree,
                &updates_after_last_checkpoint,
                usage,
                &proof_reader,
            )?
            .smt
        };

        let updates_after_last_checkpoint = Self::clone_updates(&updates_after_last_checkpoint);

        let updates_since_latest_checkpoint = {
            if last_checkpoint_index.is_some() {
                updates_after_last_checkpoint
            } else {
                let mut ret = parent_state.updates_since_base.deref().deref().clone();
                ret.extend(updates_after_last_checkpoint);
                ret
            }
        };

        info!(
            "last_checkpoint_index {last_checkpoint_index:?}, result_state: {latest_checkpoint_version:?} {:?} {:?} {current_version} {:?} {:?}",
            latest_checkpoint.root_hash(),
            latest_checkpoint.usage(),
            current_tree.root_hash(),
            current_tree.usage(),
        );

        let result_state = StateDelta::new(
            latest_checkpoint.smt,
            latest_checkpoint_version,
            current_tree,
            Some(current_version),
            updates_since_latest_checkpoint,
        );

        Ok(StateCheckpointOutput::new(
            parent_state.clone(),
            Arc::new(result_state),
            state_checkpoint_hashes,
        ))
    }

    fn calculate_updates<'a>(
        write_sets: &[&'a WriteSet],
    ) -> HashMap<&'a StateKey, Option<&'a StateValue>> {
        let _timer = OTHER_TIMERS.timer_with(&["calculate_updates"]);
        write_sets
            .iter()
            .flat_map(|w| w.state_update_refs())
            .collect()
    }

    fn clone_updates(
        updates: &HashMap<&StateKey, Option<&StateValue>>,
    ) -> HashMap<StateKey, Option<StateValue>> {
        let _timer = OTHER_TIMERS.timer_with(&["clone_updates"]);
        updates
            .iter()
            .map(|(k, v)| ((*k).clone(), v.cloned()))
            .collect()
    }

    fn add_to_delta(
        k: &StateKey,
        v: &Option<&StateValue>,
        state_cache: &DashMap<StateKey, (Option<Version>, Option<StateValue>)>,
        items_delta: &mut i64,
        bytes_delta: &mut i64,
    ) {
        let key_size = k.size();
        if let Some(value) = v {
            *items_delta += 1;
            *bytes_delta += (key_size + value.size()) as i64;
        }
        if let Some(old_entry) = state_cache.get(k) {
            if let (_, Some(old_v)) = old_entry.value() {
                *items_delta -= 1;
                *bytes_delta -= (key_size + old_v.size()) as i64;
            }
        }
    }

    fn calculate_usage(
        old_usage: StateStorageUsage,
        sharded_state_cache: &ShardedStateCache,
        updates: &HashMap<&StateKey, Option<&StateValue>>,
    ) -> StateStorageUsage {
        let _timer = OTHER_TIMERS
            .with_label_values(&["calculate_usage"])
            .start_timer();
        if old_usage.is_untracked() {
            return StateStorageUsage::new_untracked();
        }

        let mut items_delta = 0i64;
        let mut bytes_delta = 0i64;
        for (key, value) in updates {
            Self::add_to_delta(
                key,
                value,
                sharded_state_cache.shard(key.get_shard_id()),
                &mut items_delta,
                &mut bytes_delta,
            );
        }

        StateStorageUsage::new(
            (old_usage.items() as i64 + items_delta) as usize,
            (old_usage.bytes() as i64 + bytes_delta) as usize,
        )
    }

    fn make_checkpoint(
        latest_checkpoint: FrozenSparseMerkleTree<StateValue>,
        updates: &HashMap<&StateKey, Option<&StateValue>>,
        usage: StateStorageUsage,
        proof_reader: &ProofReader,
    ) -> Result<FrozenSparseMerkleTree<StateValue>> {
        let _timer = OTHER_TIMERS.timer_with(&["make_checkpoint"]);

        // Update SMT.
        //
        // TODO(aldenhu): avoid collecting into vec
        let smt_updates = {
            let _timer = OTHER_TIMERS.timer_with(&["make_smt_updates"]);
            updates.iter().map(|(key, value)| (key.hash(), *value))
        };
        let new_checkpoint = {
            let _timer = OTHER_TIMERS.timer_with(&["smt_batch_update"]);
            latest_checkpoint.batch_update(smt_updates, usage, proof_reader)?
        };
        Ok(new_checkpoint)
    }

    fn validate_input_for_block(
        base: &StateDelta,
        to_commit: &TransactionsWithOutput,
    ) -> Result<()> {
        let num_txns = to_commit.len();
        ensure!(num_txns != 0, "Empty block is not allowed.");
        ensure!(
            base.base_version == base.current_version,
            "Block base state is not a checkpoint. base_version {:?}, current_version {:?}",
            base.base_version,
            base.current_version,
        );
        ensure!(
            base.updates_since_base.is_empty(),
            "Base state is corrupted, updates_since_base is not empty at a checkpoint."
        );

        for (i, (txn, _txn_out, is_reconfig)) in to_commit.iter().enumerate() {
            ensure!(
                TransactionsWithOutput::need_checkpoint(txn, is_reconfig) ^ (i != num_txns - 1),
                "Checkpoint is allowed iff it's the last txn in the block. index: {i}, num_txns: {num_txns}, is_last: {}, txn: {txn:?}, is_reconfig: {}",
                i == num_txns - 1,
                is_reconfig,
            );
        }
        Ok(())
    }
}

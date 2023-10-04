// Copyright © Aptos Foundation
// Parts of the project are originally copyright © Meta Platforms, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::sharded_block_executor::{
    counters::{
        NUM_EXECUTOR_SHARDS, SHARDED_BLOCK_EXECUTION_SECONDS,
        SHARDED_EXECUTION_RESULT_AGGREGATION_SECONDS,
    },
    executor_client::ExecutorClient,
};
use aptos_logger::{info, trace};
use aptos_state_view::StateView;
use aptos_types::{
    block_executor::partitioner::{PartitionV3, PartitionedTransactions, SubBlocksForShard},
    transaction::{analyzed_transaction::AnalyzedTransaction, TransactionOutput},
};
use move_core_types::vm_status::VMStatus;
use std::{marker::PhantomData, sync::Arc};

pub mod aggr_overridden_state_view;
pub mod coordinator_client;
mod counters;
pub mod cross_shard_client;
mod cross_shard_state_view;
pub mod executor_client;
pub mod global_executor;
pub mod local_executor_shard;
pub mod messages;
pub mod remote_state_value;
pub mod sharded_aggregator_service;
pub mod sharded_executor_service;

/// Coordinator for sharded block executors that manages multiple shards and aggregates the results.
pub struct ShardedBlockExecutor<S: StateView + Sync + Send + 'static, C: ExecutorClient<S>> {
    executor_client: C,
    phantom: PhantomData<S>,
}

pub enum ExecutorShardCommand<S> {
    ExecuteSubBlocks(
        Arc<S>,
        SubBlocksForShard<AnalyzedTransaction>,
        usize,
        Option<u64>,
    ),
    ExecuteV3Partition(ExecuteV3PartitionCommand<S>),
    Stop,
}

pub struct ExecuteV3PartitionCommand<S> {
    pub state_view: Arc<S>,
    pub partition: PartitionV3,
    pub concurrency_level_per_shard: usize,
    pub maybe_block_gas_limit: Option<u64>,
}

impl<S: StateView + Sync + Send + 'static, C: ExecutorClient<S>> ShardedBlockExecutor<S, C> {
    pub fn new(executor_client: C) -> Self {
        info!(
            "Creating a new ShardedBlockExecutor with {} shards",
            executor_client.num_shards()
        );
        Self {
            executor_client,
            phantom: PhantomData,
        }
    }

    pub fn num_shards(&self) -> usize {
        self.executor_client.num_shards()
    }

    /// Execute a block of transactions in parallel by splitting the block into num_remote_executors partitions and
    /// dispatching each partition to a remote executor shard.
    pub fn execute_block(
        &self,
        state_view: Arc<S>,
        transactions: PartitionedTransactions,
        concurrency_level_per_shard: usize,
        maybe_block_gas_limit: Option<u64>,
    ) -> Result<Vec<TransactionOutput>, VMStatus> {
        let _timer = SHARDED_BLOCK_EXECUTION_SECONDS.start_timer();
        let num_executor_shards = self.executor_client.num_shards();
        NUM_EXECUTOR_SHARDS.set(num_executor_shards as i64);
        assert_eq!(
            num_executor_shards,
            transactions.num_shards(),
            "Block must be partitioned into {} sub-blocks",
            num_executor_shards
        );

        if let PartitionedTransactions::V3(obj) = transactions {
            return self.executor_client.execute_block_v3(
                state_view,
                obj,
                concurrency_level_per_shard,
                maybe_block_gas_limit,
            );
        }

        let (sharded_output, global_output) = self
            .executor_client
            .execute_block(
                state_view,
                transactions,
                concurrency_level_per_shard,
                maybe_block_gas_limit,
            )?
            .into_inner();
        // wait for all remote executors to send the result back and append them in order by shard id
        trace!("ShardedBlockExecutor Received all results");
        let _aggregation_timer = SHARDED_EXECUTION_RESULT_AGGREGATION_SECONDS.start_timer();
        let num_rounds = sharded_output[0].len();
        let mut aggregated_results = vec![];
        let mut ordered_results = vec![vec![]; num_executor_shards * num_rounds];
        // Append the output from individual shards in the round order
        for (shard_id, results_from_shard) in sharded_output.into_iter().enumerate() {
            for (round, result) in results_from_shard.into_iter().enumerate() {
                ordered_results[round * num_executor_shards + shard_id] = result;
            }
        }

        for result in ordered_results.into_iter() {
            aggregated_results.extend(result);
        }

        // Lastly append the global output
        aggregated_results.extend(global_output);

        Ok(aggregated_results)
    }
}

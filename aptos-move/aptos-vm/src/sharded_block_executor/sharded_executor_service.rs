// Copyright © Aptos Foundation

use crate::{
    adapter_common::{preprocess_transaction, PreprocessedTransaction},
    block_executor::BlockAptosVM,
    sharded_block_executor::{
        aggr_overridden_state_view::{AggregatorOverriddenStateView, TOTAL_SUPPLY_AGGR_BASE_VAL},
        coordinator_client::CoordinatorClient,
        counters::{SHARDED_BLOCK_EXECUTION_BY_ROUNDS_SECONDS, SHARDED_BLOCK_EXECUTOR_TXN_COUNT},
        cross_shard_client::{CrossShardClient, CrossShardCommitReceiver, CrossShardCommitSender},
        cross_shard_state_view::CrossShardStateView,
        messages::CrossShardMsg,
        ExecuteV3PartitionCommand, ExecutorShardCommand,
    },
    AptosVM,
};
use aptos_block_executor::txn_provider::{
    default::DefaultTxnProvider,
    sharded::{CrossShardClientForV3, ShardedTxnProvider},
};
use aptos_logger::{info, trace};
use aptos_state_view::StateView;
use aptos_types::{
    block_executor::partitioner::{
        PartitionV3, ShardId, SubBlock, SubBlocksForShard, TransactionWithDependencies,
    },
    transaction::{analyzed_transaction::AnalyzedTransaction, TransactionOutput},
};
use aptos_vm_logging::disable_speculative_logging;
use futures::{channel::oneshot, executor::block_on};
use move_core_types::vm_status::VMStatus;
use rayon::{
    iter::ParallelIterator,
    prelude::{IndexedParallelIterator, IntoParallelIterator},
};
use std::sync::Arc;

pub struct ShardedExecutorService<S: StateView + Sync + Send + 'static> {
    shard_id: ShardId,
    num_shards: usize,
    executor_thread_pool: Arc<rayon::ThreadPool>,
    coordinator_client: Arc<dyn CoordinatorClient<S>>,
    cross_shard_client: Arc<dyn CrossShardClient>,
    v3_client: Arc<dyn CrossShardClientForV3<PreprocessedTransaction, VMStatus>>,
}

impl<S: StateView + Sync + Send + 'static> ShardedExecutorService<S> {
    pub fn new(
        shard_id: ShardId,
        num_shards: usize,
        num_threads: usize,
        coordinator_client: Arc<dyn CoordinatorClient<S>>,
        cross_shard_client: Arc<dyn CrossShardClient>,
        v3_client: Arc<dyn CrossShardClientForV3<PreprocessedTransaction, VMStatus>>,
    ) -> Self {
        let executor_thread_pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                // We need two extra threads for the cross-shard commit receiver and the thread
                // that is blocked on waiting for execute block to finish.
                .thread_name(move |i| format!("sharded-executor-shard-{}-{}", shard_id, i))
                .num_threads(num_threads + 2)
                .build()
                .unwrap(),
        );
        Self {
            shard_id,
            num_shards,
            executor_thread_pool,
            coordinator_client,
            cross_shard_client,
            v3_client,
        }
    }

    fn execute_sub_block(
        &self,
        sub_block: SubBlock<AnalyzedTransaction>,
        round: usize,
        state_view: &S,
        concurrency_level: usize,
        maybe_block_gas_limit: Option<u64>,
    ) -> Result<Vec<TransactionOutput>, VMStatus> {
        disable_speculative_logging();
        trace!(
            "executing sub block for shard {} and round {}",
            self.shard_id,
            round
        );
        let cross_shard_commit_sender =
            CrossShardCommitSender::new(self.shard_id, self.cross_shard_client.clone(), &sub_block);
        Self::execute_transactions_with_dependencies(
            Some(self.shard_id),
            self.executor_thread_pool.clone(),
            sub_block.into_transactions_with_deps(),
            self.cross_shard_client.clone(),
            Some(cross_shard_commit_sender),
            round,
            state_view,
            concurrency_level,
            maybe_block_gas_limit,
        )
    }

    pub fn execute_transactions_with_dependencies(
        shard_id: Option<ShardId>, // None means execution on global shard
        executor_thread_pool: Arc<rayon::ThreadPool>,
        transactions: Vec<TransactionWithDependencies<AnalyzedTransaction>>,
        cross_shard_client: Arc<dyn CrossShardClient>,
        cross_shard_commit_sender: Option<CrossShardCommitSender>,
        round: usize,
        state_view: &S,
        concurrency_level: usize,
        maybe_block_gas_limit: Option<u64>,
    ) -> Result<Vec<TransactionOutput>, VMStatus> {
        let (callback, callback_receiver) = oneshot::channel();

        let cross_shard_state_view = Arc::new(CrossShardStateView::create_cross_shard_state_view(
            state_view,
            &transactions,
        ));

        let cross_shard_state_view_clone = cross_shard_state_view.clone();
        let cross_shard_client_clone = cross_shard_client.clone();

        let aggr_overridden_state_view = Arc::new(AggregatorOverriddenStateView::new(
            cross_shard_state_view.as_ref(),
            TOTAL_SUPPLY_AGGR_BASE_VAL,
        ));

        executor_thread_pool.clone().scope(|s| {
            s.spawn(move |_| {
                CrossShardCommitReceiver::start(
                    cross_shard_state_view_clone,
                    cross_shard_client,
                    round,
                );
            });
            s.spawn(move |_| {
                let txns = transactions
                    .into_iter()
                    .map(|txn| txn.into_txn().into_txn())
                    .collect();
                let pre_processed_txns =
                    executor_thread_pool.install(|| BlockAptosVM::verify_transactions(txns));

                let txn_provider = Arc::new(DefaultTxnProvider::new(pre_processed_txns));
                let ret = BlockAptosVM::execute_block(
                    executor_thread_pool,
                    txn_provider,
                    aggr_overridden_state_view.as_ref(),
                    concurrency_level,
                    maybe_block_gas_limit,
                    cross_shard_commit_sender,
                );
                if let Some(shard_id) = shard_id {
                    trace!(
                        "executed sub block for shard {} and round {}",
                        shard_id,
                        round
                    );
                    // Send a self message to stop the cross-shard commit receiver.
                    cross_shard_client_clone.send_cross_shard_msg(
                        shard_id,
                        round,
                        CrossShardMsg::StopMsg,
                    );
                } else {
                    trace!("executed block for global shard and round {}", round);
                    // Send a self message to stop the cross-shard commit receiver.
                    cross_shard_client_clone.send_global_msg(CrossShardMsg::StopMsg);
                }
                callback.send(ret).unwrap();
            });
        });
        block_on(callback_receiver).unwrap()
    }

    fn execute_block(
        &self,
        transactions: SubBlocksForShard<AnalyzedTransaction>,
        state_view: &S,
        concurrency_level: usize,
        maybe_block_gas_limit: Option<u64>,
    ) -> Result<Vec<Vec<TransactionOutput>>, VMStatus> {
        let mut result = vec![];
        for (round, sub_block) in transactions.into_sub_blocks().into_iter().enumerate() {
            let _timer = SHARDED_BLOCK_EXECUTION_BY_ROUNDS_SECONDS
                .with_label_values(&[&self.shard_id.to_string(), &round.to_string()])
                .start_timer();
            SHARDED_BLOCK_EXECUTOR_TXN_COUNT
                .with_label_values(&[&self.shard_id.to_string(), &round.to_string()])
                .observe(sub_block.transactions.len() as f64);
            info!(
                "executing sub block for shard {} and round {}, number of txns {}",
                self.shard_id,
                round,
                sub_block.transactions.len()
            );
            result.push(self.execute_sub_block(
                sub_block,
                round,
                state_view,
                concurrency_level,
                maybe_block_gas_limit,
            )?);
            trace!(
                "Finished executing sub block for shard {} and round {}",
                self.shard_id,
                round
            );
        }
        Ok(result)
    }

    pub fn start(&self) {
        trace!(
            "Shard starting, shard_id={}, num_shards={}.",
            self.shard_id,
            self.num_shards
        );
        loop {
            let command = self.coordinator_client.receive_execute_command();
            match command {
                ExecutorShardCommand::ExecuteSubBlocks(
                    state_view,
                    transactions,
                    concurrency_level_per_shard,
                    maybe_block_gas_limit,
                ) => {
                    trace!(
                        "Shard {} received ExecuteBlock command of block size {} ",
                        self.shard_id,
                        transactions.num_txns()
                    );
                    let ret = self.execute_block(
                        transactions,
                        state_view.as_ref(),
                        concurrency_level_per_shard,
                        maybe_block_gas_limit,
                    );
                    drop(state_view);
                    self.coordinator_client.send_execution_result(ret);
                },
                ExecutorShardCommand::ExecuteV3Partition(cmd) => {
                    let ExecuteV3PartitionCommand {
                        state_view,
                        partition,
                        concurrency_level_per_shard,
                        maybe_block_gas_limit,
                    } = cmd;
                    let PartitionV3 {
                        block_id,
                        txns,
                        global_idxs,
                        local_idx_by_global,
                        key_sets_by_dep,
                        follower_shard_sets,
                    } = partition;
                    let processed_txns = self.executor_thread_pool.install(|| {
                        txns.into_par_iter()
                            .with_min_len(25)
                            .map(|analyzed_txn| {
                                let txn = analyzed_txn.into_txn();
                                preprocess_transaction::<AptosVM>(txn)
                            })
                            .collect()
                    });
                    let txn_provider = ShardedTxnProvider::new(
                        block_id,
                        self.num_shards,
                        self.shard_id,
                        self.v3_client.clone(),
                        processed_txns,
                        global_idxs,
                        local_idx_by_global,
                        key_sets_by_dep,
                        follower_shard_sets,
                    );

                    disable_speculative_logging();
                    let result = BlockAptosVM::execute_block(
                        self.executor_thread_pool.clone(),
                        Arc::new(txn_provider),
                        state_view.as_ref(),
                        concurrency_level_per_shard,
                        maybe_block_gas_limit,
                        None::<CrossShardCommitSender>,
                    );

                    // Wrap the 1D result as a 2D result so we can reuse the existing `result_rxs`.
                    let wrapped_2d_result = result.map(|output_vec| vec![output_vec]);
                    self.coordinator_client
                        .send_execution_result(wrapped_2d_result)
                },
                ExecutorShardCommand::Stop => {
                    break;
                },
            }
        }
        trace!("Shard {} is shutting down", self.shard_id);
    }
}

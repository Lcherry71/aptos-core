// Copyright © Aptos Foundation
// Parts of the project are originally copyright © Meta Platforms, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::{
    executor::BlockExecutor,
    proptest_types::{
        baseline::BaselineOutput,
        types::{
            EmptyDataView, KeyType, MockOutput, MockTask, MockTransaction, TransactionGen,
            TransactionGenParams, ValueType,
        },
    },
    txn_commit_hook::NoOpTransactionCommitHook,
    txn_provider::default::DefaultTxnProvider,
};
use aptos_types::{contract_event::ReadWriteEvent, executable::ExecutableTestType};
use criterion::{BatchSize, Bencher as CBencher};
use num_cpus;
use proptest::{
    arbitrary::Arbitrary,
    collection::vec,
    prelude::*,
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use std::{fmt::Debug, hash::Hash, marker::PhantomData, sync::Arc};

pub struct Bencher<K, V, E> {
    transaction_size: usize,
    transaction_gen_param: TransactionGenParams,
    universe_size: usize,
    phantom: PhantomData<(K, V, E)>,
}

pub(crate) struct BencherState<
    K: Hash + Clone + Debug + Eq + PartialOrd + Ord,
    E: Send + Sync + Debug + Clone + ReadWriteEvent,
> {
    transactions: Vec<MockTransaction<KeyType<K>, ValueType, E>>,
    baseline_output: BaselineOutput<KeyType<K>, ValueType>,
}

impl<K, V, E> Bencher<K, V, E>
where
    K: Hash + Clone + Debug + Eq + Send + Sync + PartialOrd + Ord + Arbitrary + 'static,
    V: Clone + Eq + Send + Sync + Arbitrary + 'static,
    E: Send + Sync + Debug + Clone + ReadWriteEvent + 'static,
    Vec<u8>: From<V>,
{
    pub fn new(transaction_size: usize, universe_size: usize) -> Self {
        Self {
            transaction_size,
            transaction_gen_param: TransactionGenParams::default(),
            universe_size,
            phantom: PhantomData,
        }
    }

    pub fn bench(&self, key_strategy: &impl Strategy<Value = K>, bencher: &mut CBencher) {
        bencher.iter_batched(
            || {
                BencherState::<K, E>::with_universe::<V>(
                    vec(key_strategy, self.universe_size),
                    self.transaction_size,
                    self.transaction_gen_param,
                )
            },
            |state| state.run(),
            // The input here is the entire list of signed transactions, so it's pretty large.
            BatchSize::LargeInput,
        )
    }
}

impl<K, E> BencherState<K, E>
where
    K: Hash + Clone + Debug + Eq + Send + Sync + PartialOrd + Ord + 'static,
    E: Send + Sync + Debug + Clone + ReadWriteEvent + 'static,
{
    /// Creates a new benchmark state with the given account universe strategy and number of
    /// transactions.
    pub(crate) fn with_universe<
        V: Into<Vec<u8>> + Clone + Eq + Send + Sync + Arbitrary + 'static,
    >(
        universe_strategy: impl Strategy<Value = Vec<K>>,
        num_transactions: usize,
        transaction_params: TransactionGenParams,
    ) -> Self {
        let mut runner = TestRunner::default();
        let key_universe = universe_strategy
            .new_tree(&mut runner)
            .expect("creating a new value should succeed")
            .current();

        let transaction_gens = vec(
            any_with::<TransactionGen<V>>(transaction_params),
            num_transactions,
        )
        .new_tree(&mut runner)
        .expect("creating a new value should succeed")
        .current();

        let transactions: Vec<_> = transaction_gens
            .into_iter()
            .map(|txn_gen| txn_gen.materialize(&key_universe, (false, false)))
            .collect();

        let baseline_output = BaselineOutput::generate(&transactions, None);

        Self {
            transactions,
            baseline_output,
        }
    }

    pub(crate) fn run(self) {
        let data_view = EmptyDataView::<KeyType<K>, ValueType> {
            phantom: PhantomData,
        };

        let executor_thread_pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(num_cpus::get())
                .build()
                .unwrap(),
        );

        let txn_provider = Arc::new(DefaultTxnProvider::new(self.transactions.clone()));
        let output = BlockExecutor::<
            MockTransaction<KeyType<K>, ValueType, E>,
            MockTask<KeyType<K>, ValueType, E>,
            EmptyDataView<KeyType<K>, ValueType>,
            NoOpTransactionCommitHook<MockOutput<KeyType<K>, ValueType, E>, usize>,
            ExecutableTestType,
            _,
        >::new(num_cpus::get(), executor_thread_pool, None, None)
        .execute_transactions_parallel((), txn_provider, &data_view);

        self.baseline_output.assert_output(&output);
    }
}

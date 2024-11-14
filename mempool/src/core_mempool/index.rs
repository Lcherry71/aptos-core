// Copyright © Aptos Foundation
// Parts of the project are originally copyright © Meta Platforms, Inc.
// SPDX-License-Identifier: Apache-2.0

/// This module provides various indexes used by Mempool.
use crate::{
    core_mempool::transaction::{MempoolTransaction, TimelineState},
    shared_mempool::types::{MultiBucketTimelineIndexIds, TimelineIndexIdentifier},
};
use aptos_consensus_types::common::TransactionSummary;
use aptos_crypto::HashValue;
use aptos_logger::prelude::*;
use aptos_types::{account_address::AccountAddress, transaction::ReplayProtector};
use rand::seq::SliceRandom;
use std::{
    cmp::Ordering,
    collections::{btree_set::Iter, BTreeMap, BTreeSet, HashMap},
    hash::Hash,
    iter::Rev,
    ops::Bound,
    time::{Duration, Instant, SystemTime},
};

pub type AccountTransactions = BTreeMap<ReplayProtector, MempoolTransaction>;

/// PriorityIndex represents the main Priority Queue in Mempool.
/// It's used to form the transaction block for Consensus.
/// Transactions are ordered by gas price. Second level ordering is done by expiration time.
///
/// We don't store the full content of transactions in the index.
/// Instead we use `OrderedQueueKey` - logical reference to the transaction in the main store.
pub struct PriorityIndex {
    data: BTreeSet<OrderedQueueKey>,
}

pub type PriorityQueueIter<'a> = Rev<Iter<'a, OrderedQueueKey>>;

impl PriorityIndex {
    pub(crate) fn new() -> Self {
        Self {
            data: BTreeSet::new(),
        }
    }

    pub(crate) fn insert(&mut self, txn: &MempoolTransaction) {
        self.data.insert(self.make_key(txn));
    }

    pub(crate) fn remove(&mut self, txn: &MempoolTransaction) {
        self.data.remove(&self.make_key(txn));
    }

    pub(crate) fn contains(&self, txn: &MempoolTransaction) -> bool {
        self.data.contains(&self.make_key(txn))
    }

    fn make_key(&self, txn: &MempoolTransaction) -> OrderedQueueKey {
        OrderedQueueKey {
            gas_ranking_score: txn.ranking_score,
            expiration_time: txn.expiration_time,
            insertion_time: txn.insertion_info.insertion_time,
            address: txn.get_sender(),
            replay_protector: txn.sequence_info.transaction_replay_protector,
            hash: txn.get_committed_hash(),
        }
    }

    pub(crate) fn iter(&self) -> PriorityQueueIter {
        self.data.iter().rev()
    }

    pub(crate) fn size(&self) -> usize {
        self.data.len()
    }
}

#[derive(Eq, PartialEq, Clone, Debug, Hash)]
pub struct OrderedQueueKey {
    pub gas_ranking_score: u64,
    pub expiration_time: Duration,
    pub insertion_time: SystemTime,
    pub address: AccountAddress,
    pub replay_protector: ReplayProtector,
    pub hash: HashValue,
}

impl PartialOrd for OrderedQueueKey {
    fn partial_cmp(&self, other: &OrderedQueueKey) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedQueueKey {
    fn cmp(&self, other: &OrderedQueueKey) -> Ordering {
        match self.gas_ranking_score.cmp(&other.gas_ranking_score) {
            Ordering::Equal => {},
            ordering => return ordering,
        }
        match self.insertion_time.cmp(&other.insertion_time).reverse() {
            Ordering::Equal => {},
            ordering => return ordering,
        }
        match self.address.cmp(&other.address) {
            Ordering::Equal => {},
            ordering => return ordering,
        }
        // Question: Orderless transactions with Nonce are always prioritize over regular sequence number transactions.
        // Is it okay?
        match self.replay_protector.cmp(&other.replay_protector) {
            Ordering::Equal => {},
            ordering => return ordering,
        }
        self.hash.cmp(&other.hash)
    }
}

/// TTLIndex is used to perform garbage collection of old transactions in Mempool.
/// Periodically separate GC-like job queries this index to find out transactions that have to be
/// removed. Index is represented as `BTreeSet<TTLOrderingKey>`, where `TTLOrderingKey`
/// is a logical reference to TxnInfo.
/// Index is ordered by `TTLOrderingKey::expiration_time`.
pub struct TTLIndex {
    data: BTreeSet<TTLOrderingKey>,
    get_expiration_time: Box<dyn Fn(&MempoolTransaction) -> Duration + Send + Sync>,
}

impl TTLIndex {
    pub(crate) fn new<F>(get_expiration_time: Box<F>) -> Self
    where
        F: Fn(&MempoolTransaction) -> Duration + 'static + Send + Sync,
    {
        Self {
            data: BTreeSet::new(),
            get_expiration_time,
        }
    }

    pub(crate) fn insert(&mut self, txn: &MempoolTransaction) {
        self.data.insert(self.make_key(txn));
    }

    pub(crate) fn remove(&mut self, txn: &MempoolTransaction) {
        self.data.remove(&self.make_key(txn));
    }

    /// Garbage collect all old transactions.
    pub(crate) fn gc(&mut self, now: Duration) -> Vec<TTLOrderingKey> {
        // Ideally, we should garbage collect all transactions with expiration time < now.
        let max_expiration_time = now.saturating_sub(Duration::from_micros(1));
        let ttl_key = TTLOrderingKey {
            expiration_time: max_expiration_time,
            address: AccountAddress::ZERO,
            replay_protector: ReplayProtector::Nonce(0),
        };

        let mut active = self.data.split_off(&ttl_key);
        let ttl_transactions = self.data.iter().cloned().collect();
        self.data.clear();
        self.data.append(&mut active);
        ttl_transactions
    }

    fn make_key(&self, txn: &MempoolTransaction) -> TTLOrderingKey {
        TTLOrderingKey {
            expiration_time: (self.get_expiration_time)(txn),
            address: txn.get_sender(),
            replay_protector: txn.sequence_info.transaction_replay_protector,
        }
    }

    pub(crate) fn iter(&self) -> Iter<TTLOrderingKey> {
        self.data.iter()
    }

    pub(crate) fn size(&self) -> usize {
        self.data.len()
    }
}

#[allow(clippy::derive_ord_xor_partial_ord)]
#[derive(Eq, PartialEq, PartialOrd, Clone, Debug)]
pub struct TTLOrderingKey {
    pub expiration_time: Duration,
    pub address: AccountAddress,
    pub replay_protector: ReplayProtector,
}

/// Be very careful with this, to not break the partial ordering.
/// See:  https://rust-lang.github.io/rust-clippy/master/index.html#derive_ord_xor_partial_ord
#[allow(clippy::derive_ord_xor_partial_ord)]
impl Ord for TTLOrderingKey {
    fn cmp(&self, other: &TTLOrderingKey) -> Ordering {
        match self.expiration_time.cmp(&other.expiration_time) {
            Ordering::Equal => {
                match self.address.cmp(&other.address) {
                    Ordering::Equal => self.replay_protector.cmp(&other.replay_protector),
                    ordering => ordering,
                }
            },
            ordering => ordering,
        }
    }
}

/// TimelineIndex is an ordered log of all transactions that are "ready" for broadcast.
/// We only add a transaction to the index if it has a chance to be included in the next consensus
/// block (which means its status is != NotReady or its sequential to another "ready" transaction).
///
/// It's represented as Map <timeline_id, (Address, Replay Protector)>, where timeline_id is auto
/// increment unique id of "ready" transaction in local Mempool. (Address, Replay Protector) is a
/// logical reference to transaction content in main storage.
pub struct TimelineIndex {
    timeline_id: u64,
    timeline: BTreeMap<u64, (AccountAddress, ReplayProtector, Instant)>,
}

impl TimelineIndex {
    pub(crate) fn new() -> Self {
        Self {
            timeline_id: 1,
            timeline: BTreeMap::new(),
        }
    }

    /// Read all transactions from the timeline since <timeline_id>.
    /// At most `count` transactions will be returned.
    /// If `before` is set, only transactions inserted before this time will be returned.
    pub(crate) fn read_timeline(
        &self,
        timeline_id: u64,
        count: usize,
        before: Option<Instant>,
    ) -> Vec<(AccountAddress, ReplayProtector)> {
        let mut batch = vec![];
        for (_id, &(address, replay_protector, insertion_time)) in self
            .timeline
            .range((Bound::Excluded(timeline_id), Bound::Unbounded))
        {
            if let Some(before) = before {
                if insertion_time >= before {
                    break;
                }
            }
            if batch.len() == count {
                break;
            }
            batch.push((address, replay_protector));
        }
        batch
    }

    /// Read transactions from the timeline from `start_id` (exclusive) to `end_id` (inclusive).
    pub(crate) fn timeline_range(&self, start_id: u64, end_id: u64) -> Vec<(AccountAddress, u64)> {
        self.timeline
            .range((Bound::Excluded(start_id), Bound::Included(end_id)))
            .map(|(_idx, &(address, replay_protector, _))| (address, replay_protector))
            .collect()
    }

    pub(crate) fn insert(&mut self, txn: &mut MempoolTransaction) {
        self.timeline.insert(
            self.timeline_id,
            (
                txn.get_sender(),
                txn.sequence_info.transaction_replay_protector,
                Instant::now(),
            ),
        );
        txn.timeline_state = TimelineState::Ready(self.timeline_id);
        self.timeline_id += 1;
    }

    pub(crate) fn remove(&mut self, txn: &MempoolTransaction) {
        if let TimelineState::Ready(timeline_id) = txn.timeline_state {
            self.timeline.remove(&timeline_id);
        }
    }

    pub(crate) fn size(&self) -> usize {
        self.timeline.len()
    }
}

pub struct MultiBucketTimelineIndex {
    timelines: Vec<TimelineIndex>,
    bucket_mins: Vec<u64>,
    bucket_mins_to_string: Vec<String>,
}

impl MultiBucketTimelineIndex {
    pub(crate) fn new(bucket_mins: Vec<u64>) -> anyhow::Result<Self> {
        anyhow::ensure!(!bucket_mins.is_empty(), "Must not be empty");
        anyhow::ensure!(bucket_mins[0] == 0, "First bucket must start at 0");

        let mut prev = None;
        let mut timelines = vec![];
        for entry in bucket_mins.clone() {
            if let Some(prev) = prev {
                anyhow::ensure!(prev < entry, "Values must be sorted and not repeat");
            }
            prev = Some(entry);
            timelines.push(TimelineIndex::new());
        }

        let bucket_mins_to_string: Vec<_> = bucket_mins
            .iter()
            .map(|bucket_min| bucket_min.to_string())
            .collect();

        Ok(Self {
            timelines,
            bucket_mins,
            bucket_mins_to_string,
        })
    }

    /// Read all transactions from the timeline since <timeline_id>.
    /// At most `count` transactions will be returned.
    pub(crate) fn read_timeline(
        &self,
        timeline_id: &MultiBucketTimelineIndexIds,
        count: usize,
        before: Option<Instant>,
    ) -> Vec<Vec<(AccountAddress, u64)>> {
        assert!(timeline_id.id_per_bucket.len() == self.bucket_mins.len());

        let mut added = 0;
        let mut returned = vec![];
        for (timeline, &timeline_id) in self
            .timelines
            .iter()
            .zip(timeline_id.id_per_bucket.iter())
            .rev()
        {
            let txns = timeline.read_timeline(timeline_id, count - added, before);
            added += txns.len();
            returned.push(txns);

            if added == count {
                break;
            }
        }
        while returned.len() < self.timelines.len() {
            returned.push(vec![]);
        }
        returned.iter().rev().cloned().collect()
    }

    /// Read transactions from the timeline from `start_id` (exclusive) to `end_id` (inclusive).
    pub(crate) fn timeline_range(
        &self,
        start_end_pairs: HashMap<TimelineIndexIdentifier, (u64, u64)>,
    ) -> Vec<(AccountAddress, u64)> {
        assert_eq!(start_end_pairs.len(), self.timelines.len());

        let mut all_txns = vec![];
        for (timeline_index_identifier, (start_id, end_id)) in start_end_pairs {
            let mut txns = self
                .timelines
                .get(timeline_index_identifier as usize)
                .map_or_else(Vec::new, |timeline| {
                    timeline.timeline_range(start_id, end_id)
                });
            all_txns.append(&mut txns);
        }
        all_txns
    }

    #[inline]
    fn get_timeline(&mut self, ranking_score: u64) -> &mut TimelineIndex {
        let index = self
            .bucket_mins
            .binary_search(&ranking_score)
            .unwrap_or_else(|i| i - 1);
        self.timelines.get_mut(index).unwrap()
    }

    pub(crate) fn insert(&mut self, txn: &mut MempoolTransaction) {
        self.get_timeline(txn.ranking_score).insert(txn);
    }

    pub(crate) fn remove(&mut self, txn: &MempoolTransaction) {
        self.get_timeline(txn.ranking_score).remove(txn);
    }

    pub(crate) fn size(&self) -> usize {
        let mut size = 0;
        for timeline in &self.timelines {
            size += timeline.size()
        }
        size
    }

    pub(crate) fn get_sizes(&self) -> Vec<(&str, usize)> {
        self.bucket_mins_to_string
            .iter()
            .zip(self.timelines.iter())
            .map(|(bucket_min, timeline)| (bucket_min.as_str(), timeline.size()))
            .collect()
    }

    #[inline]
    pub(crate) fn get_bucket(&self, ranking_score: u64) -> &str {
        let index = self
            .bucket_mins
            .binary_search(&ranking_score)
            .unwrap_or_else(|i| i - 1);
        self.bucket_mins_to_string[index].as_str()
    }
}

/// ParkingLotIndex keeps track of "not_ready" transactions, e.g., transactions that
/// can't be included in the next block because their sequence number is too high.
/// We keep a separate index to be able to efficiently evict them when Mempool is full.
pub struct ParkingLotIndex {
    data: HashMap<AccountAddress, BTreeSet<(u64, HashValue)>>,
}

impl ParkingLotIndex {
    pub(crate) fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, txn: &mut MempoolTransaction) {
        // Orderless transactions are always in the "ready" state and are not stored in the parking lot.
        match txn.sequence_info.transaction_replay_protector {
            ReplayProtector::SequenceNumber(sequence_num) => {
                if txn.insertion_info.park_time.is_none() {
                    txn.insertion_info.park_time = Some(SystemTime::now());
                }
                txn.was_parked = true;
        
                self.data
                    .entry(txn.txn.sender())
                    .or_default()
                    .insert((sequence_num, txn.get_committed_hash()));
            }
            ReplayProtector::Nonce(_) => {}
        }
    }

    pub(crate) fn remove(&mut self, txn: &MempoolTransaction) {
        // Orderless transactions are always in the "ready" state and are not stored in the parking lot.
        match txn.sequence_info.transaction_replay_protector {
            ReplayProtector::SequenceNumber(sequence_num) => {
                let sender = &txn.txn.sender();
                if let Some(txns) = self.data.get_mut(sender) {
                    txns.remove(&(txn.txn.sequence_number(), txn.get_committed_hash()));

                    if txns.is_empty() {
                        self.data.remove(sender);
                    }
                }
            }
            ReplayProtector::Nonce(_) => {}
        }
    }

    pub(crate) fn contains(&self, account: &AccountAddress, replay_protector: ReplayProtector, hash: HashValue) -> bool {
        // Orderless transactions are always in the "ready" state and are not stored in the parking lot.
        match replay_protector {
            ReplayProtector::SequenceNumber(seq_num) => {
                self.data
                    .get(account)
                    .map_or(false, |txns| txns.contains(&(seq_num, hash)))
            }
            ReplayProtector::Nonce(_) => false,
        }
    }

    /// Returns a random "non-ready" transaction (with highest sequence number for that account).
    pub(crate) fn get_poppable(&self) -> Option<TxnPointer> {
        let mut rng = rand::thread_rng();
        let addresses = self.data.keys().collect::<Vec<_>>();
        let sender = addresses.choose(&mut rng)?;
        self.data.get(sender).and_then(|txns| {
            txns.iter().next_back().map(|(seq_num, hash)| TxnPointer {
                sender: **sender,
                replay_protector: ReplayProtector::SequenceNumber(*seq_num),
                hash: *hash,
            })
        })
    }

    pub(crate) fn size(&self) -> usize {
        self.data.len()
    }

    pub(crate) fn get_addresses(&self) -> Vec<(AccountAddress, u64)> {
        self.data
            .iter()
            .map(|(addr, txns)| (*addr, txns.len() as u64))
            .collect::<Vec<(AccountAddress, u64)>>()
    }
}

/// Logical pointer to `MempoolTransaction`.
/// Includes Account's address and transaction sequence number.
pub type TxnPointer = TransactionSummary;

impl From<&MempoolTransaction> for TxnPointer {
    fn from(txn: &MempoolTransaction) -> Self {
        Self {
            sender: txn.get_sender(),
            replay_protector: txn.sequence_info.transaction_replay_protector,
            hash: txn.get_committed_hash(),
        }
    }
}

impl From<&OrderedQueueKey> for TxnPointer {
    fn from(key: &OrderedQueueKey) -> Self {
        Self {
            sender: key.address,
            replay_protector: key.replay_protector,
            hash: key.hash,
        }
    }
}

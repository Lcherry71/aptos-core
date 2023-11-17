// Copyright © Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

pub mod cache_operator;
pub mod config;
pub mod constants;
pub mod counters;
pub mod file_store_operator;
pub mod storage;
pub mod types;

use anyhow::{Context, Result};
use aptos_protos::{
    indexer::v1::raw_data_client::RawDataClient,
    internal::fullnode::v1::fullnode_data_client::FullnodeDataClient, transaction::v1::Transaction,
    util::timestamp::Timestamp,
};
use prost::Message;
use std::time::Duration;
use url::Url;

pub type GrpcClientType = FullnodeDataClient<tonic::transport::Channel>;

// 9999-12-31 23:59:59, this is the max supported by Google BigQuery
pub const MAX_TIMESTAMP_SECS: i64 = 253_402_300_799;

/// Create a gRPC client with exponential backoff.
pub async fn create_grpc_client(address: Url) -> GrpcClientType {
    backoff::future::retry(backoff::ExponentialBackoff::default(), || async {
        match FullnodeDataClient::connect(address.to_string()).await {
            Ok(client) => {
                tracing::info!(
                    address = address.to_string(),
                    "[Indexer Cache] Connected to indexer gRPC server."
                );
                Ok(client
                    .max_decoding_message_size(usize::MAX)
                    .max_encoding_message_size(usize::MAX))
            },
            Err(e) => {
                tracing::error!(
                    address = address.to_string(),
                    "[Indexer Cache] Failed to connect to indexer gRPC server: {}",
                    e
                );
                Err(backoff::Error::transient(e))
            },
        }
    })
    .await
    .unwrap()
}

pub type GrpcDataServiceClientType = RawDataClient<tonic::transport::Channel>;

/// Create a gRPC client for the indexer data service with exponential backoff.
/// max_elapsed_time is the maximum time to wait for the connection to be established.
pub async fn create_data_service_grpc_client(
    address: Url,
    max_elapsed_time: Option<Duration>,
) -> Result<GrpcDataServiceClientType> {
    let mut backoff = backoff::ExponentialBackoff::default();
    if let Some(max_elapsed_time) = max_elapsed_time {
        backoff.max_elapsed_time = Some(max_elapsed_time);
    }
    let client = backoff::future::retry(backoff, || async {
        match RawDataClient::connect(address.to_string()).await {
            Ok(client) => {
                tracing::info!(
                    address = address.to_string(),
                    "[Indexer Cache] Connected to indexer data service gRPC server."
                );
                Ok(client)
            },
            Err(e) => {
                tracing::error!(
                    address = address.to_string(),
                    "[Indexer Cache] Failed to connect to indexer data service gRPC server: {}",
                    e
                );
                Err(backoff::Error::transient(e))
            },
        }
    })
    .await
    .context("Failed to create data service GRPC client")?;
    Ok(client)
}

pub fn time_diff_since_pb_timestamp_in_secs(timestamp: &Timestamp) -> f64 {
    let current_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("SystemTime before UNIX EPOCH!")
        .as_secs_f64();
    let transaction_time = timestamp.seconds as f64 + timestamp.nanos as f64 * 1e-9;
    current_timestamp - transaction_time
}

/// Convert the protobuf timestamp to ISO format
pub fn timestamp_to_iso(timestamp: &Timestamp) -> String {
    let dt = parse_timestamp(timestamp, 0);
    dt.format("%Y-%m-%dT%H:%M:%S%.9fZ").to_string()
}

/// Convert the protobuf timestamp to unixtime
pub fn timestamp_to_unixtime(timestamp: &Timestamp) -> f64 {
    timestamp.seconds as f64 + timestamp.nanos as f64 * 1e-9
}

pub fn parse_timestamp(ts: &Timestamp, version: i64) -> chrono::NaiveDateTime {
    let final_ts = if ts.seconds >= MAX_TIMESTAMP_SECS {
        Timestamp {
            seconds: MAX_TIMESTAMP_SECS,
            nanos: 0,
        }
    } else {
        ts.clone()
    };
    chrono::NaiveDateTime::from_timestamp_opt(final_ts.seconds, final_ts.nanos as u32)
        .unwrap_or_else(|| panic!("Could not parse timestamp {:?} for version {}", ts, version))
}

/// Chunk transactions into chunks with chunk size less than or equal to chunk_size.
/// If a single transaction is larger than chunk_size, it will be put into a chunk by itself.
pub fn chunk_transactions(
    transactions: Vec<Transaction>,
    chunk_size: usize,
) -> Vec<Vec<Transaction>> {
    let mut chunked_transactions = vec![];
    let mut chunk = vec![];
    let mut current_size = 0;

    for transaction in transactions {
        // Only add the chunk when it's empty.
        if !chunk.is_empty() && current_size + transaction.encoded_len() > chunk_size {
            chunked_transactions.push(chunk);
            chunk = vec![];
            current_size = 0;
        }
        current_size += transaction.encoded_len();
        chunk.push(transaction);
    }
    if !chunk.is_empty() {
        chunked_transactions.push(chunk);
    }
    chunked_transactions
}

// Tests
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_the_transactions_correctly_with_large_transaction() {
        let t = Transaction {
            version: 2,
            timestamp: Some(Timestamp {
                seconds: 1,
                nanos: 0,
            }),
            ..Transaction::default()
        };
        // Create a vec with 10 transactions.
        let transactions = vec![t.clone(); 10];
        assert!(t.encoded_len() > 1);
        let chunked_transactions = chunk_transactions(transactions, 1);
        assert_eq!(chunked_transactions.len(), 10);
    }

    #[test]
    fn test_chunk_the_transactions_correctly() {
        let t = Transaction {
            version: 2,
            timestamp: Some(Timestamp {
                seconds: 1,
                nanos: 0,
            }),
            ..Transaction::default()
        };
        // Create a vec with 10 transactions.
        let transactions = vec![t.clone(); 10];
        assert!(t.encoded_len() == 6);
        let chunked_transactions = chunk_transactions(transactions, 20);
        assert_eq!(chunked_transactions.len(), 4);
        let total_count = chunked_transactions
            .iter()
            .map(|chunk| chunk.len())
            .sum::<usize>();
        assert!(total_count == 10);
    }
}

// Copyright (c) Aptos Foundation
// SPDX-License-Identifier: Apache-2.0

use aptos_types::{
    account_address::AccountAddress,
    account_config::AccountResource,
    event::{EventHandle, EventKey},
    state_store::{state_key::StateKey, state_value::StateValue},
    write_set::{WriteOp, WriteSet},
};
use arr_macro::arr;
use criterion::{criterion_group, criterion_main, Criterion};
use itertools::Itertools;
use rayon::prelude::*;
use std::collections::HashMap;

fn collect_write_sets(c: &mut Criterion, write_sets: &[WriteSet]) {
    let mut group = c.benchmark_group("collect_write_sets");

    group.bench_function("write_set_refs", |b| {
        b.iter(|| write_sets.iter().collect_vec())
    });

    group.bench_function("write_set_cloned", |b| {
        b.iter(|| write_sets.iter().cloned().collect_vec())
    });
}

fn collect_write_ops(c: &mut Criterion, write_sets: &[WriteSet]) {
    let mut group = c.benchmark_group("collect_write_ops");

    group.bench_function("ref_vecs", |b| {
        b.iter(|| {
            write_sets
                .iter()
                .map(|w| w.iter().collect_vec())
                .collect::<Vec<Vec<(&StateKey, &WriteOp)>>>()
        })
    });

    group.bench_function("par_ref_vecs", |b| {
        b.iter(|| {
            write_sets
                .par_iter()
                .map(|w| w.iter().collect_vec())
                .collect::<Vec<Vec<(&StateKey, &WriteOp)>>>()
        })
    });

    group.bench_function("sharded_ref_vecs", |b| {
        b.iter(|| get_sharded_ref_vecs(write_sets))
    });

    group.bench_function("refs_flattened", |b| {
        b.iter(|| {
            write_sets
                .iter()
                .flat_map(|w| w.iter())
                .collect::<Vec<(&StateKey, &WriteOp)>>()
        })
    });

    group.bench_function("refs_with_idx", |b| {
        b.iter(|| {
            write_sets
                .iter()
                .enumerate()
                .flat_map(|(idx, w)| w.iter().map(move |(k, op)| (idx, k, op)))
                .collect::<Vec<(usize, &StateKey, &WriteOp)>>()
        })
    });

    group.bench_function("ref_map", |b| {
        b.iter(|| {
            write_sets
                .iter()
                .flat_map(|w| w.iter())
                .collect::<HashMap<&StateKey, &WriteOp>>()
        })
    });

    group.bench_function("par_ref_map", |b| {
        b.iter(|| {
            write_sets
                .par_iter()
                .flat_map(|w| w.iter().collect_vec())
                .collect::<HashMap<&StateKey, &WriteOp>>()
        })
    });

    let sharded_ref_vecs = get_sharded_ref_vecs(write_sets);
    group.bench_function("ref_map_per_shard", |b| {
        b.iter(|| {
            (0..16usize)
                .into_par_iter()
                .map(|shard_id| {
                    sharded_ref_vecs
                        .iter()
                        .flat_map(|shards| shards[shard_id].iter())
                        .cloned()
                        .collect::<HashMap<&StateKey, &WriteOp>>()
                })
                .collect::<Vec<HashMap<&StateKey, &WriteOp>>>()
        })
    });

    group.bench_function("ref_map_from_shards", |b| {
        b.iter(|| {
            (0..16usize)
                .into_par_iter()
                .flat_map(|shard_id| {
                    sharded_ref_vecs
                        .iter()
                        .flat_map(|shards| shards[shard_id].iter())
                        .cloned()
                        .collect::<HashMap<&StateKey, &WriteOp>>()
                })
                .collect::<HashMap<&StateKey, &WriteOp>>()
        })
    });

    group.bench_function("key_cloned_map", |b| {
        b.iter(|| {
            write_sets
                .iter()
                .flat_map(|w| w.iter().map(|(k, op)| (k.clone(), op)))
                .collect::<HashMap<StateKey, &WriteOp>>()
        })
    });

    group.bench_function("cloned_map", |b| {
        b.iter(|| {
            write_sets
                .iter()
                .flat_map(|w| w.iter().map(|(k, op)| (k.clone(), op.clone())))
                .collect::<HashMap<StateKey, WriteOp>>()
        })
    });
}

fn get_sharded_ref_vecs(write_sets: &[WriteSet]) -> Vec<[Vec<(&StateKey, &WriteOp)>; 16]> {
    write_sets
        .par_iter()
        .map(|write_set| {
            let mut ret = arr![Vec::new(); 16];
            write_set.iter().for_each(|(k, op)| {
                ret[k.get_shard_id() as usize].push((k, op));
            });
            ret
        })
        .collect::<Vec<[Vec<(&StateKey, &WriteOp)>; 16]>>()
}

fn collect_state_values(c: &mut Criterion, write_sets: &[WriteSet]) {
    let mut group = c.benchmark_group("collect_state_values");

    let write_set_refs = write_sets.iter().collect_vec();

    group.bench_function("vec_flattened", |b| {
        b.iter(|| {
            write_set_refs
                .iter()
                .flat_map(|w| w.state_update_refs())
                .collect::<Vec<(&StateKey, Option<&StateValue>)>>()
        })
    });

    group.bench_function("map", |b| {
        b.iter(|| {
            write_set_refs
                .iter()
                .flat_map(|w| w.state_update_refs())
                .collect::<HashMap<&StateKey, Option<&StateValue>>>()
        })
    });

    group.bench_function("vec_then_map", |b| {
        b.iter(|| {
            write_set_refs
                .iter()
                .flat_map(|w| w.state_update_refs())
                .collect_vec()
                .into_iter()
                .collect::<HashMap<&StateKey, Option<&StateValue>>>()
        })
    });

    group.bench_function("par_vec", |b| {
        b.iter(|| {
            write_set_refs
                .par_iter()
                .flat_map(|w| w.state_update_refs().collect_vec())
                .collect::<Vec<(&StateKey, Option<&StateValue>)>>()
        })
    });

    group.bench_function("par_extend_vec", |b| {
        b.iter(|| {
            let mut ret = Vec::new();
            ret.par_extend(
                write_set_refs
                    .par_iter()
                    .flat_map(|w| w.state_update_refs().collect_vec())
                    .collect::<Vec<(&StateKey, Option<&StateValue>)>>(),
            );
            ret
        })
    });

    group.bench_function("par_extend_map", |b| {
        b.iter(|| {
            let mut ret = HashMap::new();
            ret.par_extend(
                write_set_refs
                    .par_iter()
                    .flat_map(|w| w.state_update_refs().collect_vec()),
            );
            ret
        })
    });

    group.bench_function("par_map", |b| {
        b.iter(|| {
            write_set_refs
                .par_iter()
                .flat_map(|w| w.state_update_refs().collect_vec())
                .collect::<HashMap<&StateKey, Option<&StateValue>>>()
        })
    });

    group.bench_function("par_100_chunks_map", |b| {
        b.iter(|| {
            write_set_refs
                .par_chunks(100)
                .flat_map(|chunk| {
                    chunk
                        .iter()
                        .cloned()
                        .flat_map(WriteSet::state_update_refs)
                        .collect::<HashMap<&StateKey, Option<&StateValue>>>()
                })
                .collect::<HashMap<&StateKey, Option<&StateValue>>>()
        })
    });

    group.bench_function("par_1k_chunks_map", |b| {
        b.iter(|| {
            write_set_refs
                .par_chunks(1000)
                .flat_map(|chunk| {
                    chunk
                        .iter()
                        .cloned()
                        .flat_map(WriteSet::state_update_refs)
                        .collect::<HashMap<&StateKey, Option<&StateValue>>>()
                })
                .collect::<HashMap<&StateKey, Option<&StateValue>>>()
        })
    });

    group.bench_function("par_map_reduce_map", |b| {
        b.iter(|| {
            write_set_refs
                .par_iter()
                .map(|w| w.state_update_refs().collect())
                .reduce(
                    HashMap::<&StateKey, Option<&StateValue>>::new,
                    |mut acc, updates| {
                        acc.extend(updates);
                        acc
                    },
                )
        })
    });
}

fn collect_kvs_to_shards(c: &mut Criterion, write_sets: &[WriteSet]) {
    let mut group = c.benchmark_group("collect_state_updates_to_shards");

    let _write_set_refs = write_sets.iter().collect_vec();

    group.bench_function("collect_refs", |b| {
        b.iter(|| write_sets.iter().collect_vec())
    });

    group.bench_function("collect_vec", |b| {
        b.iter(|| {
            write_sets
                .iter()
                .flat_map(WriteSet::state_update_refs)
                .collect_vec()
        })
    });
}

fn clone_state_values(c: &mut Criterion, write_sets: &[WriteSet]) {
    let mut group = c.benchmark_group("clone_state_value_map");

    let refs: HashMap<&StateKey, Option<&StateValue>> = write_sets
        .iter()
        .flat_map(|w| w.state_update_refs())
        .collect();

    type OwnedMap = HashMap<StateKey, Option<StateValue>>;

    group.bench_function("clone", |b| {
        b.iter(|| {
            refs.iter()
                .map(|(k, v)| ((*k).clone(), (*v).cloned()))
                .collect::<OwnedMap>()
        })
    });

    group.bench_function("par_clone", |b| {
        b.iter(|| {
            refs.par_iter()
                .map(|(k, v)| ((*k).clone(), (*v).cloned()))
                .collect::<OwnedMap>()
        })
    });

    group.bench_function("par_extend", |b| {
        b.iter(|| {
            let mut ret: OwnedMap = HashMap::new();
            ret.par_extend(refs.par_iter().map(|(k, v)| ((*k).clone(), (*v).cloned())));
            ret
        })
    });
}

fn collect_state_updates(c: &mut Criterion) {
    rayon::ThreadPoolBuilder::new()
        .num_threads(32)
        .thread_name(|index| format!("rayon-global-{}", index))
        .build_global()
        .expect("Failed to build rayon global thread pool.");

    let account_resource = AccountResource::new(
        0,
        vec![0; 32],
        EventHandle::new(EventKey::new(0, AccountAddress::random()), 0),
        EventHandle::new(EventKey::new(1, AccountAddress::random()), 0),
    );
    let value_bytes = bcs::to_bytes(&account_resource).unwrap();

    let write_sets = (0..10000usize)
        .map(|idx| {
            let ws_size = idx % 10;
            WriteSet::new_for_test(
                std::iter::repeat_with(|| {
                    (
                        StateKey::resource_typed::<AccountResource>(&AccountAddress::random())
                            .unwrap(),
                        Some(StateValue::new_legacy(value_bytes.clone().into())),
                    )
                })
                .take(ws_size),
            )
        })
        .collect_vec();

    collect_write_sets(c, &write_sets);
    collect_write_ops(c, &write_sets);
    collect_state_values(c, &write_sets);
    collect_kvs_to_shards(c, &write_sets);
    clone_state_values(c, &write_sets);
}

criterion_group!(
    name = data_collection;
    config = Criterion::default();
    targets = collect_state_updates,
);

criterion_main!(data_collection);

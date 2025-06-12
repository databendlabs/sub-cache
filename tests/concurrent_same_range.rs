// Copyright 2021 Datafuse Labs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Concurrent same-range watchers test
//!
//! This test validates that multiple cache instances (watchers) can subscribe to
//! the same key range and maintain consistency when data is updated concurrently.
//! The key aspects being tested are:
//!
//! 1. **Same Range Subscriptions**: Multiple caches subscribe to identical key ranges
//! 2. **Concurrent Data Updates**: Data is written concurrently to a shared data source
//! 3. **Synchronization**: All caches stay synchronized with the data source
//! 4. **Consistency**: All caches see the same final state
//! 5. **Real-time Updates**: Changes propagate to all caches simultaneously
//! 6. **Load Distribution**: Multiple watchers don't interfere with each other
//!
//! The test simulates a scenario where multiple cache instances monitor
//! the same data range and must all stay up-to-date with concurrent changes.

use std::time::Duration;

use sub_cache::testing::source::TestSource;
use sub_cache::testing::source::Val;
use sub_cache::testing::types::TestConfig;
use sub_cache::testing::util::wait_for_cache_state;
use sub_cache::Cache;
use tokio::time::sleep;

// Configuration structures
#[derive(Clone)]
struct BatchConfig {
    start: usize,
    span: usize,
    update_delay_ms: u64,
}

impl BatchConfig {
    const fn new(start: usize, span: usize, update_delay_ms: u64) -> Self {
        Self {
            start,
            span,
            update_delay_ms,
        }
    }

    fn range(&self) -> std::ops::Range<usize> {
        self.start..(self.start + self.span)
    }
}

struct TestConfiguration {
    batches: [BatchConfig; 3],
    rapid: BatchConfig,
    existing_items: BatchConfig, // Configuration for updating existing items
    sync_wait_ms: u64,
    final_sync_wait_ms: u64,
    read_task_iterations: usize,
    read_task_delays_ms: [u64; 3],
}

// Test configuration - Only truly configurable values
const CONFIG: TestConfiguration = TestConfiguration {
    batches: [
        BatchConfig::new(10, 5, 10),
        BatchConfig::new(20, 5, 15),
        BatchConfig::new(30, 5, 12),
    ],
    rapid: BatchConfig::new(100, 10, 5),
    existing_items: BatchConfig::new(1, 3, 20), // start=1, span=3 (item1,item2,item3), delay=20ms
    sync_wait_ms: 500,
    final_sync_wait_ms: 300,
    read_task_iterations: 20,
    read_task_delays_ms: [8, 7, 9], // delays for cache A, B, C respectively
};

#[tokio::test]
async fn test_concurrent_same_range_watchers() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Create a single shared test source
    let source = TestSource::new();

    // 2. Pre-populate some initial data (hardcoded - test requires exactly these 3 items)
    source.set("data/item1", Some("initial_value_1")).await;
    source.set("data/item2", Some("initial_value_2")).await;
    source.set("data/item3", Some("initial_value_3")).await;

    // 3. Create multiple cache instances with the SAME key range
    let mut cache_a = Cache::<TestConfig>::new(source.clone(), "data", "watcher-a").await;
    let mut cache_b = Cache::<TestConfig>::new(source.clone(), "data", "watcher-b").await;
    let mut cache_c = Cache::<TestConfig>::new(source.clone(), "data", "watcher-c").await;

    // 4. Wait for all caches to initialize with the same initial data
    let expected_initial = &[
        ("data/item1", Some(Val::new(1, "initial_value_1"))),
        ("data/item2", Some(Val::new(2, "initial_value_2"))),
        ("data/item3", Some(Val::new(3, "initial_value_3"))),
    ];

    wait_for_cache_state(&mut cache_a, 3, expected_initial).await??;
    wait_for_cache_state(&mut cache_b, 3, expected_initial).await??;
    wait_for_cache_state(&mut cache_c, 3, expected_initial).await??;

    println!("All 3 caches initialized with same data");

    // 5. Start concurrent data updates from multiple threads
    let update_tasks: Vec<_> = CONFIG
        .batches
        .iter()
        .enumerate()
        .map(|(batch_idx, batch)| {
            let source = source.clone();
            let batch = batch.clone();
            tokio::spawn(async move {
                for i in batch.range() {
                    let key = format!("data/batch{}_item{}", batch_idx + 1, i);
                    let value = format!("batch{}_value_{}", batch_idx + 1, i);
                    source.set(&key, Some(&value)).await;
                    sleep(Duration::from_millis(batch.update_delay_ms)).await;
                }
            })
        })
        .collect();

    let update_task_existing = tokio::spawn({
        let source = source.clone();
        let existing = CONFIG.existing_items.clone();
        async move {
            // Update existing items using batch configuration
            for i in existing.range() {
                let key = format!("data/item{i}");
                let value = format!("updated_value_{i}");
                source.set(&key, Some(&value)).await;
                sleep(Duration::from_millis(existing.update_delay_ms)).await;
            }
        }
    });

    // 6. Wait for all concurrent updates to complete
    for task in update_tasks {
        task.await.unwrap();
    }
    update_task_existing.await.unwrap();

    // 7. Wait for synchronization across all caches
    sleep(Duration::from_millis(CONFIG.sync_wait_ms)).await;

    // 8. Verify all caches have the same final state
    let entries_a = cache_a.try_list_dir("data").await?;
    let entries_b = cache_b.try_list_dir("data").await?;
    let entries_c = cache_c.try_list_dir("data").await?;

    // All caches should have the same number of entries
    let batch_items_total: usize = CONFIG.batches.iter().map(|b| b.span).sum();
    let expected_count = 3 + batch_items_total; // 3 initial + all batch items
    assert_eq!(
        entries_a.len(),
        expected_count,
        "Cache A should have {expected_count} entries"
    );
    assert_eq!(
        entries_b.len(),
        expected_count,
        "Cache B should have {expected_count} entries"
    );
    assert_eq!(
        entries_c.len(),
        expected_count,
        "Cache C should have {expected_count} entries"
    );

    println!("All caches have {expected_count} entries");

    // 9. Verify all caches have identical data for each key
    let mut all_keys = std::collections::HashSet::new();
    for entry in &entries_a {
        all_keys.insert(entry.0.clone());
    }

    for key in &all_keys {
        let val_a = cache_a.try_get(key).await?.unwrap();
        let val_b = cache_b.try_get(key).await?.unwrap();
        let val_c = cache_c.try_get(key).await?.unwrap();

        // All caches should have identical data and sequence for the same key
        assert_eq!(val_a.data, val_b.data, "Data mismatch for key {key}");
        assert_eq!(val_a.data, val_c.data, "Data mismatch for key {key}");

        assert_eq!(val_a.seq, val_b.seq, "Sequence mismatch for key {key}");
        assert_eq!(val_a.seq, val_c.seq, "Sequence mismatch for key {key}");
    }

    println!("All caches have identical data for all keys");

    // 10. Verify updated values are correct
    let item1_a = cache_a.try_get("data/item1").await?.unwrap();
    assert_eq!(item1_a.data, "updated_value_1");

    let item2_b = cache_b.try_get("data/item2").await?.unwrap();
    assert_eq!(item2_b.data, "updated_value_2");

    let item3_c = cache_c.try_get("data/item3").await?.unwrap();
    assert_eq!(item3_c.data, "updated_value_3");

    // 11. Verify batch data is present in all caches
    for (batch_idx, batch) in CONFIG.batches.iter().enumerate() {
        for i in batch.start..batch.start + batch.span {
            let key = format!("data/batch{}_item{}", batch_idx + 1, i);
            let expected_value = format!("batch{}_value_{}", batch_idx + 1, i);

            let val_a = cache_a.try_get(&key).await?.unwrap();
            let val_b = cache_b.try_get(&key).await?.unwrap();
            let val_c = cache_c.try_get(&key).await?.unwrap();

            assert_eq!(val_a.data, expected_value);
            assert_eq!(val_b.data, expected_value);
            assert_eq!(val_c.data, expected_value);
        }
    }

    // 12. Check sequence consistency across all caches
    let seq_a = cache_a.try_last_seq().await?;
    let seq_b = cache_b.try_last_seq().await?;
    let seq_c = cache_c.try_last_seq().await?;

    assert_eq!(seq_a, seq_b, "Cache A and B should have same last sequence");
    assert_eq!(seq_a, seq_c, "Cache A and C should have same last sequence");

    println!("All caches have same last sequence: {seq_a}");

    // 13. Test rapid concurrent operations with all caches active
    let rapid_updates = tokio::spawn({
        let source = source.clone();
        let rapid = CONFIG.rapid.clone();
        async move {
            for i in rapid.start..rapid.start + rapid.span {
                let key = format!("data/rapid{i}");
                let value = format!("rapid_value_{i}");
                source.set(&key, Some(&value)).await;
                sleep(Duration::from_millis(rapid.update_delay_ms)).await;
            }
        }
    });

    // Concurrent reads from all caches while updates are happening
    let read_task_a = tokio::spawn({
        let mut cache_a = cache_a;
        async move {
            for _ in 0..CONFIG.read_task_iterations {
                let _ = cache_a.try_list_dir("data").await;
                let _ = cache_a.try_get("data/item1").await;
                sleep(Duration::from_millis(CONFIG.read_task_delays_ms[0])).await;
            }
            cache_a
        }
    });

    let read_task_b = tokio::spawn({
        let mut cache_b = cache_b;
        async move {
            for _ in 0..CONFIG.read_task_iterations {
                let _ = cache_b.try_list_dir("data").await;
                let _ = cache_b.try_get("data/item2").await;
                sleep(Duration::from_millis(CONFIG.read_task_delays_ms[1])).await;
            }
            cache_b
        }
    });

    let read_task_c = tokio::spawn({
        let mut cache_c = cache_c;
        async move {
            for _ in 0..CONFIG.read_task_iterations {
                let _ = cache_c.try_list_dir("data").await;
                let _ = cache_c.try_get("data/item3").await;
                sleep(Duration::from_millis(CONFIG.read_task_delays_ms[2])).await;
            }
            cache_c
        }
    });

    // Wait for all tasks to complete
    let (_, cache_a, cache_b, cache_c) =
        tokio::join!(rapid_updates, read_task_a, read_task_b, read_task_c);

    let mut cache_a = cache_a?;
    let mut cache_b = cache_b?;
    let mut cache_c = cache_c?;

    // Final synchronization wait
    sleep(Duration::from_millis(CONFIG.final_sync_wait_ms)).await;

    // 14. Final verification - all caches should have the rapid updates
    let final_entries_a = cache_a.try_list_dir("data").await?;
    let final_entries_b = cache_b.try_list_dir("data").await?;
    let final_entries_c = cache_c.try_list_dir("data").await?;

    let final_expected_count = 3 + batch_items_total + CONFIG.rapid.span; // 3 initial + batches + rapid
    assert_eq!(final_entries_a.len(), final_expected_count);
    assert_eq!(final_entries_b.len(), final_expected_count);
    assert_eq!(final_entries_c.len(), final_expected_count);

    // Verify rapid updates are present in all caches
    for i in CONFIG.rapid.start..CONFIG.rapid.start + CONFIG.rapid.span {
        let key = format!("data/rapid{i}");
        let expected_value = format!("rapid_value_{i}");

        let val_a = cache_a.try_get(&key).await?.unwrap();
        let val_b = cache_b.try_get(&key).await?.unwrap();
        let val_c = cache_c.try_get(&key).await?.unwrap();

        assert_eq!(val_a.data, expected_value);
        assert_eq!(val_b.data, expected_value);
        assert_eq!(val_c.data, expected_value);

        // All should have same sequence for the same key
        assert_eq!(val_a.seq, val_b.seq);
        assert_eq!(val_a.seq, val_c.seq);
    }

    // 15. Final sequence consistency check
    let final_seq_a = cache_a.try_last_seq().await?;
    let final_seq_b = cache_b.try_last_seq().await?;
    let final_seq_c = cache_c.try_last_seq().await?;

    assert_eq!(final_seq_a, final_seq_b);
    assert_eq!(final_seq_a, final_seq_c);

    println!("Concurrent same-range watchers test completed successfully!");
    println!("All 3 caches maintained perfect synchronization:");
    println!("  - Final entries: {final_expected_count}");
    println!("  - Final sequence: {final_seq_a}");
    println!("  - All caches have identical data for all keys");
    println!("  - Concurrent reads and writes worked flawlessly");
    println!("Multiple watchers on same range stayed perfectly synchronized!");

    Ok(())
}

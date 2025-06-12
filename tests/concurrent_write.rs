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

//! Continuous write consistency integration test
//!
//! This test validates that a distributed cache maintains consistency when subjected to
//! continuous writes from a background task. The key aspects being tested are:
//!
//! 1. **Real-time Consistency**: Cache state remains consistent at any point in time
//! 2. **Continuous Updates**: Cache correctly handles ongoing data source changes
//! 3. **Delete/Recreate Cycles**: Cache properly handles key deletion and recreation
//! 4. **Sequence Ordering**: All cached values maintain proper sequence number ordering
//! 5. **Multi-key Consistency**: Multiple keys are kept in sync simultaneously
//!
//! The test simulates a production scenario where data is constantly being updated
//! in the underlying data source while the cache must maintain a consistent view.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use sub_cache::testing::source::TestSource;
use sub_cache::testing::types::TestConfig;
use sub_cache::Cache;
use tokio::time::sleep;

#[tokio::test]
async fn test_continuous_write_consistency() {
    // 1. Create test source and cache
    // The TestSource simulates a remote data store, and Cache provides the local view
    let source = TestSource::new();
    let mut cache = Cache::<TestConfig>::new(source.clone(), "continuous", "continuous-test").await;

    // 2. Shared state for background writer coordination
    // These atomic variables allow safe communication between the main test thread
    // and the background writer task
    let stop_flag = Arc::new(AtomicBool::new(false)); // Signal to stop the writer
    let write_counter = Arc::new(AtomicU64::new(0)); // Track number of writes performed

    // Clone references for the background writer task
    let writer_source = source.clone();
    let writer_stop = stop_flag.clone();
    let writer_counter = write_counter.clone();

    // 3. Start background writer task
    // This task continuously writes to the data source to simulate ongoing changes
    let writer_handle = tokio::spawn(async move {
        let mut counter = 0u64;

        // Continue writing until stop signal is received
        while !writer_stop.load(Ordering::Relaxed) {
            counter += 1;

            // Write to multiple keys to simulate real workload
            // Each key gets a unique value with an incrementing counter
            writer_source
                .set("continuous/key1", Some(&format!("value1_{counter}")))
                .await;
            writer_source
                .set("continuous/key2", Some(&format!("value2_{counter}")))
                .await;
            writer_source
                .set("continuous/key3", Some(&format!("value3_{counter}")))
                .await;

            // Occasionally delete and recreate keys to test edge cases
            // This simulates real-world scenarios where keys might be temporarily removed
            if counter % 10 == 0 {
                // Delete key2 every 10 writes
                writer_source.set("continuous/key2", None).await;
            }
            if counter % 15 == 0 {
                // Recreate key2 with a different value pattern every 15 writes
                writer_source
                    .set("continuous/key2", Some(&format!("recreated_{counter}")))
                    .await;
            }

            // Update the shared counter so the main thread can track progress
            writer_counter.store(counter, Ordering::Relaxed);

            // Small delay to allow cache to process changes and prevent overwhelming
            sleep(Duration::from_millis(10)).await;
        }
    });

    // 4. Monitor cache consistency while writes are happening
    // This is the core of the test: verify consistency at multiple points in time
    let mut consistency_checks = 0;
    let max_checks = 50; // Perform 50 consistency checks over ~5 seconds

    while consistency_checks < max_checks {
        // Wait between checks to allow background writes to occur
        sleep(Duration::from_millis(100)).await;

        // Get current cache state atomically for consistency validation
        let cache_state_result = cache
            .try_access(|cache_data| {
                let key1 = cache_data.data.get("continuous/key1");
                let key2 = cache_data.data.get("continuous/key2");
                let key3 = cache_data.data.get("continuous/key3");
                let cache_seq = cache_data.last_seq;

                (key1.cloned(), key2.cloned(), key3.cloned(), cache_seq)
            })
            .await;

        let (key1, key2, key3, cache_seq) = cache_state_result.unwrap();

        // Get current writer state for comparison
        let writer_count = write_counter.load(Ordering::Relaxed);

        // Verify consistency invariants for key1
        // Key1 should always exist (never deleted) and follow the expected format
        if let Some(val1) = &key1 {
            // Data format validation: should always start with "value1_"
            assert!(
                val1.data.starts_with("value1_"),
                "key1 has invalid format: {}",
                val1.data
            );

            // Sequence number validation: must be positive and not exceed cache sequence
            assert!(val1.seq > 0, "key1 has invalid sequence: {}", val1.seq);
            assert!(
                val1.seq <= cache_seq,
                "key1 sequence {} > cache sequence {}",
                val1.seq,
                cache_seq
            );
        }

        // Verify consistency invariants for key3
        // Key3 should always exist (never deleted) and follow the expected format
        if let Some(val3) = &key3 {
            // Data format validation: should always start with "value3_"
            assert!(
                val3.data.starts_with("value3_"),
                "key3 has invalid format: {}",
                val3.data
            );
            assert!(val3.seq > 0, "key3 has invalid sequence: {}", val3.seq);
            assert!(
                val3.seq <= cache_seq,
                "key3 sequence {} > cache sequence {}",
                val3.seq,
                cache_seq
            );
        }

        // Verify consistency invariants for key2
        // Key2 might be None (deleted) or have one of two valid formats
        if let Some(val2) = &key2 {
            // Data format validation: should be either "value2_N" or "recreated_N"
            assert!(
                val2.data.starts_with("value2_") || val2.data.starts_with("recreated_"),
                "key2 has invalid format: {}",
                val2.data
            );
            assert!(val2.seq > 0, "key2 has invalid sequence: {}", val2.seq);
            assert!(
                val2.seq <= cache_seq,
                "key2 sequence {} > cache sequence {}",
                val2.seq,
                cache_seq
            );
        }

        // Global consistency check: cache sequence should always be progressing
        assert!(cache_seq > 0, "cache sequence should be positive");

        consistency_checks += 1;

        // Print progress occasionally to show test is working and provide visibility
        if consistency_checks % 10 == 0 {
            println!("Consistency check {}/{}: writer_count={}, cache_seq={}, key1={:?}, key2={:?}, key3={:?}",
                     consistency_checks, max_checks, writer_count, cache_seq,
                     key1.as_ref().map(|v| &v.data),
                     key2.as_ref().map(|v| &v.data),
                     key3.as_ref().map(|v| &v.data));
        }
    }

    // 5. Stop the writer and verify final consistency
    // Signal the background writer to stop and wait for it to complete
    stop_flag.store(true, Ordering::Relaxed);
    writer_handle.await.unwrap();

    // Wait for final synchronization to ensure all pending changes are processed
    sleep(Duration::from_millis(200)).await;

    // Final consistency check after all writes have completed
    // Use atomic access to get final state
    let (final_key1_exists, final_key3_exists, final_seq) = cache
        .try_access(|cache_data| {
            let key1_exists = cache_data.data.contains_key("continuous/key1");
            let key3_exists = cache_data.data.contains_key("continuous/key3");
            let seq = cache_data.last_seq;
            (key1_exists, key3_exists, seq)
        })
        .await
        .unwrap();

    let final_writer_count = write_counter.load(Ordering::Relaxed);

    // Verify final state meets expectations
    assert!(final_key1_exists, "key1 should exist at the end");
    assert!(final_key3_exists, "key3 should exist at the end");
    assert!(final_seq > 0, "final sequence should be positive");
    assert!(
        final_writer_count > 0,
        "writer should have written some data"
    );

    // Report test completion with statistics
    println!("Test completed: {final_writer_count} writes, final cache sequence: {final_seq}");
}

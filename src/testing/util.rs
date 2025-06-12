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

//! Testing utilities for cache state assertions.

use std::time::Duration;

use tokio::time::sleep;

use crate::errors::Unsupported;
use crate::testing::source::Val;
use crate::testing::types::TestConfig;
use crate::Cache;

/// Check if cache has expected sequence number and key-value pairs.
///
/// Returns `Ok(Ok(()))` on match, `Ok(Err(msg))` on mismatch, `Err(Unsupported)` on cache failure.
pub async fn check_cache_state(
    cache: &mut Cache<TestConfig>,
    expected_seq: u64,
    expected_kvs: &[(&str, Option<Val>)],
) -> Result<Result<(), String>, Unsupported> {
    // Use try_access for atomic access to cache data
    cache
        .try_access(|cache_data| {
            // Check last sequence number
            let actual_seq = cache_data.last_seq;
            if actual_seq != expected_seq {
                return Err(format!(
                    "Cache sequence mismatch: expected {}, got {}",
                    expected_seq, actual_seq
                ));
            }

            // Check each key-value pair
            for (key, expected_value) in expected_kvs {
                let actual_value = cache_data.data.get(*key);
                if actual_value != expected_value.as_ref() {
                    return Err(format!(
                        "Cache value mismatch for key '{}': expected {:?}, got {:?}",
                        key, expected_value, actual_value
                    ));
                }
            }

            Ok(())
        })
        .await
}

/// Retry [`check_cache_state`] with configurable attempts and delay.
///
/// Useful for waiting for cache synchronization in tests. Returns the same result types as `check_cache_state`.
pub async fn retry_check_cache_state(
    cache: &mut Cache<TestConfig>,
    expected_seq: u64,
    expected_kvs: &[(&str, Option<Val>)],
    max_attempts: u32,
    delay: Duration,
) -> Result<Result<(), String>, Unsupported> {
    let mut last_error = String::new();

    for attempt in 1..=max_attempts {
        match check_cache_state(cache, expected_seq, expected_kvs).await {
            Ok(Ok(())) => return Ok(Ok(())),
            Ok(Err(error_msg)) => {
                last_error = error_msg;
                if attempt < max_attempts {
                    sleep(delay).await;
                }
            }
            Err(unsupported_error) => {
                return Err(unsupported_error);
            }
        }
    }

    Ok(Err(format!(
        "Cache state check failed after {} attempts. Last error: {}",
        max_attempts, last_error
    )))
}

/// Alias for [`retry_check_cache_state`] with default parameters (20 attempts, 50ms delay).
pub async fn wait_for_cache_state(
    cache: &mut Cache<TestConfig>,
    expected_seq: u64,
    expected_kvs: &[(&str, Option<Val>)],
) -> Result<Result<(), String>, Unsupported> {
    retry_check_cache_state(
        cache,
        expected_seq,
        expected_kvs,
        20,
        Duration::from_millis(50),
    )
    .await
}

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

use futures::FutureExt;
use log::debug;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic;
use tokio::sync::Mutex;
use tokio::sync::MutexGuard;
use tokio::sync::oneshot;

use crate::TypeConfig;
use crate::cache_data::CacheData;
use crate::errors::Unsupported;
use crate::event_watcher::EventWatcher;

/// Cache implemented on top of the distributed remote data store.
///
/// This cache provides a local view of data stored in the remote data store, with automatic
/// background updates when the underlying data changes.
///
/// ## Features
///
/// - **Automatic Synchronization**: Background watcher task keeps local cache in sync with remote data store
/// - **Concurrency Control**: Two-level concurrency control mechanism for safe access
/// - **Safe Reconnection**: Automatic recovery from connection failures with state consistency
/// - **Consistent Initialization**: Ensures cache is fully initialized before use
///
/// ## Concurrency Control
///
/// The cache employs a two-level concurrency control mechanism:
///
/// 1. **Internal Lock (Mutex)**: Protects concurrent access between user operations and the
///    background cache updater. This lock is held briefly during each operation.
///
/// 2. **External Lock (Method Design)**: Public methods require `&mut self` even for read-only
///    operations. This prevents concurrent access to the cache instance from multiple call sites.
///    External synchronization should be implemented by the caller if needed.
///
/// This design intentionally separates concerns:
/// - The internal lock handles short-term, fine-grained synchronization with the updater
/// - The external lock requirement (`&mut self`) enables longer-duration access patterns
///   without blocking the background updater unnecessarily
///
/// Note that despite requiring `&mut self`, all operations are logically read-only
/// with respect to the cache's public API.
///
/// ## Error Handling
///
/// - Background watcher task automatically recovers from errors by:
///   - Resetting the cache state
///   - Re-establishing the watch stream
///   - Re-fetching all data to ensure consistency
/// - Users are shielded from transient errors through the abstraction
pub struct Cache<C: TypeConfig> {
    /// The dir path to store the cache ids, without trailing slash.
    ///
    /// Such as `foo`, not `foo/`
    prefix: String,

    /// The sender to cancel the background watcher task.
    ///
    /// When this sender is dropped, the corresponding receiver becomes ready,
    /// which signals the background task to terminate gracefully.
    #[allow(dead_code)]
    watcher_cancel_tx: oneshot::Sender<()>,

    data: Arc<Mutex<Result<CacheData<C>, Unsupported>>>,

    /// A process-wide unique identifier for the cache. Used for debugging purposes.
    uniq: u64,

    /// The name for this cache instance, for debugging.
    name: String,
}

impl<C> fmt::Display for Cache<C>
where
    C: TypeConfig,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Cache({})({}/)[uniq={}]",
            self.name, self.prefix, self.uniq
        )
    }
}

impl<C> Cache<C>
where
    C: TypeConfig,
{
    /// Create a new cache.
    ///
    /// The created cache starts to watch key-value change event.
    /// It does not return until initialization is started.
    /// Thus, it is safe to access the data once this method is returned, because initialization holds a lock.
    ///
    /// # Parameters
    ///
    /// * `source` - The client to interact with the remote data store.
    /// * `prefix` - The prefix of the cache name and also the directory name to store in remote data store.
    /// * `ctx` - The context info of the cache, used for debugging purposes.
    ///
    /// This method spawns a background task to watch to the remote data store key value change events.
    /// The task will be notified to quit when this instance is dropped.
    pub async fn new(source: C::Source, prefix: impl ToString, name: impl ToString) -> Self {
        let prefix = prefix.to_string();
        let prefix = prefix.trim_end_matches('/').to_string();

        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();

        static UNIQ: atomic::AtomicU64 = atomic::AtomicU64::new(0);
        let uniq = UNIQ.fetch_add(1, atomic::Ordering::SeqCst);

        let mut cache = Cache {
            prefix,
            watcher_cancel_tx: cancel_tx,
            data: Arc::new(Mutex::new(Err(Unsupported::new("Cache not initialized")))),
            uniq,
            name: name.to_string(),
        };

        cache.spawn_watcher_task(source, cancel_rx).await;

        cache
    }

    /// Get a SeqV from the cache by key.
    pub async fn try_get(&mut self, key: &str) -> Result<Option<C::Value>, Unsupported> {
        debug!("Cache::access: try_get({})", key);
        self.try_access(|cache_data| cache_data.data.get(key).cloned())
            .await
    }

    /// Get the last sequence number of the cache.
    pub async fn try_last_seq(&mut self) -> Result<u64, Unsupported> {
        self.try_access(|cache_data| cache_data.last_seq).await
    }

    /// List all entries in the cache directory.
    pub async fn try_list_dir(
        &mut self,
        prefix: &str,
    ) -> Result<Vec<(String, C::Value)>, Unsupported> {
        let prefix = prefix.trim_end_matches('/');
        let left = format!("{}/", prefix);
        let right = format!("{}0", prefix);

        debug!("Cache::access: try_list_dir({})", prefix);

        self.try_access(|cache_data| {
            cache_data
                .data
                .range(left..right)
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect()
        })
        .await
    }

    /// Get the internal cache data.
    pub async fn cache_data(&mut self) -> MutexGuard<'_, Result<CacheData<C>, Unsupported>> {
        self.data.lock().await
    }

    /// Access the cache data in read-only mode.
    pub async fn try_access<T>(
        &mut self,
        f: impl FnOnce(&CacheData<C>) -> T,
    ) -> Result<T, Unsupported> {
        let guard = self.data.lock().await;

        let g = guard.as_ref().map_err(|e| e.clone())?;

        let t = f(g);
        Ok(t)
    }

    /// Spawns a background task to watch to the remote data store key value change events, feed to the cache.
    ///
    /// It does not return until a full copy of the cache is received.
    async fn spawn_watcher_task(&mut self, source: C::Source, cancel_rx: oneshot::Receiver<()>) {
        let (left, right) = self.key_range();

        let watcher_name = format!("{}-watcher", self);
        let watcher = EventWatcher {
            left,
            right,
            source,
            data: self.data.clone(),
            name: watcher_name.to_string(),
        };

        // For receiving a signal when the cache has started to initialize and safe to use:
        // i.e., if the user acquired the data lock, they can see a complete view of the data(fully initialized).
        let (started_tx, started_rx) = oneshot::channel::<()>();

        let task_name = watcher_name.to_string();
        let fu = watcher.main(Some(started_tx), cancel_rx.map(|_| ()));

        C::spawn(fu, task_name);

        // Wait for the sending end to be dropped, indicating that the cache has started to initialize.
        started_rx.await.ok();
    }

    /// The left-close right-open range for the cached keys.
    ///
    /// Since `'0'` is the next char of `'/'`.
    /// `[prefix + "/", prefix + "0")` is the range of the cache ids.
    fn key_range(&self) -> (String, String) {
        let left = self.prefix.clone() + "/";
        let right = self.prefix.clone() + "0";

        (left, right)
    }
}

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

use std::fmt;
use std::sync::atomic;
use std::sync::Arc;

use futures::FutureExt;
use log::debug;
use tokio::sync::oneshot;
use tokio::sync::RwLock;
use tokio::sync::RwLockReadGuard;

use crate::cache_data::CacheData;
use crate::errors::Unsupported;
use crate::event_watcher::EventWatcher;
use crate::TypeConfig;

/// Cache implemented on top of the distributed remote data store.
///
/// This cache provides a local view of data stored in the remote data store, with automatic
/// background updates when the underlying data changes.
///
/// ## Features
///
/// - **Automatic Synchronization**: Background watcher task keeps local cache in sync with remote data store
/// - **Concurrency Control**: Readers share the data; reads take `&self`
/// - **Safe Reconnection**: Automatic recovery from connection failures with state consistency
/// - **Consistent Initialization**: Ensures cache is fully initialized before use
///
/// ## Concurrency Control
///
/// The data is behind a read-write lock. Readers share it; the background watcher holds it
/// exclusively, briefly to apply each change, and for the whole re-fetch while it initializes
/// or reconnects.
///
/// Every read takes `&self`, so a cache is shared by reference or `Arc` without an outer lock:
///
/// - `try_get`, `try_list_dir`, `try_last_seq` and `try_access` wait while the watcher holds the
///   data, e.g. until the cache is initialized.
/// - `try_get_now`, `try_list_dir_now` and `try_access_now` never wait: they return `None` while
///   the watcher holds the data, and the caller reads the remote data store instead.
///
/// ## Error Handling
///
/// - Background watcher task automatically recovers from errors by:
///   - Re-establishing the watch stream
///   - Re-fetching all data to ensure consistency
///   - Replacing the cache data only with a complete initial flush: a flush that fails part
///     way is never served
/// - Users are shielded from transient errors through the abstraction
pub struct Cache<C: TypeConfig> {
    /// The cached directory, used as given: the cache holds the keys under `{dir}/`.
    ///
    /// `foo` is the directory `foo`, and `foo/` the empty-named directory inside it.
    dir: String,

    /// The sender to cancel the background watcher task.
    ///
    /// When this sender is dropped, the corresponding receiver becomes ready,
    /// which signals the background task to terminate gracefully.
    #[allow(dead_code)]
    watcher_cancel_tx: oneshot::Sender<()>,

    data: Arc<RwLock<Result<CacheData<C>, Unsupported>>>,

    /// A process-wide unique identifier for the cache. Used for debugging purposes.
    uniq: u64,

    /// The name for this cache instance, for debugging.
    name: String,
}

impl<C> fmt::Display for Cache<C>
where C: TypeConfig
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Cache({})({}/)[uniq={}]", self.name, self.dir, self.uniq)
    }
}

impl<C> Cache<C>
where C: TypeConfig
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
    /// * `dir` - The directory to cache, used as given: the cache holds the keys under `{dir}/`.
    ///   `foo` is the directory `foo`, and `foo/` the empty-named directory inside it, whose keys
    ///   are `foo//...`. A trailing `/` is never stripped.
    /// * `name` - The name of the cache, used for debugging purposes.
    ///
    /// This method spawns a background task to watch to the remote data store key value change events.
    /// The task will be notified to quit when this instance is dropped.
    pub async fn new(source: C::Source, dir: impl ToString, name: impl ToString) -> Self {
        let dir = dir.to_string();

        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();

        static UNIQ: atomic::AtomicU64 = atomic::AtomicU64::new(0);
        let uniq = UNIQ.fetch_add(1, atomic::Ordering::SeqCst);

        let mut cache = Cache {
            dir,
            watcher_cancel_tx: cancel_tx,
            data: Arc::new(RwLock::new(Err(Unsupported::new("Cache not initialized")))),
            uniq,
            name: name.to_string(),
        };

        cache.spawn_watcher_task(source, cancel_rx).await;

        cache
    }

    /// Get a SeqV from the cache by key.
    pub async fn try_get(&self, key: &str) -> Result<Option<C::Value>, Unsupported> {
        debug!("Cache::access: try_get({})", key);
        self.try_access(|cache_data| cache_data.data.get(key).cloned())
            .await
    }

    /// Get the last sequence number of the cache.
    pub async fn try_last_seq(&self) -> Result<u64, Unsupported> {
        self.try_access(|cache_data| cache_data.last_seq).await
    }

    /// List the entries under the directory `dir`, the keys under `{dir}/`.
    ///
    /// `dir` is used as given, as in [`Cache::new`]: `foo/` lists the keys `foo//...` of the
    /// empty-named directory inside `foo`.
    pub async fn try_list_dir(&self, dir: &str) -> Result<Vec<(String, C::Value)>, Unsupported> {
        debug!("Cache::access: try_list_dir({})", dir);
        self.try_access(|cache_data| list_dir(cache_data, dir))
            .await
    }

    /// Get the internal cache data, for reading.
    pub async fn cache_data(&self) -> RwLockReadGuard<'_, Result<CacheData<C>, Unsupported>> {
        self.data.read().await
    }

    /// Access the cache data in read-only mode.
    pub async fn try_access<T>(
        &self,
        f: impl FnOnce(&CacheData<C>) -> T,
    ) -> Result<T, Unsupported> {
        let guard = self.data.read().await;

        let g = guard.as_ref().map_err(|e| e.clone())?;

        let t = f(g);
        Ok(t)
    }

    /// Run `f` on the cache data if the cache can answer now, without waiting.
    ///
    /// `None` while the background watcher holds the data exclusively: while it initializes or
    /// reconnects, or applies a change. `Some(Err(_))` if the remote data store cannot be
    /// subscribed. The caller reads the remote data store instead of the cache on `None`.
    ///
    /// Readers share the data, so concurrent reads do not push each other away. The read is not
    /// a future: it answers whether or not the task's tokio coop budget is spent.
    pub fn try_access_now<T>(
        &self,
        f: impl FnOnce(&CacheData<C>) -> T,
    ) -> Option<Result<T, Unsupported>> {
        let guard = self.data.try_read().ok()?;
        Some(guard.as_ref().map(f).map_err(|e| e.clone()))
    }

    /// The value of `key` if the cache can answer now, see [`Self::try_access_now`].
    pub fn try_get_now(&self, key: &str) -> Option<Result<Option<C::Value>, Unsupported>> {
        self.try_access_now(|cache_data| cache_data.data.get(key).cloned())
    }

    /// The entries under the directory `dir` if the cache can answer now, see
    /// [`Self::try_access_now`] and [`Self::try_list_dir`].
    #[allow(clippy::type_complexity)]
    pub fn try_list_dir_now(
        &self,
        dir: &str,
    ) -> Option<Result<Vec<(String, C::Value)>, Unsupported>> {
        self.try_access_now(|cache_data| list_dir(cache_data, dir))
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
    /// `[dir + "/", dir + "0")` is the range of the cache ids.
    fn key_range(&self) -> (String, String) {
        let left = self.dir.clone() + "/";
        let right = self.dir.clone() + "0";

        (left, right)
    }
}

/// The entries under the directory `dir`: the keys under `{dir}/`, `dir` used as given.
fn list_dir<C: TypeConfig>(cache_data: &CacheData<C>, dir: &str) -> Vec<(String, C::Value)> {
    let left = format!("{}/", dir);
    let right = format!("{}0", dir);
    cache_data
        .data
        .range(left..right)
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

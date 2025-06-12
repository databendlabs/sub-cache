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

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt;
use futures::Stream;
use futures::TryStreamExt;
use log::debug;
use log::error;
use log::info;
use log::warn;
use tokio::sync::oneshot;
use tokio::sync::Mutex;

use crate::cache_data::CacheData;
use crate::errors::ConnectionClosed;
use crate::errors::SubscribeError;
use crate::errors::Unsupported;
use crate::event_stream::Event;
use crate::event_stream::EventStream;
use crate::Source;
use crate::TypeConfig;

/// Watch cache events and update local copy.
pub(crate) struct EventWatcher<C: TypeConfig> {
    /// The left-closed bound of the key range to watch.
    pub(crate) left: String,

    /// The right-open bound of the key range to watch.
    pub(crate) right: String,

    /// The client to interact with the remote data store.
    pub(crate) source: C::Source,

    /// The shared cache data protected by a mutex.
    pub(crate) data: Arc<Mutex<Result<CacheData<C>, Unsupported>>>,

    /// Contains descriptive information of this watcher.
    pub(crate) name: String,
}

impl<C> EventWatcher<C>
where C: TypeConfig
{
    /// Subscribe to the key-value changes in the interested range and feed them into the local cache.
    ///
    /// This method continuously monitors the remote data store for changes within the specified key range
    /// and updates the local cache accordingly.
    ///
    /// # Parameters
    ///
    /// - `started` - An optional oneshot channel sender that is consumed when the cache initialization
    ///   begins. This signals that it's safe for users to acquire the cache data lock.
    /// - `cancel` - A future that, when completed, will terminate the subscription loop.
    ///
    /// # Error Handling
    ///
    /// - Connection failures are automatically retried with exponential backoff
    /// - On error, the cache is reset and re-fetched to ensure consistency
    /// - The watcher continues running until explicitly canceled
    pub(crate) async fn main(
        mut self,
        mut started: Option<oneshot::Sender<()>>,
        cancel: impl Future<Output = ()> + Send + 'static,
    ) {
        // sleep time and reason
        let mut sleep = None::<(Duration, String)>;

        let mut c = std::pin::pin!(cancel);

        loop {
            if let Some((sleep_time, reason)) = sleep.take() {
                info!(
                    "{}: retry establish cache watcher in {:?} because {}",
                    self.name, sleep_time, reason
                );
                tokio::time::sleep(sleep_time).await;
            }

            // 1. Retry until a successful connection is established and cache is initialized.
            let strm = {
                // Hold the lock until the cache is fully initialized.
                let mut cache_data = self.data.lock().await;

                // The data lock is acquired and will be kept until the cache is fully initialized.
                // At this point, we notify the caller that initialization has started by consuming
                // the `started` sender. This signals to the receiving end that it's now safe to
                // acquire the data lock, as we're about to populate the cache with initial data.
                started.take();

                let strm_res = self.retry_new_stream().await;

                // Everytime when establishing a cache, the old data must be cleared and receive a new one.
                *cache_data = Ok(Default::default());

                let mut strm = match strm_res {
                    Ok(strm) => {
                        info!("{}: cache watch stream established", self.name);
                        strm
                    }
                    Err(unsupported) => {
                        let sleep_time = Duration::from_secs(60 * 5);

                        let mes = format!(
                            "{}: watch stream not supported: {}; retry in {:?}",
                            self.name, unsupported, sleep_time
                        );

                        warn!("{}", mes);
                        sleep = Some((sleep_time, mes));

                        // Mark the cache as unavailable
                        *cache_data = Err(unsupported);
                        continue;
                    }
                };

                let init_res = {
                    // Safe unwrap: before entering this method, the cache data is ensured to be Ok.
                    let d = cache_data.as_mut().unwrap();
                    self.initialize_cache(d, &mut strm).await
                };

                match init_res {
                    Ok(_) => {
                        info!("{}: cache initialized successfully", self.name);
                        strm
                    }
                    Err(conn_err) => {
                        error!(
                            "{}: cache initialization failed: {}; retry re-establish cache",
                            self.name, conn_err
                        );
                        continue;
                    }
                }
            };

            // 2. Watch for changes in the stream and apply them to the local cache.
            let res = self.watch_kv_changes(strm, c.as_mut()).await;

            match res {
                Ok(_) => {
                    info!("{} watch loop exited normally(canceled by user)", self.name);
                    return;
                }
                Err(e) => {
                    error!(
                        "{} watcher loop exited with error: {}; reset cache and re-fetch all data to re-establish",
                        self.name, e
                    );
                    // continue
                }
            }
        }
    }

    /// Drain all initialization events from the watch stream and apply them to the cache.
    async fn initialize_cache(
        &self,
        cache_data: &mut CacheData<C>,
        strm: &mut EventStream<C::Value>,
    ) -> Result<(), ConnectionClosed> {
        while let Some(event) = strm.try_next().await? {
            let change = match event {
                Event::Initialization(change) => change,
                Event::InitializationComplete => {
                    info!(
                        "{}: cache is ready, initial_flush finished upto seq={}",
                        self.name, cache_data.last_seq
                    );
                    break;
                }
                Event::Change(ref _change) => {
                    unreachable!(
                        "expected only initialization events, got change event: {:?}",
                        event
                    );
                }
            };

            let (key, before, after) = change.unpack();

            cache_data.apply_update(key, before, after);
        }

        Ok(())
    }

    /// Keep retrying to establish a new watch stream until a successful one is established, or the server
    /// reports that the watch stream is not supported.
    async fn retry_new_stream(&self) -> Result<EventStream<C::Value>, Unsupported> {
        let mut sleep_duration = Duration::from_millis(50);
        let max_sleep = Duration::from_secs(5);

        loop {
            let res = self.source.subscribe(&self.left, &self.right).await;

            let conn_err = match res {
                Ok(stream) => return Ok(stream),
                Err(SubscribeError::Unsupported(u)) => return Err(u),
                Err(SubscribeError::Connection(c)) => c,
            };

            error!(
                "{}: while establish cache, error: {}; retrying in {:?}",
                self.name, conn_err, sleep_duration
            );

            tokio::time::sleep(sleep_duration).await;
            sleep_duration = std::cmp::min(sleep_duration * 3 / 2, max_sleep);
        }
    }

    /// The main loop of the cache engine.
    ///
    /// This function watches for key-value changes in the remote data store and processes them.
    /// Changes are applied to the local in-memory cache atomically.
    ///
    /// # Arguments
    ///
    /// * `strm` - The watch stream from the remote data store
    /// * `cancel` - A future that, when ready, signals this loop to terminate
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if terminated normally (i.e., the `cancel` future is ready), or
    /// `Err(ConnectionClosed)` if the remote connection was closed unexpectedly.
    pub(crate) async fn watch_kv_changes(
        &mut self,
        mut strm: impl Stream<Item = Result<Event<C::Value>, ConnectionClosed>> + Send + Unpin + 'static,
        cancel: impl Future<Output = ()> + Send,
    ) -> Result<(), ConnectionClosed> {
        let mut c = std::pin::pin!(cancel);

        loop {
            let event_result = futures::select! {
                _ = c.as_mut().fuse() => {
                    info!("cache loop canceled by user");
                    return Ok(());
                }

                watch_result = strm.try_next().fuse() => {
                    watch_result
                }
            };

            let Some(event) = event_result? else {
                error!("{} watch-stream closed", self.name);
                return Err(ConnectionClosed::new_str("watch-stream closed").context(&self.name));
            };

            let (key, before, after) = match event {
                Event::Change(change) => change.unpack(),
                _ => {
                    unreachable!(
                        "{}: expected only change events, got: {:?}",
                        self.name, event
                    );
                }
            };

            let mut cache_data = self.data.lock().await;

            // Safe unwrap: before entering this method, the cache data is ensured to be Ok.
            let d = cache_data.as_mut().unwrap();

            let new_seq = d.apply_update(key.clone(), before.clone(), after.clone());

            debug!(
                "{}: process update(key: {}, prev: {:?}, current: {:?}), new_seq={:?}",
                self.name, key, before, after, new_seq
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use super::*;
    use crate::testing::source::TestSource;
    use crate::testing::source::Val;

    // Test TypeConfig
    #[derive(Debug, Default)]
    struct TestConfig;

    impl TypeConfig for TestConfig {
        type Value = Val;
        type Source = TestSource;

        fn value_seq(value: &Self::Value) -> u64 {
            value.seq
        }

        fn spawn<F>(future: F, _name: impl ToString)
        where
            F: Future + Send + 'static,
            F::Output: Send + 'static,
        {
            tokio::spawn(future);
        }
    }

    #[test]
    fn test_event_watcher_construction() {
        let test_source = TestSource::new();

        let watcher = EventWatcher::<TestConfig> {
            left: "start".to_string(),
            right: "end".to_string(),
            source: test_source,
            data: Arc::new(Mutex::new(Ok(CacheData::default()))),
            name: "test-watcher".to_string(),
        };

        assert_eq!(watcher.left, "start");
        assert_eq!(watcher.right, "end");
        assert_eq!(watcher.name, "test-watcher");
    }

    #[tokio::test]
    async fn test_real_event_watcher_integration() {
        let test_source = TestSource::new();

        // 通过TestSource插入初始数据
        test_source
            .insert_for_test("initial_key", Val::new(1, "initial_value"))
            .await;

        let cache = Arc::new(Mutex::new(Ok(CacheData::default())));
        let watcher = EventWatcher::<TestConfig> {
            left: "".to_string(),
            right: "z".to_string(),
            source: test_source.clone(),
            data: cache.clone(),
            name: "test-integration".to_string(),
        };

        // 用oneshot作为cancel信号
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();

        // 包装cancel_rx为Future<Output=()>类型
        let cancel = async {
            let _ = cancel_rx.await;
        };

        // 启动EventWatcher::main在独立任务
        let handle = tokio::spawn(async move {
            watcher.main(None, cancel).await;
        });

        // 等待初始化完成
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 检查初始数据已同步到本地cache
        {
            let cache_data = cache.lock().await;
            let cache_data = cache_data.as_ref().unwrap();
            assert_eq!(
                cache_data.data.get("initial_key").unwrap(),
                &Val::new(1, "initial_value")
            );
            assert_eq!(cache_data.last_seq, 1);
        }

        // 通过TestSource触发事件
        test_source.set("key1", Some("value1")).await;
        test_source.set("key2", Some("value2")).await;
        test_source.set("key1", Some("updated_value1")).await;
        test_source.set("key2", None).await;

        // 等待事件被watcher处理
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 检查本地cache内容
        {
            let cache_data = cache.lock().await;
            let cache_data = cache_data.as_ref().unwrap();
            assert_eq!(
                cache_data.data.get("key1").unwrap(),
                &Val::new(3, "updated_value1")
            );
            assert!(!cache_data.data.contains_key("key2")); // 已删除
            assert_eq!(
                cache_data.data.get("initial_key").unwrap(),
                &Val::new(1, "initial_value")
            ); // 未变
            assert_eq!(cache_data.last_seq, 3); // key1的最大序列号
        }

        // 取消watcher任务
        let _ = cancel_tx.send(());
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_initialize_cache_only_initialization() {
        use futures::stream;

        use crate::event_stream::Change;
        use crate::event_stream::Event;

        // 构造事件流，包含所有类型Event
        let events = vec![
            Ok(Event::Initialization(Change::new(
                "k1",
                None,
                Some(Val::new(1, "v1")),
            ))),
            Ok(Event::Initialization(Change::new(
                "k2",
                None,
                Some(Val::new(2, "v2")),
            ))),
            Ok(Event::InitializationComplete),
            // InitializationComplete后即使还有事件也不应被处理
            Ok(Event::Change(Change::new(
                "should_also_not_be_seen",
                None,
                Some(Val::new(100, "fail2")),
            ))),
        ];
        let mut stream: crate::event_stream::EventStream<Val> = Box::pin(stream::iter(events));

        let watcher = EventWatcher::<TestConfig> {
            left: "".to_string(),
            right: "z".to_string(),
            source: TestSource::new(),
            data: Arc::new(Mutex::new(Ok(CacheData::default()))),
            name: "test-init-cache".to_string(),
        };

        let mut cache_data = CacheData::<TestConfig>::default();
        watcher
            .initialize_cache(&mut cache_data, &mut stream)
            .await
            .unwrap();

        // 断言cache_data被正确填充，且Change事件未被处理
        assert_eq!(cache_data.data.get("k1"), Some(&Val::new(1, "v1")));
        assert_eq!(cache_data.data.get("k2"), Some(&Val::new(2, "v2")));
        assert!(!cache_data.data.contains_key("should_not_be_seen"));
        assert!(!cache_data.data.contains_key("should_also_not_be_seen"));
        assert_eq!(cache_data.last_seq, 2);
    }

    #[tokio::test]
    async fn test_initialize_cache_error_event() {
        use futures::stream;

        use crate::errors::ConnectionClosed;
        use crate::event_stream::Change;
        use crate::event_stream::Event;

        // 构造事件流，包含Initialization、Err(ConnectionClosed)、InitializationComplete
        let events = vec![
            Ok(Event::Initialization(Change::new(
                "k1",
                None,
                Some(Val::new(1, "v1")),
            ))),
            Err(ConnectionClosed::new_str("test error")),
            Ok(Event::InitializationComplete),
        ];
        let mut stream: crate::event_stream::EventStream<Val> = Box::pin(stream::iter(events));

        let watcher = EventWatcher::<TestConfig> {
            left: "".to_string(),
            right: "z".to_string(),
            source: TestSource::new(),
            data: Arc::new(Mutex::new(Ok(CacheData::default()))),
            name: "test-init-cache-error".to_string(),
        };

        let mut cache_data = CacheData::<TestConfig>::default();
        let res = watcher.initialize_cache(&mut cache_data, &mut stream).await;

        // 断言返回了Err(ConnectionClosed)
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.to_string().contains("test error"));
    }

    #[tokio::test]
    async fn test_watch_kv_changes_basic() {
        use futures::stream;

        use crate::event_stream::Change;
        use crate::event_stream::Event;

        // 构造只包含Change事件的流
        let events = vec![
            Ok(Event::Change(Change::new(
                "k1",
                None,
                Some(Val::new(1, "v1")),
            ))),
            Ok(Event::Change(Change::new(
                "k2",
                None,
                Some(Val::new(2, "v2")),
            ))),
        ];
        let stream = stream::iter(events);

        let cache = Arc::new(Mutex::new(Ok(CacheData::default())));
        let mut watcher = EventWatcher::<TestConfig> {
            left: "".to_string(),
            right: "z".to_string(),
            source: TestSource::new(),
            data: cache.clone(),
            name: "test-watch-kv-changes".to_string(),
        };

        // 用oneshot作为cancel信号，测试不会提前退出
        let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let cancel = async {
            let _ = cancel_rx.await;
        };

        // 执行watch_kv_changes
        let res = watcher.watch_kv_changes(stream, cancel).await;
        // 由于流会结束，应该返回Err(ConnectionClosed)
        assert!(res.is_err());

        // 检查cache内容
        let cache_data = cache.lock().await;
        let cache_data = cache_data.as_ref().unwrap();
        assert_eq!(cache_data.data.get("k1"), Some(&Val::new(1, "v1")));
        assert_eq!(cache_data.data.get("k2"), Some(&Val::new(2, "v2")));
        assert_eq!(cache_data.last_seq, 2);
    }

    #[tokio::test]
    async fn test_watch_kv_changes_cancel() {
        use std::time::Duration;

        use futures::stream;

        // 构造一个永不结束的流
        let stream = stream::pending();

        let cache = Arc::new(Mutex::new(Ok(CacheData::default())));
        let mut watcher = EventWatcher::<TestConfig> {
            left: "".to_string(),
            right: "z".to_string(),
            source: TestSource::new(),
            data: cache.clone(),
            name: "test-watch-kv-changes-cancel".to_string(),
        };

        // cancel信号，50ms后触发
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let cancel = async {
            let _ = cancel_rx.await;
        };
        let handle = tokio::spawn(async move { watcher.watch_kv_changes(stream, cancel).await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = cancel_tx.send(());
        let res = handle.await.unwrap();
        assert!(res.is_ok());

        // 检查cache内容应为空
        let cache_data = cache.lock().await;
        let cache_data = cache_data.as_ref().unwrap();
        assert!(cache_data.data.is_empty());
        assert_eq!(cache_data.last_seq, 0);
    }

    #[tokio::test]
    async fn test_watch_kv_changes_error_event() {
        use futures::stream;

        use crate::errors::ConnectionClosed;
        use crate::event_stream::Change;
        use crate::event_stream::Event;

        // 构造流中间插入Err(ConnectionClosed)
        let events = vec![
            Ok(Event::Change(Change::new(
                "k1",
                None,
                Some(Val::new(1, "v1")),
            ))),
            Err(ConnectionClosed::new_str("test error")),
            Ok(Event::Change(Change::new(
                "k2",
                None,
                Some(Val::new(2, "v2")),
            ))),
        ];
        let stream = stream::iter(events);

        let cache = Arc::new(Mutex::new(Ok(CacheData::default())));
        let mut watcher = EventWatcher::<TestConfig> {
            left: "".to_string(),
            right: "z".to_string(),
            source: TestSource::new(),
            data: cache.clone(),
            name: "test-watch-kv-changes-error".to_string(),
        };

        let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let cancel = async {
            let _ = cancel_rx.await;
        };
        let res = watcher.watch_kv_changes(stream, cancel).await;
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(err.to_string().contains("test error"));

        // 检查cache内容只包含第一个事件
        let cache_data = cache.lock().await;
        let cache_data = cache_data.as_ref().unwrap();
        assert_eq!(cache_data.data.get("k1"), Some(&Val::new(1, "v1")));
        assert!(!cache_data.data.contains_key("k2"));
        assert_eq!(cache_data.last_seq, 1);
    }

    #[tokio::test]
    #[should_panic(expected = "expected only change events")]
    async fn test_watch_kv_changes_unexpected_event() {
        use futures::stream;

        use crate::event_stream::Change;
        use crate::event_stream::Event;

        // 构造流中插入非Change事件
        let events = vec![
            Ok(Event::Change(Change::new(
                "k1",
                None,
                Some(Val::new(1, "v1")),
            ))),
            Ok(Event::Initialization(Change::new(
                "k2",
                None,
                Some(Val::new(2, "v2")),
            ))),
        ];
        let stream = stream::iter(events);

        let cache = Arc::new(Mutex::new(Ok(CacheData::default())));
        let mut watcher = EventWatcher::<TestConfig> {
            left: "".to_string(),
            right: "z".to_string(),
            source: TestSource::new(),
            data: cache.clone(),
            name: "test-watch-kv-changes-unexpected".to_string(),
        };

        let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let cancel = async {
            let _ = cancel_rx.await;
        };
        let _ = watcher.watch_kv_changes(stream, cancel).await;

        // 检查panic前cache内容（只处理第一个Change）
        let cache_data = cache.lock().await;
        let cache_data = cache_data.as_ref().unwrap();
        assert_eq!(cache_data.data.get("k1"), Some(&Val::new(1, "v1")));
        assert!(!cache_data.data.contains_key("k2"));
        assert_eq!(cache_data.last_seq, 1);
    }

    #[tokio::test]
    async fn test_watch_kv_changes_multi_update_last_seq() {
        use futures::stream;

        use crate::event_stream::Change;
        use crate::event_stream::Event;

        // 多次变更同一key，last_seq只取最大值
        let events = vec![
            Ok(Event::Change(Change::new(
                "k1",
                None,
                Some(Val::new(1, "v1")),
            ))),
            Ok(Event::Change(Change::new(
                "k1",
                Some(Val::new(1, "v1")),
                Some(Val::new(5, "v1x")),
            ))),
            Ok(Event::Change(Change::new(
                "k1",
                Some(Val::new(5, "v1x")),
                Some(Val::new(3, "v1y")),
            ))),
        ];
        let stream = stream::iter(events);

        let cache = Arc::new(Mutex::new(Ok(CacheData::default())));
        let mut watcher = EventWatcher::<TestConfig> {
            left: "".to_string(),
            right: "z".to_string(),
            source: TestSource::new(),
            data: cache.clone(),
            name: "test-watch-kv-changes-multi-update".to_string(),
        };

        let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
        let cancel = async {
            let _ = cancel_rx.await;
        };
        let _ = watcher.watch_kv_changes(stream, cancel).await;

        let cache_data = cache.lock().await;
        let cache_data = cache_data.as_ref().unwrap();
        // last_seq应为最大值5，内容为最后一次变更
        assert_eq!(cache_data.last_seq, 5);
        assert_eq!(cache_data.data.get("k1"), Some(&Val::new(3, "v1y")));
    }
}

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
use std::future::Future;

use crate::errors::SubscribeError;
use crate::event_stream::EventStream;

pub trait TypeConfig
where
    Self: fmt::Debug,
    Self: Default,
    Self: Send + Sync + 'static,
{
    /// The type of values that are watched.
    type Value: fmt::Debug + Clone + Send + Sync + 'static;

    /// The client to create a watching stream that subscribe change event from a user defined data source.
    type Source: Source<Self::Value> + Send + Sync + 'static;

    /// Extract the seq number in the value.
    ///
    ///
    /// The Value type contains a seq number indicating the version of the value.
    fn value_seq(value: &Self::Value) -> u64;

    /// Spawn a future that will run in the background.
    ///
    /// `name` is used for debugging purposes, it can be any string that identifies the future.
    fn spawn<F>(future: F, name: impl ToString)
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static;
}

#[async_trait::async_trait]
pub trait Source<V> {
    /// Subscribe range `[left, right)` to the source and return a stream of events.
    ///
    /// `[left, right)` is left-closed and right-open key range that is interested in.
    async fn subscribe(&self, left: &str, right: &str) -> Result<EventStream<V>, SubscribeError>;
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_foo() {}
}

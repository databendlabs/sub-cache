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

use crate::errors::ConnectionClosed;
use crate::errors::Unsupported;

/// Errors that can occur when subscribing to a source of events.
#[derive(thiserror::Error, Debug)]
pub enum SubscribeError {
    #[error("Subscribe encounter Connection error: {0}")]
    Connection(#[from] ConnectionClosed),

    #[error("Subscribe is Unsupported: {0}")]
    Unsupported(#[from] Unsupported),
}

impl SubscribeError {
    pub fn context(self, context: impl fmt::Display) -> Self {
        match self {
            Self::Connection(e) => Self::Connection(e.context(context)),
            Self::Unsupported(e) => Self::Unsupported(e.context(context)),
        }
    }
}

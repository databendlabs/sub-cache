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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_functionality() {
        // Test context with Connection variant (also tests From trait)
        let conn_err = ConnectionClosed::new_str("timeout");
        let subscribe_err: SubscribeError = conn_err.into();
        let with_context = subscribe_err.context("during subscription");

        let error_string = with_context.to_string();
        assert!(error_string.contains("timeout"));
        assert!(error_string.contains("during subscription"));

        // Test context with Unsupported variant (also tests From trait)
        let unsupported_err = Unsupported::new("version mismatch");
        let subscribe_err: SubscribeError = unsupported_err.into();
        let with_context = subscribe_err.context("initializing client");

        let error_string = with_context.to_string();
        assert!(error_string.contains("version mismatch"));
        assert!(error_string.contains("initializing client"));
    }

    #[test]
    fn test_display_formatting() {
        // Test Connection variant display
        let conn_err = ConnectionClosed::new_str("connection lost");
        let subscribe_err = SubscribeError::Connection(conn_err);
        let display_string = subscribe_err.to_string();
        assert!(display_string.contains("Subscribe encounter Connection error"));
        assert!(display_string.contains("connection lost"));

        // Test Unsupported variant display
        let unsupported_err = Unsupported::new("not implemented");
        let subscribe_err = SubscribeError::Unsupported(unsupported_err);
        let display_string = subscribe_err.to_string();
        assert!(display_string.contains("Subscribe is Unsupported"));
        assert!(display_string.contains("not implemented"));
    }
}

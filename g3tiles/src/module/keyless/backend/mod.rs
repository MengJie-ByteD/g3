/*
 * Copyright 2024 ByteDance and/or its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use tokio::sync::oneshot;

use crate::module::keyless::{KeylessRequest, KeylessResponse};

mod stats;
pub(crate) use stats::{
    KeylessBackendStats, KeylessUpstreamDurationRecorder, KeylessUpstreamDurationStats,
};

mod pool;
pub(crate) use pool::{
    KeylessConnectionPool, KeylessConnectionPoolHandle, KeylessUpstreamConnect,
    KeylessUpstreamConnection,
};

mod multiplex;
pub(crate) use multiplex::MultiplexedUpstreamConnection;

pub(crate) struct KeylessForwardRequest {
    pub(crate) req: KeylessRequest,
    pub(crate) rsp_sender: oneshot::Sender<KeylessResponse>,
}

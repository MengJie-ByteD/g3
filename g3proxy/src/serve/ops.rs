/*
 * Copyright 2023 ByteDance and/or its affiliates.
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

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use log::{debug, warn};
use tokio::sync::Mutex;

use g3_types::metrics::MetricsName;
use g3_yaml::YamlDocPosition;

use crate::config::server::{AnyServerConfig, ServerConfigDiffAction};

use super::{registry, ArcServer};

use super::dummy_close::DummyCloseServer;
use super::intelli_proxy::IntelliProxy;
use super::native_tls_port::NativeTlsPort;
use super::plain_tcp_port::PlainTcpPort;
use super::plain_tls_port::PlainTlsPort;

use super::http_proxy::HttpProxyServer;
use super::http_rproxy::HttpRProxyServer;
use super::sni_proxy::SniProxyServer;
use super::socks_proxy::SocksProxyServer;
use super::tcp_stream::TcpStreamServer;
use super::tls_stream::TlsStreamServer;

static SERVER_OPS_LOCK: Mutex<()> = Mutex::const_new(());

pub fn spawn_offline_clean() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.tick().await;
        loop {
            registry::retain_offline();
            interval.tick().await;
        }
    });
}

pub async fn spawn_all() -> anyhow::Result<()> {
    let _guard = SERVER_OPS_LOCK.lock().await;

    let mut new_names = HashSet::<MetricsName>::new();

    let all_config = crate::config::server::get_all_sorted()?;
    for config in all_config {
        let name = config.name();
        new_names.insert(name.clone());
        match registry::get_config(name) {
            Some(old) => {
                debug!("reloading server {name}");
                reload_old_unlocked(old, config.as_ref().clone())?;
                debug!("server {name} reload OK");
            }
            None => {
                debug!("creating server {name}");
                spawn_new_unlocked(config.as_ref().clone())?;
                debug!("server {name} create OK");
            }
        }
    }

    for name in &registry::get_names() {
        if !new_names.contains(name) {
            debug!("deleting server {name}");
            registry::del(name);
            debug!("server {name} deleted");
        }
    }

    Ok(())
}

pub async fn stop_all() {
    let _guard = SERVER_OPS_LOCK.lock().await;

    registry::foreach_online(|_name, server| {
        server._abort_runtime();
        registry::add_offline(Arc::clone(server));
    });
}

pub(crate) fn get_server(name: &MetricsName) -> anyhow::Result<ArcServer> {
    match registry::get_server(name) {
        Some(server) => Ok(server),
        None => Err(anyhow!("no server named {name} found")),
    }
}

pub(crate) async fn reload(
    name: &MetricsName,
    position: Option<YamlDocPosition>,
) -> anyhow::Result<()> {
    let _guard = SERVER_OPS_LOCK.lock().await;

    let old_config = match registry::get_config(name) {
        Some(config) => config,
        None => return Err(anyhow!("no server with name {name} found")),
    };

    let position = match position {
        Some(position) => position,
        None => match old_config.position() {
            Some(position) => position,
            None => {
                return Err(anyhow!(
                    "no config position for server {name} found, reload is not supported"
                ));
            }
        },
    };

    let position2 = position.clone();
    let config =
        tokio::task::spawn_blocking(move || crate::config::server::load_at_position(&position2))
            .await
            .map_err(|e| anyhow!("unable to join conf load task: {e}"))?
            .context(format!("unload to load conf at position {position}"))?;
    if name != config.name() {
        return Err(anyhow!(
            "server at position {position} has name {}, while we expect {name}",
            config.name()
        ));
    }

    debug!("reloading server {name} from position {position}");
    reload_old_unlocked(old_config, config)?;
    debug!("server {name} reload OK");
    Ok(())
}

pub(crate) async fn update_dependency_to_escaper(escaper: &MetricsName, status: &str) {
    let _guard = SERVER_OPS_LOCK.lock().await;

    let mut names = Vec::<MetricsName>::new();

    registry::foreach_online(|name, server| {
        if server.escaper().eq(escaper) {
            names.push(name.clone());
        }
    });

    if names.is_empty() {
        return;
    }

    debug!("escaper {escaper} changed({status}), will reload server(s) {names:?}");
    for name in names.iter() {
        debug!("server {name}: will reload as it's using escaper {escaper}");
        if let Err(e) = registry::reload_only_escaper(name) {
            warn!("failed to reload server {name}: {e:?}");
        }
    }
}

pub(crate) async fn update_dependency_to_user_group(user_group: &MetricsName, status: &str) {
    let _guard = SERVER_OPS_LOCK.lock().await;

    let mut names = Vec::<MetricsName>::new();

    registry::foreach_online(|name, server| {
        if server.user_group().eq(user_group) {
            names.push(name.clone());
        }
    });

    if names.is_empty() {
        return;
    }

    debug!("user group {user_group} changed({status}), will reload server(s) {names:?}");
    for name in names.iter() {
        debug!("server {name}: will reload as it's using user group {user_group}");
        if let Err(e) = registry::reload_only_user_group(name) {
            warn!("failed to reload server {name}: {e:?}");
        }
    }
}

pub(crate) async fn update_dependency_to_auditor(auditor: &MetricsName, status: &str) {
    let _guard = SERVER_OPS_LOCK.lock().await;

    let mut names = Vec::<MetricsName>::new();

    registry::foreach_online(|name, server| {
        if server.auditor().eq(auditor) {
            names.push(name.clone());
        }
    });

    if names.is_empty() {
        return;
    }

    debug!("auditor {auditor} changed({status}), will reload server(s) {names:?}");
    for name in names.iter() {
        debug!("server {name}: will reload as it's using auditor {auditor}");
        if let Err(e) = registry::reload_only_auditor(name) {
            warn!("failed to reload server {name}: {e:?}");
        }
    }
}

fn reload_old_unlocked(old: AnyServerConfig, new: AnyServerConfig) -> anyhow::Result<()> {
    let name = old.name();
    match old.diff_action(&new) {
        ServerConfigDiffAction::NoAction => {
            debug!("server {name} reload: no action is needed");
            Ok(())
        }
        ServerConfigDiffAction::SpawnNew => {
            debug!("server {name} reload: will create a totally new one");
            spawn_new_unlocked(new)
        }
        ServerConfigDiffAction::ReloadOnlyConfig => {
            debug!("server {name} reload: will only reload config");
            registry::reload_only_config(name, new)
        }
        ServerConfigDiffAction::ReloadAndRespawn => {
            debug!("server {name} reload: will respawn with old stats");
            registry::reload_and_respawn(name, new)
        }
        ServerConfigDiffAction::UpdateInPlace(flags) => {
            debug!("server {name} reload: will update the existed in place");
            registry::update_config_in_place(name, flags, new)
        }
    }
}

// use async fn to allow tokio schedule
fn spawn_new_unlocked(config: AnyServerConfig) -> anyhow::Result<()> {
    let name = config.name().clone();
    let server = match config {
        AnyServerConfig::DummyClose(_) => DummyCloseServer::prepare_initial(config)?,
        AnyServerConfig::PlainTcpPort(_) => PlainTcpPort::prepare_initial(config)?,
        AnyServerConfig::PlainTlsPort(_) => PlainTlsPort::prepare_initial(config)?,
        AnyServerConfig::NativeTlsPort(_) => NativeTlsPort::prepare_initial(config)?,
        AnyServerConfig::IntelliProxy(_) => IntelliProxy::prepare_initial(config)?,
        AnyServerConfig::TcpStream(_) => TcpStreamServer::prepare_initial(config)?,
        AnyServerConfig::TlsStream(_) => TlsStreamServer::prepare_initial(config)?,
        AnyServerConfig::SniProxy(_) => SniProxyServer::prepare_initial(config)?,
        AnyServerConfig::SocksProxy(_) => SocksProxyServer::prepare_initial(config)?,
        AnyServerConfig::HttpProxy(_) => HttpProxyServer::prepare_initial(config)?,
        AnyServerConfig::HttpRProxy(_) => HttpRProxyServer::prepare_initial(config)?,
    };
    registry::add(name, server)?;
    Ok(())
}

pub(crate) async fn wait_all_tasks<F>(wait_timeout: Duration, quit_timeout: Duration, on_timeout: F)
where
    F: Fn(&MetricsName, i32),
{
    let loop_wait = async {
        loop {
            let mut has_pending = false;

            registry::foreach_offline(|server| {
                if server.alive_count() > 0 {
                    has_pending = true;
                }
            });

            if !has_pending {
                if let Some(stat_config) = g3_daemon::stat::config::get_global_stat_config() {
                    // sleep more time for flushing metrics
                    tokio::time::sleep(stat_config.emit_duration * 2).await;
                }
                break;
            }

            tokio::time::sleep(Duration::from_secs(4)).await;
        }
    };

    tokio::pin!(loop_wait);

    debug!("will wait {wait_timeout:?} for all tasks to be finished");
    if tokio::time::timeout(wait_timeout, &mut loop_wait)
        .await
        .is_ok()
    {
        return;
    }

    // enable force_quit and wait more time
    force_quit_offline_servers();

    debug!("will wait {quit_timeout:?} for all tasks to force quit");
    if tokio::time::timeout(quit_timeout, &mut loop_wait)
        .await
        .is_err()
    {
        registry::foreach_offline(|server| {
            on_timeout(server.name(), server.alive_count());
        });
    }
}

pub(crate) fn force_quit_offline_servers() {
    registry::foreach_offline(|server| {
        server.quit_policy().set_force_quit();
    });
}

pub(crate) fn force_quit_offline_server(name: &MetricsName) {
    registry::foreach_offline(|server| {
        if server.name() == name {
            server.quit_policy().set_force_quit();
        }
    });
}

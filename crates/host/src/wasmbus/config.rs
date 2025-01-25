//! Module with structs for use in managing and accessing config used by various wasmCloud entities
use std::{collections::HashMap, fmt::Debug, sync::Arc};

use anyhow::{bail, Context};
use async_nats::jetstream::kv::{Operation, Store};
use futures::{future::AbortHandle, stream::Abortable, TryStreamExt};
use tokio::sync::{
    watch::{self, Receiver, Sender},
    RwLock, RwLockReadGuard,
};
use tracing::{error, warn, Instrument};

type LockedConfig = Arc<RwLock<HashMap<String, String>>>;
/// A cache of named config mapped to an existing receiver
type WatchCache = Arc<RwLock<HashMap<String, Receiver<HashMap<String, String>>>>>;

/// A struct used for mapping a config name to a receiver for logging/tracing purposes
struct ConfigReceiver {
    pub name: String,
    pub receiver: Receiver<HashMap<String, String>>,
}

/// Helper struct that aborts on drop so we don't abort them when something is cloned in an arc. It
/// will only abort after the last arc has been dropped.
#[derive(Default)]
struct AbortHandles {
    handles: Vec<AbortHandle>,
}

impl Drop for AbortHandles {
    fn drop(&mut self) {
        for handle in &self.handles {
            handle.abort();
        }
    }
}

/// A merged bundle of configuration for use with components that watches for updates to all named
/// configs specified.
///
/// There are two main ways to get config from this struct:
///
/// 1. You can call [`get_config`](ConfigBundle::get_config) which will return a reference to the
///    merged config. This is mainly for use in components, which will fetch needed config on demand
/// 2. You can call [`changed`](ConfigBundle::changed) which will return a reference to the merged
///    config. This is for use in situations where you want to be notified when a config changes,
///    such as for a provider that needs to be notified when a config changes
pub struct ConfigBundle {
    /// A live view of the configuration that is being managed/updated by this bundle
    merged_config: LockedConfig,
    /// Names of named config that contribute to this bundle
    config_names: Vec<String>,
    /// A receiver that fires when changes are made to the bundle
    changed_receiver: Receiver<()>,
    /// Abort handles to the tasks that are watching for updates
    ///
    /// These are `drop()`ed when the bundle is dropped
    _handles: Arc<AbortHandles>,
    /// The sender that is used to notify the receiver that the config has changed, this
    /// must not be dropped until the receiver is dropped so we ensure it's kept alive
    _changed_notifier: Arc<Sender<()>>,
}

impl Clone for ConfigBundle {
    fn clone(&self) -> Self {
        // Cloning marks the value in the new receiver as seen, so we mark it as unseen, even if it
        // was already viewed before cloning. This ensures that the newly cloned bundle will return
        // the current config rather than needing to wait for a change.
        let mut changed_receiver = self.changed_receiver.clone();
        changed_receiver.mark_changed();
        ConfigBundle {
            merged_config: self.merged_config.clone(),
            config_names: self.config_names.clone(),
            changed_receiver,
            _changed_notifier: self._changed_notifier.clone(),
            _handles: self._handles.clone(),
        }
    }
}

impl Debug for ConfigBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigBundle")
            .field("merged_config", &self.merged_config)
            .finish()
    }
}

impl ConfigBundle {
    /// Create a new config bundle.
    ///
    /// It takes an ordered list of receivers that should match the
    /// order of config given by the user.
    ///
    /// This is only called internally.
    #[must_use]
    async fn new(receivers: Vec<ConfigReceiver>) -> Self {
        // Generate the initial abort handles so we can construct the bundle
        let (abort_handles, mut registrations): (Vec<_>, Vec<_>) =
            std::iter::repeat_with(AbortHandle::new_pair)
                .take(receivers.len())
                .unzip();
        // Now that we've set initial config, create the bundle and update the merged config with the latest values
        let (changed_notifier, changed_receiver) = watch::channel(());
        let changed_notifier = Arc::new(changed_notifier);
        let mut bundle = ConfigBundle {
            merged_config: Arc::default(),
            config_names: receivers.iter().map(|r| r.name.clone()).collect(),
            changed_receiver,
            _changed_notifier: changed_notifier.clone(),
            _handles: Arc::new(AbortHandles {
                handles: abort_handles,
            }),
        };
        let ordered_configs: Arc<Vec<Receiver<HashMap<String, String>>>> =
            Arc::new(receivers.iter().map(|r| r.receiver.clone()).collect());
        update_merge(&bundle.merged_config, &changed_notifier, &ordered_configs).await;
        // Move all the receivers into spawned tasks to update the config
        for ConfigReceiver { name, mut receiver } in receivers {
            // SAFETY: We know we have the right amount of registrations because we just created
            // them using the len above
            let reg = registrations
                .pop()
                .expect("missing registration, this is developer error");
            let cloned_name = name.clone();
            let ordered_receivers = ordered_configs.clone();
            let merged_config = bundle.merged_config.clone();
            let notifier = changed_notifier.clone();
            tokio::spawn(
                Abortable::new(
                    async move {
                        loop {
                            match receiver.changed().await {
                                Ok(()) => {
                                    update_merge(&merged_config, &notifier, &ordered_receivers)
                                        .await;
                                }
                                Err(e) => {
                                    warn!(error = %e, %name, "config sender dropped, updates will not be delivered");
                                    return;
                                }
                            }
                        }
                    },
                    reg,
                )
                .instrument(tracing::trace_span!("config_update", name = %cloned_name)),
            );
        }
        // More likely than not, there will be a new value in the watch channel because we always
        // read the latest value from the store before putting it here. But just in case, this
        // ensures that the newly create bundle will return the current config rather than needing
        // to wait for a change.
        bundle.changed_receiver.mark_changed();
        bundle
    }

    /// Returns a reference to the merged config behind a lock guard. This guard must be dropped
    /// when you are no longer consuming the config
    pub async fn get_config(&self) -> RwLockReadGuard<'_, HashMap<String, String>> {
        self.merged_config.read().await
    }

    /// Waits for the config to change and returns a reference to the merged config behind a lock
    /// guard. This guard must be dropped when you are no longer consuming the config.
    ///
    /// Please note that this requires a mutable borrow in order to manage underlying notification
    /// acknowledgement.
    pub async fn changed(
        &mut self,
    ) -> anyhow::Result<RwLockReadGuard<'_, HashMap<String, String>>> {
        // NOTE(thomastaylor312): We use a watch channel here because we want everything to get
        // notified individually (including clones) if config changes. Notify doesn't quite work
        // because we have to have a permit existing when we create otherwise the notify_watchers
        // won't actually get picked up (that only happens with notify_one).
        if let Err(e) = self.changed_receiver.changed().await {
            // If we get here, it likely means that a whole bunch of stuff has failed above it.
            // Might be worth changing this to a panic
            error!(error = %e, "Config changed receiver errored, this means that the config sender has dropped and the whole bundle has failed");
            bail!("failed to read receiver: {e}");
        }
        Ok(self.merged_config.read().await)
    }

    /// Returns a reference to the ordered list of config names handled by this bundle
    #[must_use]
    pub fn config_names(&self) -> &Vec<String> {
        &self.config_names
    }
}

/// A struct used for generating a config bundle given a list of named configs
#[derive(Clone)]
pub struct BundleGenerator {
    store: Store,
    watch_cache: WatchCache,
    watch_handles: Arc<RwLock<AbortHandles>>,
}

impl BundleGenerator {
    /// Create a new bundle generator
    #[must_use]
    pub fn new(store: Store) -> Self {
        Self {
            store,
            watch_cache: Arc::default(),
            watch_handles: Arc::default(),
        }
    }

    /// Generate a new config bundle. Will return an error if any of the configs do not exist or if
    /// there was an error fetching the initial config
    pub async fn generate(&self, config_names: Vec<String>) -> anyhow::Result<ConfigBundle> {
        let receivers: Vec<ConfigReceiver> =
            futures::future::join_all(config_names.into_iter().map(|name| self.get_receiver(name)))
                .await
                .into_iter()
                .collect::<anyhow::Result<_>>()?;
        Ok(ConfigBundle::new(receivers).await)
    }

    async fn get_receiver(&self, name: String) -> anyhow::Result<ConfigReceiver> {
        // First check the cache to see if we already have a receiver for this config
        if let Some(receiver) = self.watch_cache.read().await.get(&name) {
            return Ok(ConfigReceiver {
                name,
                receiver: receiver.clone(),
            });
        }

        // We need to actually try and fetch the config here. If we don't do this, then a watch will
        // just blindly watch even if the key doesn't exist. We should return an error if the config
        // doesn't exist or has data issues. It also allows us to set the initial value
        let config: HashMap<String, String> = match self.store.get(&name).await {
            Ok(Some(data)) => serde_json::from_slice(&data)
                .context("Data corruption error, unable to decode data from store")?,
            Ok(None) => return Err(anyhow::anyhow!("Config {} does not exist", name)),
            Err(e) => return Err(anyhow::anyhow!("Error fetching config {}: {}", name, e)),
        };

        // Otherwise we need to setup the watcher. We start by setting up the watch so we don't miss
        // any events after we query the initial config
        let (tx, rx) = watch::channel(config);
        let (done, wait) = tokio::sync::oneshot::channel();
        let (handle, reg) = AbortHandle::new_pair();
        tokio::task::spawn(Abortable::new(
            watcher_loop(self.store.clone(), name.clone(), tx, done),
            reg,
        ));

        wait.await
            .context("Error waiting for watcher to start")?
            .context("Error waiting for watcher to start")?;

        // NOTE(thomastaylor312): We should probably find a way to clear out this cache. The Sender
        // part of the channel can tell you how many receivers it has, but we pass that along to the
        // watcher, so there would need to be more work to expose that, probably via a struct. We
        // could also do something with a resource counter and track that way with a cleanup task.
        // But for now going the easy route as we already cache everything anyway
        self.watch_handles.write().await.handles.push(handle);
        self.watch_cache
            .write()
            .await
            .insert(name.clone(), rx.clone());

        Ok(ConfigReceiver { name, receiver: rx })
    }
}

async fn watcher_loop(
    store: Store,
    name: String,
    tx: watch::Sender<HashMap<String, String>>,
    done: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
) {
    // We need to watch with history so we can get the initial config.
    let mut watcher = match store.watch(&name).await {
        Ok(watcher) => {
            done.send(Ok(())).expect(
                "Receiver for watcher setup should not have been dropped. This is programmer error",
            );
            watcher
        }
        Err(e) => {
            done.send(Err(anyhow::anyhow!(
                "Error setting up watcher for {}: {}",
                name,
                e
            )))
            .expect(
                "Receiver for watcher setup should not have been dropped. This is programmer error",
            );
            return;
        }
    };
    loop {
        match watcher.try_next().await {
            Ok(Some(entry)) if matches!(entry.operation, Operation::Delete | Operation::Purge) => {
                // NOTE(thomastaylor312): We should probably do something and notify something up
                // the chain if we get a delete or purge event of a config that is still being used.
                // For now we just zero it out
                tx.send_replace(HashMap::new());
            }
            Ok(Some(entry)) => {
                let config: HashMap<String, String> = match serde_json::from_slice(&entry.value) {
                    Ok(config) => config,
                    Err(e) => {
                        error!(%name, error = %e, "Error decoding config from store during watch");
                        continue;
                    }
                };
                tx.send_if_modified(|current| {
                    if current == &config {
                        false
                    } else {
                        *current = config;
                        true
                    }
                });
            }
            Ok(None) => {
                error!(%name, "Watcher for config has closed");
                return;
            }
            Err(e) => {
                error!(%name, error = %e, "Error reading from watcher for config. Will wait for next entry");
                continue;
            }
        }
    }
}

async fn update_merge(
    merged_config: &RwLock<HashMap<String, String>>,
    changed_notifier: &Sender<()>,
    ordered_receivers: &[Receiver<HashMap<String, String>>],
) {
    // We get a write lock to start so nothing else can update the merged config while we merge
    // in the other configs (e.g. when one of the ordered configs is write locked)
    let mut hashmap = merged_config.write().await;
    hashmap.clear();

    // NOTE(thomastaylor312): There is a possible optimization here where we could just create a
    // temporary hashmap of borrowed strings and then after extending everything we could
    // into_iter it and clone it into the final hashmap. This would avoid extra allocations at
    // the cost of a few more iterations
    for recv in ordered_receivers {
        hashmap.extend(recv.borrow().clone());
    }
    // Send a notification that the config has changed
    changed_notifier.send_replace(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use tokio::sync::watch;

    #[tokio::test]
    async fn test_config_bundle() {
        let (foo_tx, foo_rx) =
            watch::channel(HashMap::from([("foo".to_string(), "bar".to_string())]));
        let (bar_tx, bar_rx) = watch::channel(HashMap::new());
        let (baz_tx, baz_rx) = watch::channel(HashMap::new());

        let mut bundle = ConfigBundle::new(vec![
            ConfigReceiver {
                name: "foo".to_string(),
                receiver: foo_rx,
            },
            ConfigReceiver {
                name: "bar".to_string(),
                receiver: bar_rx,
            },
            ConfigReceiver {
                name: "baz".to_string(),
                receiver: baz_rx,
            },
        ])
        .await;

        // We should be able to get the initial config before sending anything
        assert_eq!(
            *bundle.get_config().await,
            HashMap::from([("foo".to_string(), "bar".to_string())])
        );

        // Should also be able to get a value from the changed method immediately
        let _ = tokio::time::timeout(Duration::from_millis(50), bundle.changed())
            .await
            .expect("Should have received a config");

        // Update the bar config to overwrite the foo config
        bar_tx.send_replace(HashMap::from([("foo".to_string(), "baz".to_string())]));
        // Wait for the new config to come. This calls the same underlying method as get_config
        let conf = tokio::time::timeout(Duration::from_millis(50), bundle.changed())
            .await
            .expect("conf should have been present")
            .expect("Should have received a config");
        assert_eq!(
            *conf,
            HashMap::from([("foo".to_string(), "baz".to_string())])
        );
        drop(conf);

        // Update the baz config with additional data
        baz_tx.send_replace(HashMap::from([("star".to_string(), "wars".to_string())]));
        let conf = tokio::time::timeout(Duration::from_millis(50), bundle.changed())
            .await
            .expect("conf should have been present")
            .expect("Should have received a config");
        assert_eq!(
            *conf,
            HashMap::from([
                ("foo".to_string(), "baz".to_string()),
                ("star".to_string(), "wars".to_string())
            ])
        );
        drop(conf);

        // Update foo config with additional data
        foo_tx.send_replace(HashMap::from([
            ("starship".to_string(), "troopers".to_string()),
            ("foo".to_string(), "bar".to_string()),
        ]));
        let conf = tokio::time::timeout(Duration::from_millis(50), bundle.changed())
            .await
            .expect("conf should have been present")
            .expect("Should have received a config");
        // Check that the config merged correctly
        assert_eq!(
            *conf,
            HashMap::from([
                ("foo".to_string(), "baz".to_string()),
                ("star".to_string(), "wars".to_string()),
                ("starship".to_string(), "troopers".to_string())
            ]),
        );
    }
}

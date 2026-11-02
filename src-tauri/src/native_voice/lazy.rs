//! The real engine, made on first use.
//!
//! The plugin's default engine starts at app launch and lists the audio devices
//! every 2 s from then on. Someone who never joins a call shouldn't pay for that
//! or have K2 touching their audio stack, so K2 hands the plugin this wrapper:
//! the real engine exists from the first join or device list.

use std::sync::{Arc, OnceLock};

use tauri_plugin_sew_voice::wire::DeviceList;
use tauri_plugin_sew_voice::{BoxFuture, CallHandle, EngineError, JoinProvider, JoinRequest, VoiceEngine};
use tokio::sync::broadcast;

type Make = Box<dyn Fn() -> Arc<dyn VoiceEngine> + Send + Sync>;

pub struct LazyEngine {
    make: Make,
    real: OnceLock<Arc<dyn VoiceEngine>>,
    devices_tx: broadcast::Sender<DeviceList>,
}

impl LazyEngine {
    pub fn new(make: impl Fn() -> Arc<dyn VoiceEngine> + Send + Sync + 'static) -> LazyEngine {
        let (devices_tx, _) = broadcast::channel(16);
        LazyEngine { make: Box::new(make), real: OnceLock::new(), devices_tx }
    }

    /// True once something has needed the engine.
    pub fn started(&self) -> bool {
        self.real.get().is_some()
    }

    fn real(&self) -> &Arc<dyn VoiceEngine> {
        self.real.get_or_init(|| {
            let engine = (self.make)();
            let mut rx = engine.device_events();
            let tx = self.devices_tx.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(list) => {
                            // No receivers right now is fine: the plugin subscribes once at start.
                            let _ = tx.send(list);
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => return,
                    }
                }
            });
            engine
        })
    }
}

impl VoiceEngine for LazyEngine {
    fn join(&self, provider: Arc<dyn JoinProvider>, req: JoinRequest) -> BoxFuture<'_, Result<Arc<dyn CallHandle>, EngineError>> {
        self.real().join(provider, req)
    }

    fn devices(&self) -> Result<DeviceList, EngineError> {
        self.real().devices()
    }

    fn set_device(&self, input: Option<String>, output: Option<String>) -> Result<(), EngineError> {
        self.real().set_device(input, output)
    }

    fn device_events(&self) -> broadcast::Receiver<DeviceList> {
        self.devices_tx.subscribe()
    }
}

use crate::ids::{SceneId, SourceId};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

pub use crate::stats::{RecordState, ReplayBufferState, StreamState};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EngineEvent {
    SceneSwitched {
        scene_id: SceneId,
        scene_name: String,
    },
    PreviewSceneChanged {
        scene_id: SceneId,
        scene_name: String,
    },
    SceneListChanged,
    SceneItemsChanged {
        scene_id: SceneId,
    },
    SourceCreated {
        source_id: SourceId,
        name: String,
        kind: String,
    },
    SourceRemoved {
        source_id: SourceId,
        name: String,
    },
    SourceUpdated {
        source_id: SourceId,
        name: String,
    },
    SourceVolumeChanged {
        source_id: SourceId,
        name: String,
        volume_db: f32,
        volume_mul: f32,
    },
    SourceMuteChanged {
        source_id: SourceId,
        name: String,
        muted: bool,
    },
    StudioModeChanged {
        enabled: bool,
    },
    TransitionStarted {
        name: String,
        duration_ms: u32,
    },
    TransitionEnded {
        name: String,
    },
    RecordStateChanged {
        state: RecordState,
        output_path: Option<String>,
    },
    StreamStateChanged {
        state: StreamState,
        error: Option<String>,
    },
    ReplayBufferStateChanged {
        state: ReplayBufferState,
    },
    ReplaySaved {
        saved_path: String,
    },
    ProfileChanged {
        profile: String,
    },
    SceneCollectionChanged {
        collection: String,
    },
    PluginLoaded {
        id: String,
        name: String,
    },
    Warning {
        message: String,
    },
    Error {
        message: String,
    },
}

/// Thread-safe event broadcast hub consumed by the UI, OBS WebSocket 5.x server, and plugins.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<EngineEvent>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(64));
        Self { tx }
    }

    pub fn publish(&self, event: EngineEvent) {
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(512)
    }
}

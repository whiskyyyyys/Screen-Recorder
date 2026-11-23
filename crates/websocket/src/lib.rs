use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use futures_util::{SinkExt, StreamExt};
use obs_core::{EngineEvent, EngineStats, EventBus, RecordState, ReplayBufferState, StreamState};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::Message;

pub const OBS_WEBSOCKET_VERSION: &str = "5.5.4";
pub const RPC_VERSION: u32 = 1;

pub mod close_codes {
    pub const UNSUPPORTED_RPC_VERSION: u16 = 4006;
    pub const NOT_IDENTIFIED: u16 = 4007;
    pub const ALREADY_IDENTIFIED: u16 = 4008;
    pub const AUTHENTICATION_FAILED: u16 = 4009;
}

pub mod request_status {
    pub const SUCCESS: u32 = 100;
    pub const UNKNOWN_REQUEST_TYPE: u32 = 204;
    pub const MISSING_REQUEST_FIELD: u32 = 300;
    pub const RESOURCE_NOT_FOUND: u32 = 600;
    pub const RESOURCE_ALREADY_EXISTS: u32 = 601;
    pub const REQUEST_PROCESSING_FAILED: u32 = 700;
}

/// Backend command interface implemented by `ObsEngine` for handling OBS WebSocket 5.x RPC requests.
pub trait WebSocketBackend: Send + Sync + 'static {
    fn stats(&self) -> EngineStats;
    fn list_scenes(&self) -> (String, Option<String>, Vec<String>);
    fn set_current_program_scene(&self, scene_name: &str) -> Result<(), String>;
    fn set_current_preview_scene(&self, scene_name: &str) -> Result<(), String>;
    fn create_scene(&self, scene_name: &str) -> Result<(), String>;
    fn remove_scene(&self, scene_name: &str) -> Result<(), String>;
    fn set_scene_name(&self, scene_name: &str, new_name: &str) -> Result<(), String>;

    fn get_studio_mode(&self) -> bool;
    fn set_studio_mode(&self, enabled: bool);
    fn trigger_transition(&self);

    fn get_transition_info(&self) -> (String, u32, Vec<String>);
    fn set_current_transition(&self, name: &str) -> Result<(), String>;
    fn set_transition_duration(&self, duration_ms: u32);
    fn set_tbar_position(&self, pos: f32);

    fn list_scene_items(&self, scene_name: &str) -> Result<Vec<Value>, String>;
    fn set_scene_item_enabled(
        &self,
        scene_name: &str,
        item_id: i64,
        enabled: bool,
    ) -> Result<(), String>;
    fn set_scene_item_locked(
        &self,
        scene_name: &str,
        item_id: i64,
        locked: bool,
    ) -> Result<(), String>;
    fn set_scene_item_transform(
        &self,
        scene_name: &str,
        item_id: i64,
        transform: &Value,
    ) -> Result<(), String>;
    fn remove_scene_item(&self, scene_name: &str, item_id: i64) -> Result<(), String>;

    fn list_inputs(&self) -> Vec<Value>;
    fn get_input_volume(&self, input_name: &str) -> Result<(f32, f32), String>;
    fn set_input_volume(
        &self,
        input_name: &str,
        volume_mul: Option<f32>,
        volume_db: Option<f32>,
    ) -> Result<(), String>;
    fn get_input_mute(&self, input_name: &str) -> Result<bool, String>;
    fn set_input_mute(&self, input_name: &str, muted: bool) -> Result<(), String>;
    fn toggle_input_mute(&self, input_name: &str) -> Result<bool, String>;

    fn start_record(&self) -> Result<(), String>;
    fn stop_record(&self) -> Result<String, String>;
    fn pause_record(&self) -> Result<(), String>;
    fn resume_record(&self) -> Result<(), String>;
    fn toggle_record_pause(&self) -> Result<bool, String>;

    fn start_stream(&self) -> Result<(), String>;
    fn stop_stream(&self) -> Result<(), String>;

    fn start_replay_buffer(&self) -> Result<(), String>;
    fn stop_replay_buffer(&self) -> Result<(), String>;
    fn save_replay_buffer(&self) -> Result<String, String>;

    fn list_profiles(&self) -> (String, Vec<String>);
    fn set_current_profile(&self, name: &str) -> Result<(), String>;
    fn list_scene_collections(&self) -> (String, Vec<String>);
    fn set_current_scene_collection(&self, name: &str) -> Result<(), String>;

    fn list_hotkey_names(&self) -> Vec<String>;
    fn trigger_hotkey(&self, action: &str) -> Result<(), String>;
}

#[derive(Debug, Clone)]
pub struct WebSocketConfig {
    pub host: String,
    pub port: u16,
    pub auth_enabled: bool,
    pub password: String,
}

#[derive(Debug, Deserialize)]
struct IncomingMessage {
    op: u32,
    d: Value,
}

#[derive(Debug, Serialize)]
struct OutgoingMessage {
    op: u32,
    d: Value,
}

/// Computes the OBS WebSocket 5.x double SHA-256 + Base64 authentication challenge response.
pub fn compute_auth_string(password: &str, salt: &str, challenge: &str) -> String {
    let mut h1 = Sha256::new();
    h1.update(password.as_bytes());
    h1.update(salt.as_bytes());
    let secret = B64.encode(h1.finalize());

    let mut h2 = Sha256::new();
    h2.update(secret.as_bytes());
    h2.update(challenge.as_bytes());
    B64.encode(h2.finalize())
}

fn random_b64_token(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    rand::thread_rng().fill_bytes(&mut bytes);
    B64.encode(bytes)
}

pub struct WebSocketServerHandle {
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    pub bound_addr: SocketAddr,
    pub client_count: Arc<AtomicUsize>,
}

impl WebSocketServerHandle {
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

impl Drop for WebSocketServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub async fn start_websocket_server(
    cfg: WebSocketConfig,
    backend: Arc<dyn WebSocketBackend>,
    events: EventBus,
) -> Result<WebSocketServerHandle, std::io::Error> {
    let addr: SocketAddr = format!("{}:{}", cfg.host, cfg.port)
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], cfg.port)));
    let listener = TcpListener::bind(addr).await?;
    let bound_addr = listener.local_addr()?;
    let client_count = Arc::new(AtomicUsize::new(0));
    let cc_clone = Arc::clone(&client_count);
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accepted = listener.accept() => {
                    let Ok((stream, _peer)) = accepted else { break; };
                    let b = Arc::clone(&backend);
                    let eb = events.clone();
                    let cfg_c = cfg.clone();
                    let cc = Arc::clone(&cc_clone);
                    tokio::spawn(async move {
                        cc.fetch_add(1, Ordering::Relaxed);
                        let _ = handle_client(stream, cfg_c, b, eb).await;
                        cc.fetch_sub(1, Ordering::Relaxed);
                    });
                }
            }
        }
    });

    Ok(WebSocketServerHandle {
        shutdown_tx: Some(shutdown_tx),
        bound_addr,
        client_count,
    })
}

async fn handle_client(
    stream: tokio::net::TcpStream,
    cfg: WebSocketConfig,
    backend: Arc<dyn WebSocketBackend>,
    events: EventBus,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[allow(clippy::result_large_err)]
    let ws = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, mut resp: Response| {
        if let Some(proto) = req.headers().get("Sec-WebSocket-Protocol") {
            if let Ok(s) = proto.to_str() {
                if s.contains("obswebsocket.json") {
                    resp.headers_mut().insert(
                        "Sec-WebSocket-Protocol",
                        "obswebsocket.json".parse().unwrap(),
                    );
                }
            }
        }
        Ok(resp)
    })
    .await?;

    let (mut sink, mut source) = ws.split();

    let salt = random_b64_token(24);
    let challenge = random_b64_token(24);
    let mut hello_d = json!({
        "obsStudioVersion": "31.0.0-oxidestudio",
        "obsWebSocketVersion": OBS_WEBSOCKET_VERSION,
        "rpcVersion": RPC_VERSION,
    });
    if cfg.auth_enabled && !cfg.password.is_empty() {
        hello_d["authentication"] = json!({
            "challenge": challenge,
            "salt": salt,
        });
    }
    send_json(&mut sink, 0, hello_d).await?;

    // Wait for Identify (OpCode 1)
    let mut event_subs: u64 = 0xFFFF_FFFF;
    let mut identified = false;
    while let Some(msg_res) = source.next().await {
        let msg = msg_res?;
        if let Message::Text(txt) = msg {
            let parsed: IncomingMessage = serde_json::from_str(&txt)?;
            if parsed.op != 1 {
                close_with(&mut sink, close_codes::NOT_IDENTIFIED, "Expected Identify").await;
                return Ok(());
            }
            let req_rpc = parsed
                .d
                .get("rpcVersion")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32;
            if req_rpc != RPC_VERSION {
                close_with(
                    &mut sink,
                    close_codes::UNSUPPORTED_RPC_VERSION,
                    "Unsupported RPC version",
                )
                .await;
                return Ok(());
            }
            if cfg.auth_enabled && !cfg.password.is_empty() {
                let expected = compute_auth_string(&cfg.password, &salt, &challenge);
                let provided = parsed
                    .d
                    .get("authentication")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if provided != expected {
                    close_with(
                        &mut sink,
                        close_codes::AUTHENTICATION_FAILED,
                        "Authentication failed",
                    )
                    .await;
                    return Ok(());
                }
            }
            if let Some(subs) = parsed.d.get("eventSubscriptions").and_then(|v| v.as_u64()) {
                event_subs = subs;
            }
            send_json(&mut sink, 2, json!({ "negotiatedRpcVersion": RPC_VERSION })).await?;
            identified = true;
            break;
        }
    }
    if !identified {
        return Ok(());
    }

    let mut evt_rx = events.subscribe();

    loop {
        tokio::select! {
            ws_msg = source.next() => {
                let Some(Ok(msg)) = ws_msg else { break; };
                match msg {
                    Message::Text(txt) => {
                        let Ok(parsed) = serde_json::from_str::<IncomingMessage>(&txt) else {
                            continue;
                        };
                        match parsed.op {
                            1 => {
                                close_with(&mut sink, close_codes::ALREADY_IDENTIFIED, "Already identified").await;
                                break;
                            }
                            3 => {
                                if let Some(subs) = parsed.d.get("eventSubscriptions").and_then(|v| v.as_u64()) {
                                    event_subs = subs;
                                }
                                send_json(&mut sink, 2, json!({ "negotiatedRpcVersion": RPC_VERSION })).await?;
                            }
                            6 => {
                                let resp = process_request(&*backend, &parsed.d);
                                send_json(&mut sink, 7, resp).await?;
                            }
                            8 => {
                                let req_id = parsed.d.get("requestId").cloned().unwrap_or(json!(""));
                                let halt = parsed.d.get("haltOnFailure").and_then(|v| v.as_bool()).unwrap_or(false);
                                let reqs = parsed.d.get("requests").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                                let mut results = Vec::with_capacity(reqs.len());
                                for r in reqs {
                                    let resp = process_request(&*backend, &r);
                                    let ok = resp["requestStatus"]["result"].as_bool().unwrap_or(false);
                                    results.push(resp);
                                    if halt && !ok {
                                        break;
                                    }
                                }
                                send_json(&mut sink, 9, json!({
                                    "requestId": req_id,
                                    "results": results,
                                })).await?;
                            }
                            _ => {}
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(p) => {
                        let _ = sink.send(Message::Pong(p)).await;
                    }
                    _ => {}
                }
            }
            evt_res = evt_rx.recv() => {
                match evt_res {
                    Ok(evt) => {
                        if let Some((intent_bit, ev_json)) = map_engine_event(&evt) {
                            if (event_subs & intent_bit) != 0 {
                                send_json(&mut sink, 5, ev_json).await?;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
    Ok(())
}

async fn send_json<S>(
    sink: &mut S,
    op: u32,
    d: Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    S: SinkExt<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    let txt = serde_json::to_string(&OutgoingMessage { op, d })?;
    sink.send(Message::Text(txt.into())).await?;
    Ok(())
}

async fn close_with<S>(sink: &mut S, code: u16, reason: &str)
where
    S: SinkExt<Message> + Unpin,
{
    let _ = sink
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(code),
            reason: reason.to_string().into(),
        })))
        .await;
}

fn ok_resp(req_type: &str, req_id: &Value, data: Option<Value>) -> Value {
    let mut obj = json!({
        "requestType": req_type,
        "requestId": req_id,
        "requestStatus": {
            "result": true,
            "code": request_status::SUCCESS,
        }
    });
    if let Some(d) = data {
        obj["responseData"] = d;
    }
    obj
}

fn err_resp(req_type: &str, req_id: &Value, code: u32, comment: &str) -> Value {
    json!({
        "requestType": req_type,
        "requestId": req_id,
        "requestStatus": {
            "result": false,
            "code": code,
            "comment": comment,
        }
    })
}

fn process_request(backend: &dyn WebSocketBackend, d: &Value) -> Value {
    let req_type = d.get("requestType").and_then(|v| v.as_str()).unwrap_or("");
    let req_id = d.get("requestId").cloned().unwrap_or(json!(""));
    let data = d.get("requestData").cloned().unwrap_or_else(|| json!({}));

    match req_type {
        "GetVersion" => ok_resp(
            req_type,
            &req_id,
            Some(json!({
                "obsVersion": "31.0.0-oxidestudio",
                "obsWebSocketVersion": OBS_WEBSOCKET_VERSION,
                "rpcVersion": RPC_VERSION,
                "availableRequests": [
                    "GetVersion", "GetStats", "GetSceneList", "GetCurrentProgramScene",
                    "SetCurrentProgramScene", "GetCurrentPreviewScene", "SetCurrentPreviewScene",
                    "CreateScene", "RemoveScene", "SetSceneName", "GetStudioModeEnabled",
                    "SetStudioModeEnabled", "TriggerStudioModeTransition", "GetSceneItemList",
                    "SetSceneItemEnabled", "SetSceneItemLocked", "SetSceneItemTransform",
                    "RemoveSceneItem", "GetInputList", "GetInputVolume", "SetInputVolume",
                    "GetInputMute", "SetInputMute", "ToggleInputMute", "GetRecordStatus",
                    "StartRecord", "StopRecord", "ToggleRecord", "PauseRecord", "ResumeRecord",
                    "ToggleRecordPause", "GetStreamStatus", "StartStream", "StopStream",
                    "ToggleStream", "GetReplayBufferStatus", "StartReplayBuffer",
                    "StopReplayBuffer", "ToggleReplayBuffer", "SaveReplayBuffer",
                    "GetProfileList", "SetCurrentProfile", "GetSceneCollectionList",
                    "SetCurrentSceneCollection", "GetHotkeyList", "TriggerHotkeyByName"
                ],
                "supportedImageFormats": ["png", "jpg", "bmp"],
                "platform": "windows",
                "platformDescription": "Windows x64 (OxideStudio Rust)"
            })),
        ),
        "GetStats" => {
            let s = backend.stats();
            ok_resp(
                req_type,
                &req_id,
                Some(json!({
                    "cpuUsage": s.cpu_usage_percent,
                    "memoryUsage": s.memory_usage_mb,
                    "activeFps": s.active_fps,
                    "averageFrameRenderTime": s.render_time_ms,
                    "renderSkippedFrames": s.lagged_frames,
                    "renderTotalFrames": s.total_rendered_frames,
                    "outputSkippedFrames": s.skipped_frames,
                    "outputTotalFrames": s.recording.encoded_frames + s.streaming.total_frames,
                    "webSocketSessionIncomingMessages": 0,
                    "webSocketSessionOutgoingMessages": 0,
                })),
            )
        }
        "GetSceneList" => {
            let (cur_prog, cur_prev, names) = backend.list_scenes();
            let scenes: Vec<Value> = names
                .iter()
                .enumerate()
                .map(|(i, n)| json!({ "sceneName": n, "sceneIndex": i }))
                .collect();
            ok_resp(
                req_type,
                &req_id,
                Some(json!({
                    "currentProgramSceneName": cur_prog,
                    "currentPreviewSceneName": cur_prev,
                    "scenes": scenes,
                })),
            )
        }
        "GetCurrentProgramScene" => {
            let (cur, _, _) = backend.list_scenes();
            ok_resp(
                req_type,
                &req_id,
                Some(json!({ "currentProgramSceneName": cur })),
            )
        }
        "SetCurrentProgramScene" => {
            let Some(name) = data.get("sceneName").and_then(|v| v.as_str()) else {
                return err_resp(
                    req_type,
                    &req_id,
                    request_status::MISSING_REQUEST_FIELD,
                    "Missing sceneName",
                );
            };
            match backend.set_current_program_scene(name) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "GetCurrentPreviewScene" => {
            let (prog, prev, _) = backend.list_scenes();
            ok_resp(
                req_type,
                &req_id,
                Some(json!({ "currentPreviewSceneName": prev.unwrap_or(prog) })),
            )
        }
        "SetCurrentPreviewScene" => {
            let Some(name) = data.get("sceneName").and_then(|v| v.as_str()) else {
                return err_resp(
                    req_type,
                    &req_id,
                    request_status::MISSING_REQUEST_FIELD,
                    "Missing sceneName",
                );
            };
            match backend.set_current_preview_scene(name) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "CreateScene" => {
            let Some(name) = data.get("sceneName").and_then(|v| v.as_str()) else {
                return err_resp(
                    req_type,
                    &req_id,
                    request_status::MISSING_REQUEST_FIELD,
                    "Missing sceneName",
                );
            };
            match backend.create_scene(name) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(
                    req_type,
                    &req_id,
                    request_status::RESOURCE_ALREADY_EXISTS,
                    &e,
                ),
            }
        }
        "RemoveScene" => {
            let Some(name) = data.get("sceneName").and_then(|v| v.as_str()) else {
                return err_resp(
                    req_type,
                    &req_id,
                    request_status::MISSING_REQUEST_FIELD,
                    "Missing sceneName",
                );
            };
            match backend.remove_scene(name) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "SetSceneName" => {
            let old = data.get("sceneName").and_then(|v| v.as_str()).unwrap_or("");
            let new_n = data
                .get("newSceneName")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match backend.set_scene_name(old, new_n) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(
                    req_type,
                    &req_id,
                    request_status::REQUEST_PROCESSING_FAILED,
                    &e,
                ),
            }
        }
        "GetStudioModeEnabled" => ok_resp(
            req_type,
            &req_id,
            Some(json!({ "studioModeEnabled": backend.get_studio_mode() })),
        ),
        "SetStudioModeEnabled" => {
            let enabled = data
                .get("studioModeEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            backend.set_studio_mode(enabled);
            ok_resp(req_type, &req_id, None)
        }
        "TriggerStudioModeTransition" => {
            backend.trigger_transition();
            ok_resp(req_type, &req_id, None)
        }
        "GetCurrentSceneTransition" => {
            let (name, dur, _) = backend.get_transition_info();
            ok_resp(
                req_type,
                &req_id,
                Some(json!({ "transitionName": name, "transitionDuration": dur })),
            )
        }
        "GetSceneTransitionList" => {
            let (name, _, list) = backend.get_transition_info();
            let trs: Vec<Value> = list
                .iter()
                .map(|t| json!({ "transitionName": t }))
                .collect();
            ok_resp(
                req_type,
                &req_id,
                Some(json!({ "currentSceneTransitionName": name, "transitions": trs })),
            )
        }
        "SetCurrentSceneTransition" => {
            let name = data
                .get("transitionName")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match backend.set_current_transition(name) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "SetCurrentSceneTransitionDuration" => {
            let dur = data
                .get("transitionDuration")
                .and_then(|v| v.as_u64())
                .unwrap_or(300) as u32;
            backend.set_transition_duration(dur);
            ok_resp(req_type, &req_id, None)
        }
        "SetTBarPosition" => {
            let pos = data.get("position").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            backend.set_tbar_position(pos);
            ok_resp(req_type, &req_id, None)
        }
        "GetSceneItemList" => {
            let scene_name = data.get("sceneName").and_then(|v| v.as_str()).unwrap_or("");
            match backend.list_scene_items(scene_name) {
                Ok(items) => ok_resp(req_type, &req_id, Some(json!({ "sceneItems": items }))),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "SetSceneItemEnabled" => {
            let scene_name = data.get("sceneName").and_then(|v| v.as_str()).unwrap_or("");
            let id = data
                .get("sceneItemId")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            let en = data
                .get("sceneItemEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            match backend.set_scene_item_enabled(scene_name, id, en) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "SetSceneItemLocked" => {
            let scene_name = data.get("sceneName").and_then(|v| v.as_str()).unwrap_or("");
            let id = data
                .get("sceneItemId")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            let lk = data
                .get("sceneItemLocked")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            match backend.set_scene_item_locked(scene_name, id, lk) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "SetSceneItemTransform" => {
            let scene_name = data.get("sceneName").and_then(|v| v.as_str()).unwrap_or("");
            let id = data
                .get("sceneItemId")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            let tr = data
                .get("sceneItemTransform")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match backend.set_scene_item_transform(scene_name, id, &tr) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "RemoveSceneItem" => {
            let scene_name = data.get("sceneName").and_then(|v| v.as_str()).unwrap_or("");
            let id = data
                .get("sceneItemId")
                .and_then(|v| v.as_i64())
                .unwrap_or(-1);
            match backend.remove_scene_item(scene_name, id) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "GetInputList" => ok_resp(
            req_type,
            &req_id,
            Some(json!({ "inputs": backend.list_inputs() })),
        ),
        "GetInputVolume" => {
            let name = data.get("inputName").and_then(|v| v.as_str()).unwrap_or("");
            match backend.get_input_volume(name) {
                Ok((mul, db)) => ok_resp(
                    req_type,
                    &req_id,
                    Some(json!({ "inputVolumeMul": mul, "inputVolumeDb": db })),
                ),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "SetInputVolume" => {
            let name = data.get("inputName").and_then(|v| v.as_str()).unwrap_or("");
            let mul = data
                .get("inputVolumeMul")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32);
            let db = data
                .get("inputVolumeDb")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32);
            match backend.set_input_volume(name, mul, db) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "GetInputMute" => {
            let name = data.get("inputName").and_then(|v| v.as_str()).unwrap_or("");
            match backend.get_input_mute(name) {
                Ok(muted) => ok_resp(req_type, &req_id, Some(json!({ "inputMuted": muted }))),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "SetInputMute" => {
            let name = data.get("inputName").and_then(|v| v.as_str()).unwrap_or("");
            let muted = data
                .get("inputMuted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            match backend.set_input_mute(name, muted) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "ToggleInputMute" => {
            let name = data.get("inputName").and_then(|v| v.as_str()).unwrap_or("");
            match backend.toggle_input_mute(name) {
                Ok(muted) => ok_resp(req_type, &req_id, Some(json!({ "inputMuted": muted }))),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "GetRecordStatus" => {
            let r = backend.stats().recording;
            ok_resp(
                req_type,
                &req_id,
                Some(json!({
                    "outputActive": matches!(r.state, RecordState::Recording | RecordState::Paused),
                    "outputPaused": r.state == RecordState::Paused,
                    "outputDuration": r.duration_ms,
                    "outputBytes": r.bytes_written,
                })),
            )
        }
        "StartRecord" => match backend.start_record() {
            Ok(()) => ok_resp(req_type, &req_id, None),
            Err(e) => err_resp(
                req_type,
                &req_id,
                request_status::REQUEST_PROCESSING_FAILED,
                &e,
            ),
        },
        "StopRecord" => match backend.stop_record() {
            Ok(p) => ok_resp(req_type, &req_id, Some(json!({ "outputPath": p }))),
            Err(e) => err_resp(
                req_type,
                &req_id,
                request_status::REQUEST_PROCESSING_FAILED,
                &e,
            ),
        },
        "ToggleRecord" => {
            let active = matches!(
                backend.stats().recording.state,
                RecordState::Recording | RecordState::Paused
            );
            if active {
                match backend.stop_record() {
                    Ok(p) => ok_resp(
                        req_type,
                        &req_id,
                        Some(json!({ "outputActive": false, "outputPath": p })),
                    ),
                    Err(e) => err_resp(
                        req_type,
                        &req_id,
                        request_status::REQUEST_PROCESSING_FAILED,
                        &e,
                    ),
                }
            } else {
                match backend.start_record() {
                    Ok(()) => ok_resp(req_type, &req_id, Some(json!({ "outputActive": true }))),
                    Err(e) => err_resp(
                        req_type,
                        &req_id,
                        request_status::REQUEST_PROCESSING_FAILED,
                        &e,
                    ),
                }
            }
        }
        "PauseRecord" => match backend.pause_record() {
            Ok(()) => ok_resp(req_type, &req_id, None),
            Err(e) => err_resp(
                req_type,
                &req_id,
                request_status::REQUEST_PROCESSING_FAILED,
                &e,
            ),
        },
        "ResumeRecord" => match backend.resume_record() {
            Ok(()) => ok_resp(req_type, &req_id, None),
            Err(e) => err_resp(
                req_type,
                &req_id,
                request_status::REQUEST_PROCESSING_FAILED,
                &e,
            ),
        },
        "ToggleRecordPause" => match backend.toggle_record_pause() {
            Ok(paused) => ok_resp(req_type, &req_id, Some(json!({ "outputPaused": paused }))),
            Err(e) => err_resp(
                req_type,
                &req_id,
                request_status::REQUEST_PROCESSING_FAILED,
                &e,
            ),
        },
        "GetStreamStatus" => {
            let s = backend.stats().streaming;
            ok_resp(
                req_type,
                &req_id,
                Some(json!({
                    "outputActive": s.state != StreamState::Stopped,
                    "outputReconnecting": s.state == StreamState::Reconnecting,
                    "outputDuration": s.duration_ms,
                    "outputBytes": s.bytes_sent,
                    "outputSkippedFrames": s.dropped_frames,
                    "outputTotalFrames": s.total_frames,
                })),
            )
        }
        "StartStream" => match backend.start_stream() {
            Ok(()) => ok_resp(req_type, &req_id, None),
            Err(e) => err_resp(
                req_type,
                &req_id,
                request_status::REQUEST_PROCESSING_FAILED,
                &e,
            ),
        },
        "StopStream" => match backend.stop_stream() {
            Ok(()) => ok_resp(req_type, &req_id, None),
            Err(e) => err_resp(
                req_type,
                &req_id,
                request_status::REQUEST_PROCESSING_FAILED,
                &e,
            ),
        },
        "ToggleStream" => {
            let active = backend.stats().streaming.state != StreamState::Stopped;
            let res = if active {
                backend.stop_stream().map(|_| false)
            } else {
                backend.start_stream().map(|_| true)
            };
            match res {
                Ok(now_active) => ok_resp(
                    req_type,
                    &req_id,
                    Some(json!({ "outputActive": now_active })),
                ),
                Err(e) => err_resp(
                    req_type,
                    &req_id,
                    request_status::REQUEST_PROCESSING_FAILED,
                    &e,
                ),
            }
        }
        "GetReplayBufferStatus" => {
            let rb = backend.stats().replay;
            ok_resp(
                req_type,
                &req_id,
                Some(json!({ "outputActive": rb.state != ReplayBufferState::Stopped })),
            )
        }
        "StartReplayBuffer" => match backend.start_replay_buffer() {
            Ok(()) => ok_resp(req_type, &req_id, None),
            Err(e) => err_resp(
                req_type,
                &req_id,
                request_status::REQUEST_PROCESSING_FAILED,
                &e,
            ),
        },
        "StopReplayBuffer" => match backend.stop_replay_buffer() {
            Ok(()) => ok_resp(req_type, &req_id, None),
            Err(e) => err_resp(
                req_type,
                &req_id,
                request_status::REQUEST_PROCESSING_FAILED,
                &e,
            ),
        },
        "ToggleReplayBuffer" => {
            let active = backend.stats().replay.state != ReplayBufferState::Stopped;
            let res = if active {
                backend.stop_replay_buffer().map(|_| false)
            } else {
                backend.start_replay_buffer().map(|_| true)
            };
            match res {
                Ok(now_active) => ok_resp(
                    req_type,
                    &req_id,
                    Some(json!({ "outputActive": now_active })),
                ),
                Err(e) => err_resp(
                    req_type,
                    &req_id,
                    request_status::REQUEST_PROCESSING_FAILED,
                    &e,
                ),
            }
        }
        "SaveReplayBuffer" => match backend.save_replay_buffer() {
            Ok(p) => ok_resp(req_type, &req_id, Some(json!({ "savedReplayPath": p }))),
            Err(e) => err_resp(
                req_type,
                &req_id,
                request_status::REQUEST_PROCESSING_FAILED,
                &e,
            ),
        },
        "GetProfileList" => {
            let (cur, profiles) = backend.list_profiles();
            ok_resp(
                req_type,
                &req_id,
                Some(json!({ "currentProfileName": cur, "profiles": profiles })),
            )
        }
        "SetCurrentProfile" => {
            let name = data
                .get("profileName")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match backend.set_current_profile(name) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "GetSceneCollectionList" => {
            let (cur, cols) = backend.list_scene_collections();
            ok_resp(
                req_type,
                &req_id,
                Some(json!({ "currentSceneCollectionName": cur, "sceneCollections": cols })),
            )
        }
        "SetCurrentSceneCollection" => {
            let name = data
                .get("sceneCollectionName")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match backend.set_current_scene_collection(name) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        "GetHotkeyList" => ok_resp(
            req_type,
            &req_id,
            Some(json!({ "hotkeys": backend.list_hotkey_names() })),
        ),
        "TriggerHotkeyByName" => {
            let name = data
                .get("hotkeyName")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match backend.trigger_hotkey(name) {
                Ok(()) => ok_resp(req_type, &req_id, None),
                Err(e) => err_resp(req_type, &req_id, request_status::RESOURCE_NOT_FOUND, &e),
            }
        }
        _ => err_resp(
            req_type,
            &req_id,
            request_status::UNKNOWN_REQUEST_TYPE,
            "Unsupported or unknown requestType",
        ),
    }
}

fn map_engine_event(evt: &EngineEvent) -> Option<(u64, Value)> {
    match evt {
        EngineEvent::SceneSwitched { scene_name, .. } => Some((
            1 << 2,
            json!({
                "eventType": "CurrentProgramSceneChanged",
                "eventIntent": 4,
                "eventData": { "sceneName": scene_name }
            }),
        )),
        EngineEvent::PreviewSceneChanged { scene_name, .. } => Some((
            1 << 2,
            json!({
                "eventType": "CurrentPreviewSceneChanged",
                "eventIntent": 4,
                "eventData": { "sceneName": scene_name }
            }),
        )),
        EngineEvent::SceneListChanged => Some((
            1 << 2,
            json!({
                "eventType": "SceneListChanged",
                "eventIntent": 4,
                "eventData": {}
            }),
        )),
        EngineEvent::SourceVolumeChanged {
            name,
            volume_db,
            volume_mul,
            ..
        } => Some((
            1 << 3,
            json!({
                "eventType": "InputVolumeChanged",
                "eventIntent": 8,
                "eventData": {
                    "inputName": name,
                    "inputVolumeMul": volume_mul,
                    "inputVolumeDb": volume_db,
                }
            }),
        )),
        EngineEvent::SourceMuteChanged { name, muted, .. } => Some((
            1 << 3,
            json!({
                "eventType": "InputMuteStateChanged",
                "eventIntent": 8,
                "eventData": { "inputName": name, "inputMuted": muted }
            }),
        )),
        EngineEvent::StudioModeChanged { enabled } => Some((
            1 << 10,
            json!({
                "eventType": "StudioModeStateChanged",
                "eventIntent": 1024,
                "eventData": { "studioModeEnabled": enabled }
            }),
        )),
        EngineEvent::RecordStateChanged { state, output_path } => {
            let (active, state_str) = match state {
                RecordState::Stopped => (false, "OBS_WEBSOCKET_OUTPUT_STOPPED"),
                RecordState::Starting => (false, "OBS_WEBSOCKET_OUTPUT_STARTING"),
                RecordState::Recording => (true, "OBS_WEBSOCKET_OUTPUT_STARTED"),
                RecordState::Paused => (true, "OBS_WEBSOCKET_OUTPUT_PAUSED"),
                RecordState::Stopping => (false, "OBS_WEBSOCKET_OUTPUT_STOPPING"),
            };
            Some((
                1 << 6,
                json!({
                    "eventType": "RecordStateChanged",
                    "eventIntent": 64,
                    "eventData": {
                        "outputActive": active,
                        "outputState": state_str,
                        "outputPath": output_path,
                    }
                }),
            ))
        }
        EngineEvent::StreamStateChanged { state, .. } => {
            let (active, state_str) = match state {
                StreamState::Stopped => (false, "OBS_WEBSOCKET_OUTPUT_STOPPED"),
                StreamState::Connecting => (false, "OBS_WEBSOCKET_OUTPUT_STARTING"),
                StreamState::Streaming => (true, "OBS_WEBSOCKET_OUTPUT_STARTED"),
                StreamState::Reconnecting => (false, "OBS_WEBSOCKET_OUTPUT_RECONNECTING"),
                StreamState::Stopping => (false, "OBS_WEBSOCKET_OUTPUT_STOPPING"),
            };
            Some((
                1 << 6,
                json!({
                    "eventType": "StreamStateChanged",
                    "eventIntent": 64,
                    "eventData": {
                        "outputActive": active,
                        "outputState": state_str,
                    }
                }),
            ))
        }
        EngineEvent::ReplaySaved { saved_path } => Some((
            1 << 6,
            json!({
                "eventType": "ReplayBufferSaved",
                "eventIntent": 64,
                "eventData": { "savedReplayPath": saved_path }
            }),
        )),
        EngineEvent::ProfileChanged { profile } => Some((
            1 << 1,
            json!({
                "eventType": "CurrentProfileChanged",
                "eventIntent": 2,
                "eventData": { "profileName": profile }
            }),
        )),
        EngineEvent::SceneCollectionChanged { collection } => Some((
            1 << 1,
            json!({
                "eventType": "CurrentSceneCollectionChanged",
                "eventIntent": 2,
                "eventData": { "sceneCollectionName": collection }
            }),
        )),
        _ => None,
    }
}

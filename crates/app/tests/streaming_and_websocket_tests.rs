use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use obs_core::EventBus;
use obs_rust::engine::ObsEngine;
use obs_streamer::{RtmpClient, RtmpPublishTarget};
use obs_ui::StudioBackend;
use obs_websocket::{start_websocket_server, WebSocketBackend, WebSocketConfig};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ServerSession, ServerSessionConfig, ServerSessionEvent, ServerSessionResult,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn pure_rust_rtmp_client_publishes_to_loopback_server() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let got_publish = Arc::new(AtomicBool::new(false));
    let got_video = Arc::new(AtomicBool::new(false));
    let got_audio = Arc::new(AtomicBool::new(false));

    let gp = Arc::clone(&got_publish);
    let gv = Arc::clone(&got_video);
    let ga = Arc::clone(&got_audio);

    let server_task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut hs = Handshake::new(PeerType::Server);
        let mut buf = [0u8; 4096];
        let leftover;
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            if n == 0 {
                return;
            }
            match hs.process_bytes(&buf[..n]).unwrap() {
                HandshakeProcessResult::InProgress { response_bytes } => {
                    if !response_bytes.is_empty() {
                        socket.write_all(&response_bytes).await.unwrap();
                    }
                }
                HandshakeProcessResult::Completed {
                    response_bytes,
                    remaining_bytes,
                } => {
                    if !response_bytes.is_empty() {
                        socket.write_all(&response_bytes).await.unwrap();
                    }
                    leftover = remaining_bytes;
                    break;
                }
            }
        }

        let (mut srv, init_res) = ServerSession::new(ServerSessionConfig::new()).unwrap();
        for r in init_res {
            if let ServerSessionResult::OutboundResponse(pkt) = r {
                socket.write_all(&pkt.bytes).await.unwrap();
            }
        }
        let mut pending_input = leftover;
        loop {
            if pending_input.is_empty() {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                pending_input.extend_from_slice(&buf[..n]);
            }
            let results = srv.handle_input(&pending_input).unwrap();
            pending_input.clear();
            for r in results {
                match r {
                    ServerSessionResult::OutboundResponse(pkt) => {
                        socket.write_all(&pkt.bytes).await.unwrap();
                    }
                    ServerSessionResult::RaisedEvent(ev) => match ev {
                        ServerSessionEvent::ConnectionRequested { request_id, .. } => {
                            let acc = srv.accept_request(request_id).unwrap();
                            for a in acc {
                                if let ServerSessionResult::OutboundResponse(p) = a {
                                    socket.write_all(&p.bytes).await.unwrap();
                                }
                            }
                        }
                        ServerSessionEvent::PublishStreamRequested {
                            request_id,
                            stream_key,
                            ..
                        } => {
                            assert_eq!(stream_key, "live_test_key");
                            gp.store(true, Ordering::SeqCst);
                            let acc = srv.accept_request(request_id).unwrap();
                            for a in acc {
                                if let ServerSessionResult::OutboundResponse(p) = a {
                                    socket.write_all(&p.bytes).await.unwrap();
                                }
                            }
                        }
                        ServerSessionEvent::VideoDataReceived { data, .. } if !data.is_empty() => {
                            gv.store(true, Ordering::SeqCst);
                        }
                        ServerSessionEvent::AudioDataReceived { data, .. } if !data.is_empty() => {
                            ga.store(true, Ordering::SeqCst);
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            if gp.load(Ordering::SeqCst) && gv.load(Ordering::SeqCst) && ga.load(Ordering::SeqCst) {
                break;
            }
        }
    });

    let target =
        RtmpPublishTarget::parse(&format!("rtmp://127.0.0.1:{port}/live"), "live_test_key")
            .unwrap();
    let mut client = RtmpClient::connect_and_publish(&target).await.unwrap();
    client
        .send_video_data(Bytes::from_static(&[0x17, 0x00, 0x00, 0x00, 0x00]), 0)
        .await
        .unwrap();
    client
        .send_audio_data(Bytes::from_static(&[0xAF, 0x00, 0x11, 0x90]), 0)
        .await
        .unwrap();

    let _ = tokio::time::timeout(std::time::Duration::from_secs(3), server_task).await;
    assert!(got_publish.load(Ordering::SeqCst));
    assert!(got_video.load(Ordering::SeqCst));
    assert!(got_audio.load(Ordering::SeqCst));
}

#[tokio::test]
async fn websocket_v5_protocol_auth_and_rpc_compliance() {
    let engine = ObsEngine::initialize(true);
    let backend: Arc<dyn WebSocketBackend> = engine.clone();
    let eb = EventBus::new(64);

    let mut srv = start_websocket_server(
        WebSocketConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            auth_enabled: true,
            password: "secret_obs_pw".to_string(),
        },
        backend,
        eb,
    )
    .await
    .unwrap();
    let url = format!("ws://127.0.0.1:{}", srv.bound_addr.port());

    // 1. Bad password must be rejected with close code 4009
    {
        let (mut ws_bad, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let _hello = ws_bad.next().await.unwrap().unwrap();
        ws_bad
            .send(Message::Text(
                json!({
                    "op": 1,
                    "d": { "rpcVersion": 1, "authentication": "wrong_hash" }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let closed = ws_bad.next().await.unwrap().unwrap();
        match closed {
            Message::Close(Some(cf)) => assert_eq!(u16::from(cf.code), 4009),
            other => panic!("Expected Close(4009), got {other:?}"),
        }
    }

    // 2. Valid SHA-256 + Base64 auth succeeds and executes RPCs + RequestBatch
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let hello_msg = ws.next().await.unwrap().unwrap().into_text().unwrap();
    let hello: Value = serde_json::from_str(&hello_msg).unwrap();
    assert_eq!(hello["op"], 0);
    let challenge = hello["d"]["authentication"]["challenge"].as_str().unwrap();
    let salt = hello["d"]["authentication"]["salt"].as_str().unwrap();

    let mut h1 = Sha256::new();
    h1.update(b"secret_obs_pw");
    h1.update(salt.as_bytes());
    let sec = B64.encode(h1.finalize());
    let mut h2 = Sha256::new();
    h2.update(sec.as_bytes());
    h2.update(challenge.as_bytes());
    let auth = B64.encode(h2.finalize());

    ws.send(Message::Text(
        json!({
            "op": 1,
            "d": { "rpcVersion": 1, "authentication": auth, "eventSubscriptions": 65535 }
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    let identified: Value =
        serde_json::from_str(&ws.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
    assert_eq!(identified["op"], 2);
    assert_eq!(identified["d"]["negotiatedRpcVersion"], 1);

    // GetVersion + CreateScene + SetStudioModeEnabled via RequestBatch (OpCode 8 -> 9)
    ws.send(Message::Text(
        json!({
            "op": 8,
            "d": {
                "requestId": "batch1",
                "requests": [
                    { "requestType": "GetVersion", "requestId": "r1" },
                    { "requestType": "CreateScene", "requestId": "r2", "requestData": { "sceneName": "WS Scene" } },
                    { "requestType": "SetStudioModeEnabled", "requestId": "r3", "requestData": { "studioModeEnabled": true } },
                    { "requestType": "GetSceneList", "requestId": "r4" }
                ]
            }
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    let batch_resp: Value =
        serde_json::from_str(&ws.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
    assert_eq!(batch_resp["op"], 9);
    let results = batch_resp["d"]["results"].as_array().unwrap();
    assert_eq!(results.len(), 4);
    assert!(results[0]["requestStatus"]["result"].as_bool().unwrap());
    assert!(results[1]["requestStatus"]["result"].as_bool().unwrap());
    assert!(results[2]["requestStatus"]["result"].as_bool().unwrap());
    let scenes = results[3]["responseData"]["scenes"].as_array().unwrap();
    assert!(scenes.iter().any(|s| s["sceneName"] == "WS Scene"));

    srv.stop();
    engine.shutdown();
}

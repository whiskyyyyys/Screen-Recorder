use bytes::Bytes;
use obs_core::{ObsError, Result};
use rml_rtmp::handshake::{Handshake, HandshakeProcessResult, PeerType};
use rml_rtmp::sessions::{
    ClientSession, ClientSessionConfig, ClientSessionEvent, ClientSessionResult, PublishRequestType,
};
use rml_rtmp::time::RtmpTimestamp;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use url::Url;

pub trait RtmpStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> RtmpStream for T {}

pub struct RtmpPublishTarget {
    pub app: String,
    pub stream_key: String,
    pub host: String,
    pub port: u16,
    pub use_tls: bool,
}

impl RtmpPublishTarget {
    pub fn parse(server_url: &str, stream_key: &str) -> Result<Self> {
        let mut normalized = server_url.trim().to_string();
        if !normalized.contains("://") {
            normalized = format!("rtmp://{normalized}");
        }
        let parsed = Url::parse(&normalized)
            .map_err(|e| ObsError::Stream(format!("Invalid RTMP URL '{server_url}': {e}")))?;
        let use_tls = parsed.scheme().eq_ignore_ascii_case("rtmps");
        let host = parsed
            .host_str()
            .ok_or_else(|| ObsError::Stream("RTMP URL missing host".into()))?
            .to_string();
        let port = parsed.port().unwrap_or(if use_tls { 443 } else { 1935 });
        let path_trimmed = parsed.path().trim_matches('/');
        let app = if path_trimmed.is_empty() {
            "live".to_string()
        } else {
            path_trimmed.to_string()
        };
        let key = stream_key.trim().to_string();
        if key.is_empty() {
            return Err(ObsError::Stream("Stream key cannot be empty".into()));
        }
        Ok(Self {
            app,
            stream_key: key,
            host,
            port,
            use_tls,
        })
    }
}

/// Pure-Rust asynchronous RTMP/RTMPS publisher using `rml_rtmp` + Windows Schannel TLS (`native-tls`).
pub struct RtmpClient {
    stream: Pin<Box<dyn RtmpStream>>,
    session: ClientSession,
    pub bytes_sent: u64,
}

impl RtmpClient {
    pub async fn connect_and_publish(target: &RtmpPublishTarget) -> Result<Self> {
        let addr = format!("{}:{}", target.host, target.port);
        let tcp = TcpStream::connect(&addr)
            .await
            .map_err(|e| ObsError::Stream(format!("TCP connect to {addr} failed: {e}")))?;
        let _ = tcp.set_nodelay(true);

        let mut stream: Pin<Box<dyn RtmpStream>> = if target.use_tls {
            let cx = native_tls::TlsConnector::new()
                .map_err(|e| ObsError::Stream(format!("TLS init failed: {e}")))?;
            let cx = tokio_native_tls::TlsConnector::from(cx);
            let tls = cx
                .connect(&target.host, tcp)
                .await
                .map_err(|e| ObsError::Stream(format!("RTMPS TLS handshake failed: {e}")))?;
            Box::pin(tls)
        } else {
            Box::pin(tcp)
        };

        // RTMP C0/C1/C2 Handshake
        let mut hs = Handshake::new(PeerType::Client);
        let c0_c1 = hs
            .generate_outbound_p0_and_p1()
            .map_err(|e| ObsError::Stream(format!("RTMP handshake C0/C1 error: {e:?}")))?;
        stream.write_all(&c0_c1).await?;
        stream.flush().await?;

        let mut buf = [0u8; 4096];
        let leftover;
        loop {
            let n = stream.read(&mut buf).await?;
            if n == 0 {
                return Err(ObsError::Stream(
                    "RTMP server closed during handshake".into(),
                ));
            }
            match hs
                .process_bytes(&buf[..n])
                .map_err(|e| ObsError::Stream(format!("RTMP handshake error: {e:?}")))?
            {
                HandshakeProcessResult::InProgress { response_bytes } => {
                    if !response_bytes.is_empty() {
                        stream.write_all(&response_bytes).await?;
                        stream.flush().await?;
                    }
                }
                HandshakeProcessResult::Completed {
                    response_bytes,
                    remaining_bytes,
                } => {
                    if !response_bytes.is_empty() {
                        stream.write_all(&response_bytes).await?;
                        stream.flush().await?;
                    }
                    leftover = remaining_bytes;
                    break;
                }
            }
        }

        let (session, init_results) = ClientSession::new(ClientSessionConfig::new())
            .map_err(|e| ObsError::Stream(format!("ClientSession::new: {e:?}")))?;

        let mut client = Self {
            stream,
            session,
            bytes_sent: 0,
        };
        client.handle_results(init_results).await?;

        let conn_res = client
            .session
            .request_connection(target.app.clone())
            .map_err(|e| ObsError::Stream(format!("request_connection: {e:?}")))?;
        client.handle_results(vec![conn_res]).await?;

        if !leftover.is_empty() {
            let res = client
                .session
                .handle_input(&leftover)
                .map_err(|e| ObsError::Stream(format!("handle_input leftover: {e:?}")))?;
            client.handle_results(res).await?;
        }

        // Wait until ConnectionRequestAccepted, then request_publishing and wait for PublishRequestAccepted
        let mut accepted_conn = false;
        let mut publishing_ready = false;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);

        while !publishing_ready {
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(ObsError::Stream(
                    "Timed out waiting for RTMP publish acceptance".into(),
                ));
            }
            let n = tokio::time::timeout(deadline - now, client.stream.read(&mut buf))
                .await
                .map_err(|_| ObsError::Stream("RTMP publish timeout".into()))??;
            if n == 0 {
                return Err(ObsError::Stream(
                    "RTMP server disconnected before publish".into(),
                ));
            }
            let results = client
                .session
                .handle_input(&buf[..n])
                .map_err(|e| ObsError::Stream(format!("RTMP input error: {e:?}")))?;

            for res in results {
                match res {
                    ClientSessionResult::OutboundResponse(packet) => {
                        client.bytes_sent += packet.bytes.len() as u64;
                        client.stream.write_all(&packet.bytes).await?;
                    }
                    ClientSessionResult::RaisedEvent(evt) => match evt {
                        ClientSessionEvent::ConnectionRequestAccepted => {
                            accepted_conn = true;
                            let pub_res = client
                                .session
                                .request_publishing(
                                    target.stream_key.clone(),
                                    PublishRequestType::Live,
                                )
                                .map_err(|e| {
                                    ObsError::Stream(format!("request_publishing: {e:?}"))
                                })?;
                            client.handle_results(vec![pub_res]).await?;
                        }
                        ClientSessionEvent::ConnectionRequestRejected { description } => {
                            return Err(ObsError::Stream(format!(
                                "RTMP connection rejected: {description}"
                            )));
                        }
                        ClientSessionEvent::PublishRequestAccepted => {
                            publishing_ready = true;
                        }
                        ClientSessionEvent::StreamMetadataReceived { .. } => {}
                        _ => {}
                    },
                    ClientSessionResult::UnhandleableMessageReceived(_) => {}
                }
            }
            client.stream.flush().await?;
        }

        let _ = accepted_conn;
        Ok(client)
    }

    pub async fn send_video_data(&mut self, data: Bytes, timestamp_ms: u32) -> Result<()> {
        let res = self
            .session
            .publish_video_data(data, RtmpTimestamp::new(timestamp_ms), true)
            .map_err(|e| ObsError::Stream(format!("publish_video_data: {e:?}")))?;
        self.handle_results(vec![res]).await
    }

    pub async fn send_audio_data(&mut self, data: Bytes, timestamp_ms: u32) -> Result<()> {
        let res = self
            .session
            .publish_audio_data(data, RtmpTimestamp::new(timestamp_ms), true)
            .map_err(|e| ObsError::Stream(format!("publish_audio_data: {e:?}")))?;
        self.handle_results(vec![res]).await
    }

    pub async fn send_metadata(
        &mut self,
        metadata: rml_rtmp::sessions::StreamMetadata,
    ) -> Result<()> {
        let res = self
            .session
            .publish_metadata(&metadata)
            .map_err(|e| ObsError::Stream(format!("publish_metadata: {e:?}")))?;
        self.handle_results(vec![res]).await
    }

    pub async fn poll_inbound_nonblocking(&mut self) -> Result<()> {
        let mut buf = [0u8; 2048];
        if let Ok(Ok(n)) = tokio::time::timeout(
            std::time::Duration::from_millis(1),
            self.stream.read(&mut buf),
        )
        .await
        {
            if n == 0 {
                return Err(ObsError::Stream("RTMP server closed socket".into()));
            }
            let res = self
                .session
                .handle_input(&buf[..n])
                .map_err(|e| ObsError::Stream(format!("RTMP inbound: {e:?}")))?;
            self.handle_results(res).await?;
        }
        Ok(())
    }

    async fn handle_results(&mut self, results: Vec<ClientSessionResult>) -> Result<()> {
        for r in results {
            if let ClientSessionResult::OutboundResponse(packet) = r {
                self.bytes_sent += packet.bytes.len() as u64;
                self.stream.write_all(&packet.bytes).await?;
            }
        }
        self.stream.flush().await?;
        Ok(())
    }
}

#[allow(dead_code)]
fn _poll_marker(_cx: &mut Context<'_>, _rb: &mut ReadBuf<'_>) -> Poll<()> {
    Poll::Ready(())
}

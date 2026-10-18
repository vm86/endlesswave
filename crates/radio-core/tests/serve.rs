//! The server on a real socket: the API answers, a stream goes on, and the server stops when it is told to. Stage 4 of
//! docs/rust-port.md. The client is a bare TCP connection, to see the bytes as another program would.

use bytes::Bytes;
use radio_core::{
    api::{Api, serve},
    backend::{
        MusicBackend,
        fake::{FakeBackend, FakeOptions},
    },
    clock::ManualClock,
    encode::Streams,
    radio::{Radio, RadioConfig},
    spectrum::{SpectrumFeed, StartPositions},
    stream::broadcaster::{Broadcaster, Config},
};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

async fn get(address: std::net::SocketAddr, request: &str) -> TcpStream {
    let mut socket = TcpStream::connect(address).await.unwrap();
    socket.write_all(request.as_bytes()).await.unwrap();
    socket
}

#[tokio::test]
async fn the_server_answers_streams_and_stops() {
    let workdir = std::env::temp_dir().join(format!("radio-core-serve-{}", std::process::id()));
    std::fs::create_dir_all(&workdir).unwrap();
    let radio = Radio::new(
        Arc::new(FakeBackend::new(Vec::new(), FakeOptions::default())) as Arc<dyn MusicBackend>,
        workdir,
        RadioConfig::default(),
        None,
        Arc::new(ManualClock::new(0.0)),
    );
    let streams = Streams {
        mp3: Broadcaster::new(Config::mp3()),
        flac: None,
        ogg: None,
    };
    let hub = streams.mp3.clone();
    let api = Api {
        radio,
        backends: None,
        streams,
        spectrum: Some(SpectrumFeed::new()),
        starts: Arc::new(StartPositions::new()),
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(listener, api, async move {
        let _ = stopped.await;
    }));

    // the API
    let mut socket = get(address, "GET /api/pause HTTP/1.0\r\n\r\n").await;
    let mut text = String::new();
    socket.read_to_string(&mut text).await.unwrap();
    assert!(text.starts_with("HTTP/1.0 405"), "{text}");
    let mut socket = get(address, "POST /api/pause HTTP/1.0\r\n\r\n").await;
    text.clear();
    socket.read_to_string(&mut text).await.unwrap();
    assert!(text.starts_with("HTTP/1.0 200 OK"), "{text}");
    assert!(
        text.to_lowercase()
            .contains("content-type: application/json; charset=utf-8")
    );
    assert!(
        text.ends_with("{\"pause_reason\":\"user\",\"paused\":true}")
            || text.ends_with("{\"paused\":true,\"pause_reason\":\"user\"}"),
        "{text}"
    );

    // a stream: the headers, then what is published
    let mut socket = get(
        address,
        "GET /stream?l=me HTTP/1.1\r\nHost: x\r\nIcy-MetaData: 1\r\n\r\n",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(hub.has_listeners());
    hub.set_title("Artist - Song");
    hub.publish(Bytes::from(vec![0xAB; 20000]));
    let mut received = Vec::new();
    let mut buffer = [0u8; 8192];
    while received.windows(4).position(|w| w == b"\r\n\r\n").is_none() || received.len() < 16200 {
        let n = tokio::time::timeout(Duration::from_secs(5), socket.read(&mut buffer))
            .await
            .expect("data came")
            .unwrap();
        assert!(n > 0, "the stream ended");
        received.extend_from_slice(&buffer[..n]);
    }
    let head = String::from_utf8_lossy(
        &received[..received.windows(4).position(|w| w == b"\r\n\r\n").unwrap()],
    )
    .to_lowercase();
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(
        head.contains("icy-metaint: 16000")
            && head.contains("content-type: audio/mpeg")
            && head.contains("access-control-allow-origin: *"),
        "{head}"
    );

    // leaving the stream is dropping the connection
    drop(socket);
    for _ in 0..50 {
        if !hub.has_listeners() {
            break;
        }
        hub.publish(Bytes::from(vec![0u8; 1000]));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !hub.has_listeners(),
        "the listener is gone with its connection"
    );

    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the server stopped")
        .unwrap()
        .unwrap();
}

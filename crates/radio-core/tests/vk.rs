//! The VK backend against a mock of the API of VK: signing in with a token, the wave, the music of the user, likes and the
//! download of a file.

use axum::{
    Router,
    extract::{Request, State},
    http::StatusCode,
    response::IntoResponse,
};
use radio_core::backend::{
    MusicBackend, Track,
    vk::{VkBackend, VkOptions},
    yandex::{MemoryStore, SettingsStore},
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

type Log = Arc<Mutex<Vec<String>>>;

#[derive(Clone, Default)]
struct Mock {
    log: Log,
    origin: Arc<Mutex<String>>,
}

async fn handle(State(mock): State<Mock>, request: Request) -> axum::response::Response {
    let path = request.uri().path().to_owned();
    let body = axum::body::to_bytes(request.into_body(), 1 << 20)
        .await
        .unwrap();
    let form: HashMap<String, String> = url_pairs(&String::from_utf8_lossy(&body));
    mock.log.lock().unwrap().push(format!(
        "{path} {}",
        form.iter()
            .filter(|(k, _)| !["v", "client_id", "access_token"].contains(&k.as_str()))
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join("&")
    ));
    if let Some(file) = path.strip_prefix("/file/") {
        return (StatusCode::OK, format!("audio of {file}")).into_response();
    }
    let token = form.get("access_token").map_or("", String::as_str);
    if token != "good" {
        return json_answer(
            json!({"error": {"error_code": 5, "error_msg": "User authorization failed"}}),
        );
    }
    let method = path.trim_start_matches("/method/");
    let origin = mock.origin.lock().unwrap().clone();
    let audio = |id: u64, url: &str| {
        json!({"owner_id": 100, "id": id, "title": format!("Song {id}"), "artist": "Band",
               "access_key": "ak", "url": url})
    };
    let answer = match method {
        "users.get" => json!({"response": [{"id": 42}]}),
        "audio.getStreamMixAudios" => {
            json!({"response": [audio(1, "https://x/1"), audio(2, "https://x/2")]})
        }
        "audio.get" => json!({"response": {"count": 2, "items": [audio(1, ""), audio(3, "")]}}),
        "audio.getById" => {
            json!({"response": [audio(1, &format!("{origin}/file/1"))]})
        }
        "audio.add" | "audio.delete" => json!({"response": 1}),
        _ => json!({"error": {"error_code": 3, "error_msg": "unknown"}}),
    };
    json_answer(answer)
}

fn json_answer(value: Value) -> axum::response::Response {
    (StatusCode::OK, value.to_string()).into_response()
}

fn url_pairs(text: &str) -> HashMap<String, String> {
    text.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (percent(k), percent(v)))
        .collect()
}

fn percent(text: &str) -> String {
    let bytes = text.replace('+', " ");
    let bytes = bytes.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() + 0 {
            if let Ok(b) = u8::from_str_radix(&String::from_utf8_lossy(&bytes[i + 1..i + 3]), 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[tokio::test]
async fn vk_signs_in_with_a_token_plays_the_wave_and_the_music_and_likes() {
    let mock = Mock::default();
    let log = mock.log.clone();
    let app = Router::new().fallback(handle).with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    *mock.origin.lock().unwrap() = origin.clone();
    tokio::spawn(async move { axum::serve(listener, app).await });

    let store = Arc::new(MemoryStore::default());
    let backend = VkBackend::new(
        store.clone(),
        VkOptions {
            api: format!("{origin}/method/"),
        },
    )
    .unwrap();
    assert!(!backend.authorized());
    assert_eq!(backend.auth_prompt().fields[0].name, "token");

    let bad = HashMap::from([("token".to_owned(), "bad".to_owned())]);
    assert!(backend.authorize(&bad).await.is_err());
    assert!(backend.authorize(&HashMap::new()).await.is_err());
    assert!(!backend.authorized());

    let good = HashMap::from([("token".to_owned(), " good ".to_owned())]);
    backend.authorize(&good).await.unwrap();
    assert!(backend.authorized());
    let kept = Value::Object(store.load());
    assert_eq!(
        (kept["token"].as_str(), kept["user_id"].as_i64()),
        (Some("good"), Some(42))
    );

    // the wave: the tracks come with their access keys, and are not offered twice
    let none = HashSet::new();
    let wave = backend.station_batch("wave", &none).await.unwrap();
    assert_eq!(
        wave.iter().map(|t| t.track_id.as_str()).collect::<Vec<_>>(),
        ["100_1", "100_2"]
    );
    assert_eq!(
        (
            wave[0].title.as_str(),
            wave[0].artists.clone(),
            wave[0].batch_id.as_str()
        ),
        ("Song 1", vec!["Band".to_owned()], "ak")
    );
    assert!(
        backend
            .station_batch("wave", &none)
            .await
            .unwrap()
            .is_empty()
    );

    // the music of the user: ids by `audio.get`, then tracks with links by `audio.getById`
    let liked = backend.liked_track_ids().await.unwrap();
    assert_eq!(
        liked,
        ["100_1", "100_3"].into_iter().map(str::to_owned).collect()
    );
    let favorites = backend.station_batch("favorites", &none).await.unwrap();
    assert_eq!(favorites.len(), 1, "the mock gives one audio for any ids");

    // likes
    let track: Track = wave[0].clone();
    backend.set_like(&track, true).await.unwrap();
    backend.set_like(&track, false).await.unwrap();

    // the file of a track
    let dir = std::env::temp_dir().join(format!("radio-core-vk-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("1.mp3");
    backend.download(&track, &file).await.unwrap();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "audio of 1");
    let log_text = log.lock().unwrap().join("\n");
    assert!(
        log_text.contains("/method/audio.add audio_id=1&access_key=ak&owner_id=100")
            || log_text.contains("/method/audio.add access_key=ak&audio_id=1&owner_id=100"),
        "{log_text}"
    );
    assert!(
        log_text.contains("/method/audio.delete audio_id=1&owner_id=100"),
        "{log_text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_token_that_vk_refuses_is_dropped() {
    let app = Router::new().fallback(handle).with_state(Mock::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });

    let store = Arc::new(MemoryStore::default());
    store
        .save(&serde_json::from_str(r#"{"token": "old", "user_id": 42}"#).unwrap())
        .unwrap();
    let backend = VkBackend::new(
        store,
        VkOptions {
            api: format!("{origin}/method/"),
        },
    )
    .unwrap();
    assert!(backend.authorized());
    let error = backend
        .station_batch("wave", &HashSet::new())
        .await
        .unwrap_err();
    assert!(error.0.contains("sign in again"), "{error}");
    assert!(!backend.authorized(), "the radio asks for a new token");
}

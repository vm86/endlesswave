//! The Yandex backend against the original Python one (its answers are kept in `tests/golden/yandex/expected.json`). Task
//! 5.3 of docs/rust-port.md.
//!
//! The service is a mock that answers as the scenario says, and writes down what it is asked: the Python backend was run
//! against it, and the Rust one must send the same requests (the method, the path, the query, the headers that matter, the
//! body) and make the same of the answers.

use axum::{
    Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use radio_core::{
    backend::{
        Feedback, MusicBackend, Track,
        yandex::{DbStore, MemoryStore, SettingsStore, YandexBackend, YandexOptions},
    },
    cache::TrackCache,
    clock::ManualClock,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const WALL: f64 = 1_700_000_000.0;

struct Mock {
    routes: Vec<Value>,
    calls: Mutex<HashMap<usize, usize>>,
    log: Mutex<Vec<Value>>,
    origin: Mutex<String>,
}

fn header(headers: &HeaderMap, name: &str) -> Value {
    headers
        .get(name)
        .map_or(Value::Null, |v| json!(v.to_str().unwrap()))
}

async fn handle(State(mock): State<Arc<Mock>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let raw: Bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let kind = header(&parts.headers, "content-type");
    let kind_text = kind.as_str().unwrap_or("");
    let body: Value = if kind_text.contains("json") {
        serde_json::from_slice(&raw).unwrap()
    } else if kind_text.contains("x-www-form-urlencoded") {
        let mut form = serde_json::Map::new();
        for pair in String::from_utf8(raw.to_vec()).unwrap().split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = percent(value);
            form.insert(
                key.to_owned(),
                if key == "device_id" {
                    json!("<id>")
                } else {
                    json!(value)
                },
            );
        }
        Value::Object(form)
    } else if raw.is_empty() {
        Value::Null
    } else {
        json!(String::from_utf8(raw.to_vec()).unwrap())
    };
    let mut query: Vec<(String, String)> = parts
        .uri
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent(key), percent(value))
        })
        .collect();
    query.sort();
    let method = parts.method.as_str().to_owned();
    let path = percent(parts.uri.path());
    mock.log.lock().unwrap().push(json!({
        "method": method,
        "path": path,
        "query": query.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
        "headers": if path.starts_with("/dl/") { Value::Null } else { json!({
            "x-yandex-music-client": header(&parts.headers, "x-yandex-music-client"),
            "user-agent": header(&parts.headers, "user-agent"),
            "authorization": header(&parts.headers, "authorization"),
            "content-type": kind,
        }) },
        "body": body,
    }));

    let answer = mock.routes.iter().enumerate().find(|(_, route)| {
        route["method"] == method.as_str()
            && route["path"] == path.as_str()
            && route["when"].as_object().is_none_or(|when| {
                when.iter().all(|(k, v)| {
                    query
                        .iter()
                        .any(|(qk, qv)| qk == k && Some(qv.as_str()) == v.as_str())
                })
            })
    });
    let origin = mock.origin.lock().unwrap().clone();
    let Some((index, route)) = answer else {
        return (
            StatusCode::NOT_FOUND,
            [("content-type", "application/json")],
            r#"{"error": "no route"}"#,
        )
            .into_response();
    };
    let turn = {
        let mut calls = mock.calls.lock().unwrap();
        let turn = calls.entry(index).or_insert(0);
        *turn += 1;
        *turn - 1
    };
    let answers = route["responses"].as_array().unwrap();
    let answer = &answers[turn.min(answers.len() - 1)];
    let status = StatusCode::from_u16(answer["status"].as_u64().unwrap() as u16).unwrap();
    match answer.get("text").and_then(Value::as_str) {
        Some(text) => (status, text.replace("<mock>", &origin)).into_response(),
        None => (
            status,
            [("content-type", "application/json")],
            answer["json"].to_string().replace("<mock>", &origin),
        )
            .into_response(),
    }
}

fn percent(text: &str) -> String {
    let bytes = text.replace('+', " ").into_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() + 0 && i + 2 <= bytes.len() - 1 {
            out.push(
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap(), 16).unwrap(),
            );
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn track(value: &Value) -> Track {
    serde_json::from_value(value.clone()).unwrap()
}

/// The error, as the Python harness writes it (what a failed download says of the HTTP error is not compared).
fn error_text(kind: &str, message: &str, origin: &str) -> String {
    let text = format!("{kind}: {message}").replace(origin, "<mock>");
    match text
        .find("download of ")
        .and_then(|at| text[at..].find(": ").map(|colon| (at, colon)))
    {
        Some((at, colon)) => {
            let rest = &text[at + colon + 2..];
            // after "from <url>"
            if text[at..].contains(" from ") && rest.contains(&text[at + 12..at + colon]) == false {
                match text[at..]
                    .find(" from ")
                    .and_then(|f| text[at + f..].find(": ").map(|c| at + f + c))
                {
                    Some(cut) => text[..cut].to_owned(),
                    None => text,
                }
            } else {
                text
            }
        }
        None => text,
    }
}

fn first_difference(got: &Value, want: &Value, path: &str) -> Option<String> {
    match (got, want) {
        (Value::Object(a), Value::Object(b)) => a.keys().chain(b.keys()).find_map(|key| {
            first_difference(
                a.get(key).unwrap_or(&Value::Null),
                b.get(key).unwrap_or(&Value::Null),
                &format!("{path}.{key}"),
            )
        }),
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => a
            .iter()
            .zip(b)
            .enumerate()
            .find_map(|(i, (x, y))| first_difference(x, y, &format!("{path}[{i}]"))),
        _ if got == want => None,
        _ => Some(format!("{path}: {got} against {want}")),
    }
}

/// Requests sent at once come in any order: runs of get-file-info are put in order.
fn settle(requests: Vec<Value>) -> Vec<Value> {
    let mut out = Vec::new();
    let mut run: Vec<Value> = Vec::new();
    let flush = |run: &mut Vec<Value>, out: &mut Vec<Value>| {
        run.sort_by_key(|r| r.to_string());
        out.append(run);
    };
    for request in requests {
        if request["path"] == "/get-file-info" {
            run.push(request);
            continue;
        }
        flush(&mut run, &mut out);
        out.push(request);
    }
    flush(&mut run, &mut out);
    out
}

#[tokio::test]
async fn the_yandex_backend_sends_and_reads_as_the_python_one_does() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/yandex/expected.json");
    let text = fs::read_to_string(&path).unwrap();
    let scenarios: Vec<Value> = serde_json::from_str(&text).unwrap();
    let scratch = std::env::temp_dir().join(format!("radio-core-yandex-{}", std::process::id()));
    fs::create_dir_all(&scratch).unwrap();

    for scenario in &scenarios {
        let name = scenario["name"].as_str().unwrap();
        let dir = scratch.join(name);
        fs::create_dir_all(&dir).unwrap();
        let mock = Arc::new(Mock {
            routes: scenario["routes"].as_array().unwrap().clone(),
            calls: Mutex::default(),
            log: Mutex::default(),
            origin: Mutex::default(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        *mock.origin.lock().unwrap() = origin.clone();
        let app = Router::new().fallback(handle).with_state(mock.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // The settings are in the database of the radio, as in the app.
        let db_path = dir.join("endlesswave.db");
        let cache = Arc::new(Mutex::new(TrackCache::open(&db_path).unwrap()));
        let store = Arc::new(DbStore::new(cache));
        if let Value::Object(settings) = &scenario["settings"] {
            store.save(settings).unwrap();
        }
        let delays = scenario
            .get("rate_limit_delays")
            .and_then(Value::as_array)
            .map_or_else(
                || vec![0.01, 0.02, 0.04, 0.08],
                |d| d.iter().map(|v| v.as_f64().unwrap()).collect(),
            );
        let backend = YandexBackend::new(
            store.clone(),
            YandexOptions {
                api: origin.clone(),
                oauth: origin.clone(),
                rate_limit_delays: delays,
            },
            Arc::new(ManualClock::new(WALL)),
        )
        .unwrap();

        let mut sent = 0;
        for (index, step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
            let want = &scenario["results"][index];
            let op = step["op"].as_str().unwrap();
            let mut got = serde_json::Map::new();
            let failed = |kind: &str,
                          error: &dyn std::fmt::Display,
                          got: &mut serde_json::Map<String, Value>| {
                got.insert(
                    "error".to_owned(),
                    json!(error_text(kind, &error.to_string(), &origin)),
                );
            };
            match op {
                "batch" => {
                    let exclude = step["exclude"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|v| v.as_str().unwrap().to_owned())
                        .collect();
                    match backend
                        .station_batch(step["station"].as_str().unwrap(), &exclude)
                        .await
                    {
                        Ok(tracks) => {
                            got.insert("tracks".to_owned(), serde_json::to_value(tracks).unwrap());
                        }
                        Err(error) => failed("BackendError", &error, &mut got),
                    }
                }
                "download" => {
                    let target = dir.join(step["name"].as_str().unwrap());
                    match backend.download(&track(&step["track"]), &target).await {
                        Ok(()) => {
                            got.insert("file".to_owned(), json!(fs::read_to_string(&target).ok()));
                        }
                        Err(error) => failed("BackendError", &error, &mut got),
                    }
                }
                "feedback" => {
                    let kind = match step["kind"].as_str().unwrap() {
                        "started" => Feedback::Started,
                        "finished" => Feedback::Finished,
                        _ => Feedback::Skipped,
                    };
                    backend
                        .feedback_track(
                            &track(&step["track"]),
                            kind,
                            step["station"].as_str().unwrap(),
                            step["seconds"].as_f64().unwrap(),
                        )
                        .await
                        .unwrap();
                }
                "station_settings" => {
                    let (a, b, c) = backend
                        .station_settings(step["station"].as_str().unwrap())
                        .await
                        .unwrap();
                    got.insert("settings".to_owned(), json!([a, b, c]));
                }
                "set_station_settings" => {
                    let s = |key: &str| step[key].as_str().unwrap().to_owned();
                    backend
                        .set_station_settings(
                            &s("station"),
                            &s("mood"),
                            &s("diversity"),
                            &s("language"),
                        )
                        .await
                        .unwrap();
                }
                "authorize" => {
                    if let Err(error) = backend.authorize(&HashMap::new()).await {
                        failed("AuthError", &error, &mut got);
                    }
                }
                "auth" => {
                    backend.start_auth();
                    for _ in 0..500 {
                        if backend.auth_prompt().code.is_some() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    let prompt = backend.auth_prompt();
                    got.insert(
                        "prompt".to_owned(),
                        json!({"url": prompt.url, "code": prompt.code}),
                    );
                    tokio::time::timeout(Duration::from_secs(10), backend.wait_authorized())
                        .await
                        .unwrap();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                "query" => {}
                other => panic!("{other}"),
            }
            let requests: Vec<Value> = {
                let log = mock.log.lock().unwrap();
                let new = log[sent..].to_vec();
                sent = log.len();
                new
            };
            got.insert("requests".to_owned(), json!(settle(requests)));
            // The database holds the token: only its owner reads it. (Python kept a file; the mode of the file in the
            // golden data is not the point, so it is left out of the comparison.)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(&db_path).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o600, "scenario {name}: the mode of the database");
            }
            got.insert(
                "state".to_owned(),
                json!({
                    "authorized": backend.authorized(),
                    "prompt": backend.auth_prompt(),
                    "settings_file": Some(store.load()).filter(|m| !m.is_empty()),
                }),
            );
            let mut want = want.clone();
            want["state"]
                .as_object_mut()
                .unwrap()
                .remove("settings_mode");
            if let Some(difference) = first_difference(&Value::Object(got), &want, "") {
                panic!("scenario {name}, step {index} ({op}): {difference}");
            }
        }
        backend.aclose().await;
        server.abort();
    }
    let _ = fs::remove_dir_all(&scratch);
}

#[tokio::test]
async fn the_quality_is_a_setting_of_the_backend_and_is_kept() {
    let dir =
        std::env::temp_dir().join(format!("radio-core-yandex-quality-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Arc::new(MemoryStore::default());
    store
        .save(
            &serde_json::from_str::<serde_json::Map<String, Value>>(
                r#"{"token": "t", "from_id": "music-x", "quality": "lossless"}"#,
            )
            .unwrap(),
        )
        .unwrap();
    let backend = YandexBackend::new(
        store.clone(),
        YandexOptions::default(),
        Arc::new(ManualClock::new(WALL)),
    )
    .unwrap();

    let (values, fields) = backend.settings().expect("Yandex has settings");
    assert_eq!(values["quality"], "lossless");
    let quality = fields.iter().find(|f| f.name == "quality").unwrap();
    let offered: Vec<&str> = quality.choices.iter().map(|c| c.value.as_str()).collect();
    assert_eq!(offered, ["lossless", "hq", "nq"]);
    assert_eq!(values["tracks"], "wave", "the wave unless chosen otherwise");
    let tracks = fields.iter().find(|f| f.name == "tracks").unwrap();
    let offered: Vec<&str> = tracks.choices.iter().map(|c| c.value.as_str()).collect();
    assert_eq!(offered, ["wave", "favorites"]);

    backend
        .update_settings(&HashMap::from([("quality".to_owned(), "hq".to_owned())]))
        .await
        .unwrap();
    assert_eq!(backend.settings().unwrap().0["quality"], "hq");
    let kept = Value::Object(store.load());
    assert_eq!(
        (kept["quality"].as_str(), kept["token"].as_str()),
        (Some("hq"), Some("t")),
        "the token stays"
    );

    for bad in [
        HashMap::from([("quality".to_owned(), "ultra".to_owned())]),
        HashMap::new(),
    ] {
        assert!(backend.update_settings(&bad).await.is_err());
    }
    assert_eq!(
        backend.settings().unwrap().0["quality"],
        "hq",
        "a refused value changes nothing"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_settings_file_of_an_earlier_version_goes_into_the_store_and_is_removed() {
    use radio_core::backend::yandex::{has_token, import_settings_file};
    let dir = std::env::temp_dir().join(format!("radio-core-yandex-import-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("endlesswave.json");
    std::fs::write(
        &file,
        r#"{"token": "old", "from_id": "music-1", "quality": "hq"}"#,
    )
    .unwrap();

    let store = MemoryStore::default();
    assert!(!has_token(&store));
    // what the store has already is not replaced
    store
        .save(&serde_json::from_str(r#"{"quality": "nq"}"#).unwrap())
        .unwrap();
    let done = import_settings_file(&file, &store)
        .unwrap()
        .expect("the file was there");
    assert!(done.contains("token"), "{done}");
    let kept = store.load();
    assert_eq!(
        (
            kept["token"].as_str(),
            kept["from_id"].as_str(),
            kept["quality"].as_str()
        ),
        (Some("old"), Some("music-1"), Some("nq"))
    );
    assert!(has_token(&store));
    assert!(!file.exists(), "the file is removed");
    assert!(
        import_settings_file(&file, &store).unwrap().is_none(),
        "nothing to do the second time"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn likes_are_read_added_and_taken_back_on_the_account() {
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = log.clone();
    let app = Router::new().fallback(move |request: Request| {
        let seen = seen.clone();
        async move {
            let line = format!("{} {}", request.method(), request.uri().path());
            let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                .await
                .unwrap();
            seen.lock()
                .unwrap()
                .push(format!("{line} {}", String::from_utf8_lossy(&body)));
            let answer = if line.contains("/account/status") {
                json!({"result": {"account": {"uid": 42}}})
            } else if line.starts_with("GET") {
                json!({"result": {"library": {"tracks": [{"id": "7", "albumId": "1"}, {"id": 8}]}}})
            } else {
                json!({"result": {"revision": 5}})
            };
            (StatusCode::OK, answer.to_string()).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });

    let store = Arc::new(MemoryStore::default());
    store
        .save(&serde_json::from_str(r#"{"token": "t", "from_id": "music-x"}"#).unwrap())
        .unwrap();
    let options = YandexOptions {
        api: origin,
        ..YandexOptions::default()
    };
    let backend = YandexBackend::new(store, options, Arc::new(ManualClock::new(WALL))).unwrap();
    assert!(backend.supports_likes());

    let track: Track = serde_json::from_value(json!({
        "track_id": "7", "title": "t", "artists": [], "codec": "mp3"
    }))
    .unwrap();
    let other: Track = serde_json::from_value(json!({
        "track_id": "9", "title": "t", "artists": [], "codec": "mp3"
    }))
    .unwrap();
    let liked = backend.liked_among(&[track.clone(), other]).await.unwrap();
    assert_eq!(liked, ["7"].into_iter().map(str::to_owned).collect());

    backend.set_like(&track, true).await.unwrap();
    backend.set_like(&track, false).await.unwrap();

    let log = log.lock().unwrap().clone();
    assert_eq!(
        log,
        [
            "GET /account/status ",
            "GET /users/42/likes/tracks ",
            "POST /users/42/likes/tracks/add-multiple track-ids=7",
            "POST /users/42/likes/tracks/remove track-ids=7",
        ],
        "the account is asked for once"
    );
}

#[tokio::test]
async fn the_tracks_can_come_from_the_liked_tracks_instead_of_the_wave() {
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = log.clone();
    let app = Router::new().fallback(move |request: Request| {
        let seen = seen.clone();
        async move {
            let line = format!("{} {}", request.method(), request.uri().path());
            let body = axum::body::to_bytes(request.into_body(), 1 << 20)
                .await
                .unwrap();
            seen.lock()
                .unwrap()
                .push(format!("{line} {}", String::from_utf8_lossy(&body)));
            let track = |id: u32, album: u32| {
                json!({"id": id.to_string(), "title": format!("Song {id}"), "available": true,
                       "artists": [{"name": "Band"}], "albums": [{"id": album, "genre": "rock"}]})
            };
            let answer = if line.contains("/account/status") {
                json!({"result": {"account": {"uid": 42}}})
            } else if line.contains("/likes/tracks") {
                json!({"result": {"library": {"tracks": [{"id": "7", "albumId": "1"}, {"id": "8", "albumId": "2"}]}}})
            } else if line.starts_with("POST /tracks") {
                json!({"result": [track(7, 1), track(8, 2)]})
            } else if line.contains("/get-file-info") {
                json!({"result": {"downloadInfo": {"codec": "mp3", "bitrate": 320, "urls": ["http://x/y"]}}})
            } else {
                json!({"result": {"batchId": "b", "sequence": []}})
            };
            (StatusCode::OK, answer.to_string()).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await });

    let store = Arc::new(MemoryStore::default());
    store
        .save(&serde_json::from_str(r#"{"token": "t", "from_id": "music-x"}"#).unwrap())
        .unwrap();
    let options = YandexOptions {
        api: origin,
        ..YandexOptions::default()
    };
    let backend = YandexBackend::new(store, options, Arc::new(ManualClock::new(WALL))).unwrap();
    assert!(
        backend
            .update_settings(&HashMap::from([("tracks".to_owned(), "radio".to_owned())]))
            .await
            .is_err()
    );
    backend
        .update_settings(&HashMap::from([(
            "tracks".to_owned(),
            "favorites".to_owned(),
        )]))
        .await
        .unwrap();

    let tracks = backend
        .station_batch("user:onyourwave", &Default::default())
        .await
        .unwrap();
    let mut ids: Vec<&str> = tracks.iter().map(|t| t.track_id.as_str()).collect();
    ids.sort();
    assert_eq!(ids, ["7:1", "8:2"]);
    assert!(
        tracks.iter().all(|t| t.batch_id.is_empty()),
        "no feedback for liked tracks"
    );
    let log = log.lock().unwrap().join("\n");
    assert!(!log.contains("rotor"), "the station is not asked: {log}");
    assert!(log.contains("POST /tracks track-ids="), "{log}");
}

//! The Yandex backend against the Python one (`tools/gen_golden.py`, `tests/golden/yandex/expected.json`). Task 5.3 of
//! docs/rust-port.md.
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
        yandex::{YandexBackend, YandexOptions},
    },
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
    let Ok(text) = fs::read_to_string(&path) else {
        assert!(
            std::env::var_os("GOLDEN_REQUIRED").is_none(),
            "no golden file, run tools/gen_golden.py"
        );
        eprintln!("no golden file: skipped");
        return;
    };
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

        let settings_path = dir.join("yamusic-radio.json");
        if !scenario["settings"].is_null() {
            fs::write(&settings_path, scenario["settings"].to_string()).unwrap();
        }
        let delays = scenario
            .get("rate_limit_delays")
            .and_then(Value::as_array)
            .map_or_else(
                || vec![0.01, 0.02, 0.04, 0.08],
                |d| d.iter().map(|v| v.as_f64().unwrap()).collect(),
            );
        let backend = YandexBackend::new(
            settings_path.clone(),
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
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                fs::metadata(&settings_path)
                    .ok()
                    .map(|m| m.permissions().mode() & 0o777)
            };
            #[cfg(not(unix))]
            let mode = want["state"]["settings_mode"].as_u64().map(|m| m as u32);
            got.insert(
                "state".to_owned(),
                json!({
                    "authorized": backend.authorized(),
                    "prompt": backend.auth_prompt(),
                    "settings_file": fs::read_to_string(&settings_path).ok().map(|t| serde_json::from_str::<Value>(&t).unwrap()),
                    "settings_mode": mode,
                }),
            );
            if let Some(difference) = first_difference(&Value::Object(got), want, "") {
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
    let path = dir.join("endlesswave.json");
    std::fs::write(
        &path,
        r#"{"token": "t", "from_id": "music-x", "quality": "lossless"}"#,
    )
    .unwrap();
    let backend = YandexBackend::new(
        path.clone(),
        YandexOptions::default(),
        Arc::new(ManualClock::new(WALL)),
    )
    .unwrap();

    let (values, fields) = backend.settings().expect("Yandex has settings");
    assert_eq!(values["quality"], "lossless");
    assert_eq!(fields[0].name, "quality");
    let offered: Vec<&str> = fields[0].choices.iter().map(|c| c.value.as_str()).collect();
    assert_eq!(offered, ["lossless", "hq", "nq"]);

    backend
        .update_settings(&HashMap::from([("quality".to_owned(), "hq".to_owned())]))
        .await
        .unwrap();
    assert_eq!(backend.settings().unwrap().0["quality"], "hq");
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
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

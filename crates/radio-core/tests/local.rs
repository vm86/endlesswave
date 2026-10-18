//! The local backend against the Python one (`tools/gen_golden.py`, `tests/golden/local/expected.json`). Task 5.2 of
//! docs/rust-port.md.
//!
//! A folder of real audio files with tags (and with what is not to be taken) is made by the stand; both backends find the
//! same files with the same ids, read the same tags, and answer the same to the same steps. A batch is a random choice,
//! so where the choice is not forced, only its size and the tracks (as read) are compared. Needs ffmpeg, and the folder
//! (`tests/golden/local/music`, not in git): without them the test is skipped, unless `GOLDEN_REQUIRED` is set.

use radio_core::{
    backend::{
        MusicBackend, Track,
        local::{LocalBackend, parse_tags, scan, track_id},
    },
    clock::ManualClock,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/local")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(entry.path()).unwrap(), &target).unwrap();
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Hide where the folder is, as the stand does.
fn plain(text: &str, root: &Path) -> String {
    let resolved = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    text.replace(&resolved.display().to_string(), "<root>")
        .replace(&root.display().to_string(), "<root>")
}

fn id_of(root: &Path, relative: &str) -> String {
    track_id(&root.join(relative).strip_prefix(root).unwrap().to_owned())
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

fn skip(why: &str) {
    assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "{why}");
    eprintln!("{why}: skipped");
}

fn expected() -> Option<(Value, PathBuf)> {
    let music = dir().join("music");
    if !music.join("a.mp3").exists() {
        skip("no folder of music, run tools/gen_golden.py");
        return None;
    }
    if !std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        skip("no ffmpeg");
        return None;
    }
    Some((
        serde_json::from_str(&fs::read_to_string(dir().join("expected.json")).unwrap()).unwrap(),
        music,
    ))
}

#[test]
fn the_files_and_the_tags_are_read_as_python_reads_them() {
    let Some((golden, music)) = expected() else {
        return;
    };
    // the files the scan finds, by path and id
    let found = scan(&music).unwrap();
    let mut files: Vec<(String, String)> = found
        .iter()
        .map(|(id, path)| {
            (
                path.strip_prefix(&music)
                    .unwrap()
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
                id.clone(),
            )
        })
        .collect();
    files.sort();
    let want: Vec<(String, String)> = golden["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f[0].as_str().unwrap().to_owned(),
                f[1].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(files, want);

    for case in golden["parse_tags"].as_array().unwrap() {
        let tags: BTreeMap<String, String> = parse_tags(case["text"].as_str().unwrap())
            .into_iter()
            .collect();
        let wanted: BTreeMap<String, String> = case["tags"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
            .collect();
        assert_eq!(tags, wanted, "tags of {:?}", case["text"]);
    }
}

#[tokio::test]
async fn the_local_backend_answers_as_the_python_one_does() {
    let Some((golden, music)) = expected() else {
        return;
    };
    let tracks = golden["tracks"].as_object().unwrap();
    let scratch = std::env::temp_dir().join(format!("radio-core-local-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);

    for (index, scenario) in golden["scenarios"].as_array().unwrap().iter().enumerate() {
        let name = scenario["name"].as_str().unwrap();
        let base = scratch.join(format!("{index}"));
        let root = base.join("music");
        copy_tree(&music, &root);
        let work = base.join("downloads");
        fs::create_dir_all(&work).unwrap();
        let clock = Arc::new(ManualClock::new(1000.0));
        let chosen: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = chosen.clone();
        let backend = LocalBackend::new(
            None,
            Some(Box::new(move |d| sink.lock().unwrap().push(d.to_owned()))),
            clock.clone(),
        )
        .with_batch_size(scenario["batch_size"].as_u64().unwrap_or(3) as usize)
        .with_seed(5);

        for (n, (step, want)) in scenario["steps"]
            .as_array()
            .unwrap()
            .iter()
            .zip(scenario["results"].as_array().unwrap())
            .enumerate()
        {
            let mut got = json!({});
            let mut error: Option<String> = None;
            match step["op"].as_str().unwrap() {
                "authorize" => {
                    let typed = step["directory"]
                        .as_str()
                        .unwrap()
                        .replace("<root>", &root.display().to_string());
                    match backend
                        .authorize(&HashMap::from([("directory".to_owned(), typed)]))
                        .await
                    {
                        Ok(()) => {
                            got["chosen"] = json!(
                                chosen
                                    .lock()
                                    .unwrap()
                                    .iter()
                                    .map(|c| plain(c, &root))
                                    .collect::<Vec<_>>()
                            )
                        }
                        Err(e) => error = Some(plain(&format!("AuthError: {}", e.0), &root)),
                    }
                }
                "authorize_raw" => {
                    let values: HashMap<String, String> = step["values"]
                        .as_object()
                        .unwrap()
                        .iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                        .collect();
                    error = backend
                        .authorize(&values)
                        .await
                        .err()
                        .map(|e| plain(&format!("AuthError: {}", e.0), &root));
                }
                "batch" => {
                    let mut exclude: HashSet<String> = step["exclude"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|r| id_of(&root, r.as_str().unwrap()))
                        .collect();
                    if step["exclude_all"].as_bool().unwrap_or(false) {
                        exclude = scan(&root).unwrap().into_keys().collect();
                    }
                    match backend
                        .station_batch(step["station"].as_str().unwrap_or("shuffle"), &exclude)
                        .await
                    {
                        Ok(batch) => {
                            // Where the choice is not forced, it is the size and the tracks that are compared.
                            let mut list: Vec<Value> = batch
                                .iter()
                                .map(|t| serde_json::to_value(t).unwrap())
                                .collect();
                            list.sort_by_key(|t| t["track_id"].as_str().unwrap().to_owned());
                            if step["deterministic"].as_bool().unwrap_or(false) {
                                got["tracks"] = json!(list);
                            } else {
                                assert_eq!(
                                    list.len(),
                                    want["tracks"].as_array().unwrap().len(),
                                    "{name}, step {n}: how many"
                                );
                                for track in &list {
                                    let id = track["track_id"].as_str().unwrap();
                                    assert_eq!(
                                        track, &tracks[id],
                                        "{name}, step {n}: the track {id}"
                                    );
                                }
                                got["tracks"] = want["tracks"].clone();
                            }
                        }
                        Err(e) => error = Some(plain(&format!("BackendError: {}", e.0), &root)),
                    }
                }
                "download" => {
                    let track = match step["path"].as_str() {
                        Some(path) => Track {
                            track_id: id_of(&root, path),
                            title: step["title"].as_str().unwrap().to_owned(),
                            artists: vec!["x".into()],
                            codec: "mp3".into(),
                            ..blank()
                        },
                        None => Track {
                            track_id: step["id"].as_str().unwrap().to_owned(),
                            title: "Gone".into(),
                            artists: vec!["x".into()],
                            codec: "mp3".into(),
                            ..blank()
                        },
                    };
                    let target = work.join(step["name"].as_str().unwrap());
                    match backend.download(&track, &target).await {
                        Ok(()) => {
                            let resolved = target.canonicalize().unwrap();
                            got["link"] = json!({
                                "is_symlink": target.is_symlink(),
                                "target": plain(&resolved.display().to_string(), &root),
                                "size": fs::metadata(&target).unwrap().len(),
                            });
                        }
                        Err(e) => error = Some(plain(&format!("BackendError: {}", e.0), &root)),
                    }
                }
                "write" => {
                    let path = root.join(step["path"].as_str().unwrap());
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::copy(root.join("a.mp3"), path).unwrap();
                }
                "remove" => fs::remove_file(root.join(step["path"].as_str().unwrap())).unwrap(),
                "advance" => clock.advance(step["seconds"].as_f64().unwrap()),
                "query" => {}
                other => panic!("{other}"),
            }
            if let Some(error) = error {
                got["error"] = error.into();
            }
            let (settings, fields) = backend.settings().unwrap();
            got["state"] = json!({
                "authorized": backend.authorized(),
                "settings": settings.into_iter().map(|(k, v)| (k, plain(&v, &root))).collect::<BTreeMap<_, _>>(),
                "setting_fields": fields,
                "prompt": backend.auth_prompt(),
                "default_station": backend.default_station(),
                "supports_moods": backend.supports_moods(),
            });
            if let Some(difference) = first_difference(&got, want, "") {
                panic!("{name}, step {n} ({step}): {difference}");
            }
        }
        println!("{name}: the same");
    }
}

fn blank() -> Track {
    Track {
        track_id: String::new(),
        title: String::new(),
        artists: vec![],
        codec: String::new(),
        bitrate_in_kbps: 0,
        genre: String::new(),
        cover: String::new(),
        batch_id: String::new(),
        mood: String::new(),
        source: String::new(),
        native_id: String::new(),
    }
}

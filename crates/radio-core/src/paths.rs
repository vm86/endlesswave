//! Where the radio keeps its files, named after the app, and the move of what earlier versions kept under the names of
//! the Python radio (`yamusic-radio`).

use std::{
    io,
    path::{Path, PathBuf},
};

pub const CONFIG_NAME: &str = "endlesswave.json";
pub const CACHE_NAME: &str = "endlesswave.db";
pub const LOG_NAME: &str = "endlesswave.log";

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// `~/.config/endlesswave.json`: where the token of Yandex Music and its settings were kept before they went into the
/// database; the radio takes the file in and removes it.
pub fn legacy_config_file() -> PathBuf {
    home().join(".config").join(CONFIG_NAME)
}

/// `~/.cache/endlesswave.db`: the analysis of the tracks, the history and the settings of the radio.
pub fn cache_file() -> PathBuf {
    home().join(".cache").join(CACHE_NAME)
}

/// `~/.cache/endlesswave-tracks`: the loaded tracks that wait for their turn; it goes with the cache's name.
pub fn tracks_dir_of(cache: &Path) -> PathBuf {
    let stem = cache
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    cache.with_file_name(format!("{stem}-tracks"))
}

/// `~/.cache/endlesswave.log`
pub fn log_file() -> PathBuf {
    home().join(".cache").join(LOG_NAME)
}

/// Move what the earlier names held to the new ones, and remove what is left of the old. Says what it did.
///
/// - The token file and the database (with the files SQLite keeps beside it) are moved. If the new name is taken already,
///   the new one is kept and the old is removed.
/// - The tracks that waited are removed, not moved: the playlist of the database names their files by path, and they are
///   loaded again.
/// - The log is removed.
pub fn migrate_legacy(home: &Path) -> io::Result<Vec<String>> {
    let (config, cache) = (home.join(".config"), home.join(".cache"));
    let mut done = Vec::new();

    let take = |old: PathBuf, new: PathBuf, done: &mut Vec<String>| -> io::Result<()> {
        if !old.exists() {
            return Ok(());
        }
        if new.exists() {
            std::fs::remove_file(&old)?;
            done.push(format!(
                "removed {} ({} is there)",
                old.display(),
                new.display()
            ));
        } else {
            std::fs::rename(&old, &new)?;
            done.push(format!("moved {} to {}", old.display(), new.display()));
        }
        Ok(())
    };
    take(
        config.join("yamusic-radio.json"),
        config.join(CONFIG_NAME),
        &mut done,
    )?;
    take(
        config.join("yamusic-radio.json.tmp"),
        config.join(format!("{CONFIG_NAME}.tmp")),
        &mut done,
    )
    .ok();

    // The database goes with its sidecars, or not at all: a write-ahead log without its database is lost, and one beside
    // another database is wrong.
    let (old_db, new_db) = (cache.join("yamusic-radio.db"), cache.join(CACHE_NAME));
    if old_db.exists() {
        let keep_new = new_db.exists();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let (old, new) = (
                PathBuf::from(format!("{}{suffix}", old_db.display())),
                PathBuf::from(format!("{}{suffix}", new_db.display())),
            );
            if keep_new {
                if old.exists() {
                    std::fs::remove_file(&old)?;
                }
            } else if old.exists() {
                std::fs::rename(&old, &new)?;
            }
        }
        done.push(if keep_new {
            format!(
                "removed {} ({} is there)",
                old_db.display(),
                new_db.display()
            )
        } else {
            format!("moved {} to {}", old_db.display(), new_db.display())
        });
    }

    let tracks = cache.join("yamusic-radio-tracks");
    if tracks.is_dir() {
        std::fs::remove_dir_all(&tracks)?;
        done.push(format!("removed {}", tracks.display()));
    }
    let log = cache.join("yamusic_radio.log");
    if log.exists() {
        std::fs::remove_file(&log)?;
        done.push(format!("removed {}", log.display()));
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("paths-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".config")).unwrap();
        fs::create_dir_all(dir.join(".cache")).unwrap();
        dir
    }

    #[test]
    fn the_old_files_are_moved_and_the_rest_is_removed() {
        let home = scratch("move");
        fs::write(home.join(".config/yamusic-radio.json"), b"token").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                home.join(".config/yamusic-radio.json"),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
        for (name, text) in [
            ("yamusic-radio.db", "db"),
            ("yamusic-radio.db-wal", "wal"),
            ("yamusic-radio.db-shm", "shm"),
            ("yamusic_radio.log", "log"),
        ] {
            fs::write(home.join(".cache").join(name), text).unwrap();
        }
        fs::create_dir_all(home.join(".cache/yamusic-radio-tracks")).unwrap();
        fs::write(home.join(".cache/yamusic-radio-tracks/a.mp3"), b"x").unwrap();

        let done = migrate_legacy(&home).unwrap();
        assert!(done.len() >= 4, "{done:?}");
        assert_eq!(
            fs::read(home.join(".config/endlesswave.json")).unwrap(),
            b"token"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(home.join(".config/endlesswave.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(fs::read(home.join(".cache/endlesswave.db")).unwrap(), b"db");
        assert_eq!(
            fs::read(home.join(".cache/endlesswave.db-wal")).unwrap(),
            b"wal"
        );
        assert_eq!(
            fs::read(home.join(".cache/endlesswave.db-shm")).unwrap(),
            b"shm"
        );
        for gone in [
            ".config/yamusic-radio.json",
            ".cache/yamusic-radio.db",
            ".cache/yamusic-radio.db-wal",
            ".cache/yamusic-radio-tracks",
            ".cache/yamusic_radio.log",
        ] {
            assert!(!home.join(gone).exists(), "{gone} is still there");
        }
        // a second start has nothing to do
        assert!(migrate_legacy(&home).unwrap().is_empty());
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn what_is_new_stays_and_the_old_goes() {
        let home = scratch("both");
        fs::write(home.join(".config/yamusic-radio.json"), b"old").unwrap();
        fs::write(home.join(".config/endlesswave.json"), b"new").unwrap();
        fs::write(home.join(".cache/yamusic-radio.db"), b"old").unwrap();
        fs::write(home.join(".cache/yamusic-radio.db-wal"), b"old").unwrap();
        fs::write(home.join(".cache/endlesswave.db"), b"new").unwrap();
        migrate_legacy(&home).unwrap();
        assert_eq!(
            fs::read(home.join(".config/endlesswave.json")).unwrap(),
            b"new"
        );
        assert_eq!(
            fs::read(home.join(".cache/endlesswave.db")).unwrap(),
            b"new"
        );
        assert!(!home.join(".config/yamusic-radio.json").exists());
        assert!(
            !home.join(".cache/yamusic-radio.db").exists()
                && !home.join(".cache/yamusic-radio.db-wal").exists()
        );
        assert!(
            !home.join(".cache/endlesswave.db-wal").exists(),
            "the old log is not taken for the new database's"
        );
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn nothing_old_nothing_done() {
        let home = scratch("none");
        assert!(migrate_legacy(&home).unwrap().is_empty());
        assert_eq!(
            tracks_dir_of(Path::new("/c/endlesswave.db")),
            PathBuf::from("/c/endlesswave-tracks")
        );
        let _ = fs::remove_dir_all(&home);
    }
}

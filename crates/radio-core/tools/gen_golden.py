#!/usr/bin/env python3
"""Make the golden files for the Rust port from the Python radio.

For every case it writes
  tests/golden/NAME.json  what yamusic_radio.dj_mix.analyze() answers (committed),
  tests/golden/NAME.f32   the mono signal it was given, float32 little-endian (not committed: it is
                          big, and it is made again from the parameters below).
The signal is stereo with two equal channels, so the file holds one.

  YAMUSIC_RADIO=~/devel/yamusic-radio ~/devel/yamusic-radio/.venv/bin/python tools/gen_golden.py
"""

import json
import os
import sys
from pathlib import Path

import numpy as np

radio = Path(os.environ.get("YAMUSIC_RADIO", "~/devel/yamusic-radio")).expanduser()
sys.path.insert(0, str(radio))
sys.path.insert(0, str(Path(__file__).resolve().parent))
from yamusic_radio import dj_mix  # noqa: E402

SR = dj_mix.SAMPLE_RATE
OUT = Path(__file__).resolve().parent.parent / "tests" / "golden"


def click_track(bpm: float, seconds: float, first_beat: float = 0.3) -> np.ndarray:
    """A kick on every beat over faint noise (the one of tests/test_dj.py of the radio)."""
    size = int(SR * seconds)
    audio = np.zeros(size, dtype=np.float32)
    kick_t = np.arange(3000) / SR
    kick = (np.sin(2 * np.pi * 60 * kick_t) * np.exp(-kick_t * 30)) * 0.8
    period = SR * 60 / bpm
    beat = 0
    while (start := int(first_beat * SR + beat * period)) < size - 3000:
        audio[start : start + 3000] += kick.astype(np.float32)
        beat += 1
    audio += np.random.default_rng(0).normal(0, 0.01, size).astype(np.float32)
    return audio


def chord_track(tonic: int, minor: bool, seconds: float) -> np.ndarray:
    """A triad with its bass, held and then changed to the fifth, on faint noise: it has a key."""
    t = np.arange(int(SR * seconds)) / SR
    third = 3 if minor else 4
    half = len(t) * 4 // 5
    # the first four fifths on the tonic chord, the rest on the fifth's: a cadence, not a drone
    first = np.zeros_like(t)
    second = np.zeros_like(t)
    for octave, level in ((1, 0.35), (2, 0.2), (4, 0.1)):
        for degree in (0, third, 7):
            hz = 440 * 2 ** ((tonic - 9 + degree) / 12) * octave / 4
            first += level * np.sin(2 * np.pi * hz * t)
        for degree in (7, 7 + third, 14):
            hz = 440 * 2 ** ((tonic - 9 + degree) / 12) * octave / 4
            second += level * np.sin(2 * np.pi * hz * t)
    signal = np.where(np.arange(len(t)) < half, first, second)
    signal += np.random.default_rng(tonic + 24 * minor).normal(0, 0.01, len(t))
    return (signal / max(1.0, float(np.abs(signal).max()) / 0.8)).astype(np.float32)


def padded(mono: np.ndarray, before: float, after: float) -> np.ndarray:
    return np.concatenate([np.zeros(int(SR * before), np.float32), mono, np.zeros(int(SR * after), np.float32)])


CASES: dict[str, np.ndarray] = {
    "click_124": click_track(124, 60),
    "click_90_late": click_track(90, 45, first_beat=0.1),
    "click_120_silence": padded(click_track(120, 40), 2, 2),
    "noise": np.random.default_rng(1).normal(0, 0.1, SR * 30).astype(np.float32),
    "silence": np.zeros(SR * 5, np.float32),
    # shorter than two analysis frames: 0.3 s makes analyze() of the radio fail (flux and trend differ in size)
    "short": click_track(128, 0.04),
    "chord_c_major": chord_track(0, False, 30),
    "chord_a_minor": chord_track(9, True, 30),
    "chord_f_sharp_minor": chord_track(6, True, 30),
    "chord_d_flat_major": chord_track(1, False, 30),
    # every key (0-11 major, 12-23 minor): the spelling of the tonics, flats too
    **{f"chord_key_{key:02}": chord_track(key % 12, key >= 12, 12) for key in range(24)},
}


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    for name, mono in CASES.items():
        pcm = np.stack([mono, mono], axis=1)
        result = dj_mix.analyze(pcm)
        (OUT / f"{name}.f32").write_bytes(mono.astype("<f4").tobytes())
        data = {
            "sample_rate": SR,
            "frames": int(len(mono)),
            "profile": profile_json(pcm, result),
            "key_detector": dj_mix.key_detector(),
            "loudness_meter": dj_mix.loudness_meter(),
            "analysis": {
                "bpm": result.bpm,
                "head_beat": result.head_beat,
                "tail_beat": result.tail_beat,
                "start": result.start,
                "end": result.end,
                "loudness_db": result.loudness_db,
                "intro_end": result.intro_end,
                "outro_start": result.outro_start,
                "key": result.key,
                "camelot": result.camelot,
            },
        }
        (OUT / f"{name}.json").write_text(json.dumps(data, indent=2) + "\n")
        print(f"{name}: bpm {result.bpm:.2f}, key {result.key} ({result.camelot}), {result.loudness_db:.2f} LUFS")


def profile_json(pcm: np.ndarray, result: dj_mix.TrackAnalysis) -> dict[str, object]:
    """What the key by the profiles gives, for the tracks Essentia has no answer for."""
    mono = np.ascontiguousarray(pcm[result.start : result.end].mean(axis=1), dtype=np.float32)
    key, confidence = dj_mix.profile_key(mono)
    return {"key": key, "confidence": confidence, "chroma": [float(c) for c in dj_mix.chroma(mono)]}


def analysis_json(a: dj_mix.TrackAnalysis) -> dict[str, object]:
    return {
        "bpm": a.bpm,
        "head_beat": a.head_beat,
        "tail_beat": a.tail_beat,
        "start": a.start,
        "end": a.end,
        "loudness_db": a.loudness_db,
        "intro_end": a.intro_end,
        "outro_start": a.outro_start,
        "key": a.key,
    }


def random_analysis(rng: np.random.Generator) -> dj_mix.TrackAnalysis:
    """A track as the analysis could find it: the tempo, the key or the sections may be unknown."""
    bpm = 0.0 if rng.random() < 0.1 else float(rng.choice([rng.uniform(70, 180), rng.uniform(118, 130)]))
    start = int(rng.integers(0, SR))
    end = start + int(rng.uniform(150, 400) * SR)
    sections = rng.random() > 0.2
    return dj_mix.TrackAnalysis(
        bpm=bpm,
        head_beat=float(start + rng.integers(0, SR)),
        tail_beat=float(end - rng.integers(0, SR)),
        start=start,
        end=end,
        loudness_db=float(rng.uniform(-22, -6)),
        intro_end=start + int(rng.uniform(1, 40) * SR) if sections else -1,
        outro_start=end - int(rng.uniform(1, 40) * SR) if sections else -1,
        key=-1 if rng.random() < 0.15 else int(rng.integers(0, 24)),
    )


def logic() -> dict[str, object]:
    """The choice of the next track: keys, moods, costs, and the picks of choose_next on random pools."""
    rng = np.random.default_rng(2026)
    keys = list(range(-2, 26))
    relations = [
        {
            "from": a,
            "to": b,
            "relation": dj_mix.key_relation(a, b),
            "quality": dj_mix.key_quality(dj_mix.key_relation(a, b)),
            "cost": dj_mix.key_cost(a, b),
        }
        for a in keys
        for b in keys
    ]
    tracks = [random_analysis(rng) for _ in range(40)]
    samples = [
        {
            "analysis": analysis_json(t),
            "energy": dj_mix.energy(t),
            "mood": dj_mix.mood_of(t),
        }
        for t in tracks
    ]
    pairs = []
    for _ in range(150):
        a, b = rng.choice(len(tracks), 2)
        current, following = tracks[a], tracks[b]
        speed = dj_mix.transition_speed(current.bpm, following)
        pairs.append(
            {
                "current": analysis_json(current),
                "following": analysis_json(following),
                "speed": speed,
                "section": dj_mix.section_length(current, following),
                "cost": dj_mix.transition_cost(current, following)[0],
                "tempo_ratio": dj_mix.tempo_ratio(current.bpm, following.bpm) if current.bpm and following.bpm else None,
                "kind": {
                    str(matched): dj_mix.choose_transition(current, following, beat_matched=matched).value
                    for matched in (True, False)
                },
            }
        )
    plans = []
    for _ in range(300):
        a, b = rng.choice(len(tracks), 2)
        current, following = tracks[a], tracks[b]
        speed = dj_mix.transition_speed(current.bpm, following) or 1.0
        both = bool(current.bpm and following.bpm)
        plan = {
            "current": analysis_json(current),
            "following": analysis_json(following),
            "speed": speed,
            "matched": dj_mix.plan_matched(current, following, speed) if both else None,
            "blend": {
                str(matched): dj_mix.plan_blend(current, following, beat_matched=matched, speed=speed)
                for matched in (True, False)
            },
            "early": [],
        }
        for _ in range(3):
            position = int(rng.integers(current.start, current.end))
            for matched in (True, False):
                plan["early"].append(
                    {
                        "position": position,
                        "beat_matched": matched,
                        "plan": dj_mix.plan_early(current, following, position, beat_matched=matched, speed=speed),
                    }
                )
        plans.append(plan)
    artists = [f"artist {i}" for i in range(8)]
    scenarios = []
    for number in range(80):
        size = int(rng.integers(1, 13))
        pool = [
            dj_mix.Entry(
                analysis=random_analysis(rng),
                artists=frozenset(rng.choice(artists, int(rng.integers(0, 3)), replace=False).tolist()),
                mood=str(rng.choice(["", "sad", "calm", "fun", "active"])),
            )
            for _ in range(size)
        ]
        current = None if number % 7 == 0 else random_analysis(rng)
        history = tuple(
            frozenset(rng.choice(artists, int(rng.integers(0, 3)), replace=False).tolist())
            for _ in range(int(rng.integers(0, 7)))
        )
        position = int(rng.integers(0, 25))
        depth = min(len(dj_mix.LOOKAHEAD_WEIGHTS), len(pool))
        cost, _ = dj_mix._best_path(current, pool, history, position, depth)
        scenarios.append(
            {
                "current": None if current is None else analysis_json(current),
                "pool": [
                    {"analysis": analysis_json(e.analysis), "artists": sorted(e.artists), "mood": e.mood} for e in pool
                ],
                "history": [sorted(h) for h in history],
                "position": position,
                "index": dj_mix.choose_next(current, pool, history, position),
                "cost": cost,
                "steps": [dj_mix.step_cost(current, e, history, position)[0] for e in pool],
            }
        )
    return {
        "keys": [{"key": k, "camelot": dj_mix.camelot_code(k), "name": dj_mix.key_name(k)} for k in keys],
        "relations": relations,
        "tracks": samples,
        "arc": [
            {"position": n, "target": dj_mix.arc_target(n), "mood": dj_mix.mood_for(n)} for n in range(-3, 40)
        ],
        "pairs": pairs,
        "plans": plans,
        "scenarios": scenarios,
    }


def music(seed: int, low_hz: float, high_hz: float, seconds: float) -> np.ndarray:
    """A stereo piece with a bass, a lead, kicks and noise, the two channels a little different."""
    rng = np.random.default_rng(seed)
    t = np.arange(int(SR * seconds)) / SR
    bass = 0.35 * np.sin(2 * np.pi * low_hz * t)
    lead = 0.15 * np.sin(2 * np.pi * high_hz * t) * (0.6 + 0.4 * np.sin(2 * np.pi * 0.7 * t))
    kicks = np.zeros_like(t)
    kick_t = np.arange(4000) / SR
    kick = np.sin(2 * np.pi * 55 * kick_t) * np.exp(-kick_t * 25) * 0.5
    for start in range(int(0.2 * SR), len(t) - 4000, SR // 2):
        kicks[start : start + 4000] += kick
    left = bass + lead + kicks + rng.normal(0, 0.03, len(t))
    right = bass * 0.9 + lead * 1.1 + kicks + rng.normal(0, 0.03, len(t))
    return np.stack([left, right], axis=1).astype(np.float32)


class Recorder:
    """A numpy generator that remembers every normal() draw, so that the Rust side can be given the same noise."""

    def __init__(self, seed: int) -> None:
        self._rng = np.random.default_rng(seed)
        self.draws: list[np.ndarray] = []

    def normal(self, loc: float, scale: float, size: object) -> np.ndarray:
        drawn = self._rng.normal(loc, scale, size)
        self.draws.append(np.asarray(drawn, dtype=np.float64).ravel())
        return drawn


def cache_scenario(cache: object, rng: np.random.Generator, tick: object) -> dict[str, object]:
    """Fill a Python TrackCache with tracks, plays, blends and settings; say what its readers answer."""
    tracks = []
    genres = ["house", "techno", "", "rock", "house"]
    for i in range(30):
        tracks.append((f"ya:{i}", ("mp3", "flac", "aac")[i % 3], f"Title {i}", random_analysis(rng), genres[i % 5]))
    for track_id, codec, title, analysis, genre in tracks:
        cache.put_analysis(track_id, codec, title, analysis, genre)
    # the same track in another codec, and a track put again with another genre
    cache.put_analysis("ya:0", "flac", "Title 0 flac", random_analysis(rng), "ambient")
    cache.put_analysis("ya:1", "flac", "Title 1", tracks[1][3], "dub")
    for i in (5, 3, 9, 3, 12, 20, 7):
        cache.mark_played(tracks[i][0], tracks[i][1])
    cache.set_genre("ya:4", tracks[4][1], "electro")
    cache.mark_played("ya:0", "flac")
    kinds = ["bass swap", "eq blend", "filter sweep", "crossfade"]
    effects = ["none", "drop", "echo"]
    camelot = ["8A", "9A", "8B", "3B", "", "12A"]
    for n in range(40):
        a, b = camelot[n % 6], camelot[(n * 5 + 1) % 6]
        relation = ["same", "neighbour", "boost", "clash", "unknown", "diagonal"][n % 6]
        cache.add_transition(
            kinds[n % 4],
            beat_matched=bool(n % 3),
            key_relation=relation,
            from_track=f"ya:{n % 30}",
            to_track=f"ya:{(n + 1) % 30}",
            from_key=a,
            to_key=b,
            effect=effects[n % 3],
        )
    # rows made before the keys and the effects were kept have NULLs there
    for n in range(5):
        cache._db.execute(
            "INSERT INTO transitions (at, kind, beat_matched, key_relation, from_track, to_track) "
            "VALUES (?, 'crossfade', 1, 'same', 'old', 'older')",
            (tick(),),
        )
    cache._db.commit()
    settings = {
        "moods": ["calm", "fun"],
        "language": "russian",
        "playlist": [{"track_id": "ya:1", "codec": "mp3", "pinned": True}],
        "number": 7,
        "nothing": None,
        "text": "тест ✓",
    }
    for key, value in settings.items():
        cache.put_setting(key, value)
    cache.put_setting("gone", 1)
    cache.delete_setting("gone")
    cache._db.execute("INSERT INTO settings (key, value) VALUES ('broken', '{not json')")
    cache._db.commit()

    ids = [(t[0], t[1]) for t in tracks] + [("ya:0", "mp3"), ("missing", "mp3")]
    return {
        "key_detector": dj_mix.key_detector(),
        "loudness_meter": dj_mix.loudness_meter(),
        "analyses": [
            {
                "track_id": track_id,
                "codec": codec,
                "analysis": None if (a := cache.get_analysis(track_id, codec)) is None else analysis_json(a),
                "genre": cache.get_genre(track_id, codec),
            }
            for track_id, codec in ids
        ],
        "settings": {key: cache.get_setting(key) for key in [*settings, "gone", "broken", "never"]},
        "stats": cache.transition_stats(),
        "key_pairs": [list(p) for p in cache.key_pairs()],
        "known_genres": [list(g) for g in cache.known_genres()],
        "recently_played": {str(n): sorted(cache.recently_played(n)) for n in (1, 3, 5, 100)},
    }


def write_cache() -> None:
    """Caches of the Python radio, and what it reads from them, for the Rust cache to read the same."""
    from yamusic_radio import cache as cache_module

    folder = OUT / "cache"
    folder.mkdir(parents=True, exist_ok=True)
    clock = iter(np.arange(1_000_000.0, 2_000_000.0, 1.0))
    cache_module.time.time = lambda: float(next(clock))  # a clock that ticks once a call: a play has its order
    expected: dict[str, object] = {}

    for name in ("python.db", "prune.db", "old.db"):
        (folder / name).unlink(missing_ok=True)

    cache = cache_module.TrackCache(folder / "python.db")
    expected["main"] = cache_scenario(cache, np.random.default_rng(7), lambda: float(next(clock)))
    cache.close()

    cache_module.MAX_ROWS = 5
    small = cache_module.TrackCache(folder / "prune.db")
    rng = np.random.default_rng(8)
    for i in range(9):
        small.put_analysis(f"p:{i}", "mp3", f"P {i}", random_analysis(rng))
        if i == 2:
            small.mark_played("p:0", "mp3")  # used again: it stays longer
    expected["prune"] = sorted(r[0] for r in small._db.execute("SELECT track_id FROM tracks"))
    small.close()
    cache_module.MAX_ROWS = 20000

    # a cache of the first version: no genre, no keys and effects of the transitions
    import sqlite3

    old = sqlite3.connect(folder / "old.db")
    old.executescript(
        "CREATE TABLE tracks (track_id TEXT NOT NULL, codec TEXT NOT NULL, title TEXT NOT NULL, analysis TEXT NOT NULL,"
        " played_at REAL, used_at REAL NOT NULL, PRIMARY KEY (track_id, codec));"
        "CREATE TABLE transitions (at REAL NOT NULL, kind TEXT NOT NULL, beat_matched INTEGER NOT NULL,"
        " key_relation TEXT NOT NULL, from_track TEXT NOT NULL, to_track TEXT NOT NULL);"
        "INSERT INTO transitions VALUES (1.0, 'bass swap', 1, 'same', 'a', 'b');"
    )
    old.commit()
    old.close()
    migrated = cache_module.TrackCache(folder / "old.db")
    expected["old"] = {
        "tracks": [r[1] for r in migrated._db.execute("PRAGMA table_info(tracks)")],
        "transitions": [r[1] for r in migrated._db.execute("PRAGMA table_info(transitions)")],
        "stats": migrated.transition_stats(),
        "key_pairs": migrated.key_pairs(),
    }
    migrated.close()
    # the migration above changed old.db: start the Rust side from the old one again
    (folder / "old.db").unlink()
    old = sqlite3.connect(folder / "old.db")
    old.executescript(
        "CREATE TABLE tracks (track_id TEXT NOT NULL, codec TEXT NOT NULL, title TEXT NOT NULL, analysis TEXT NOT NULL,"
        " played_at REAL, used_at REAL NOT NULL, PRIMARY KEY (track_id, codec));"
        "CREATE TABLE transitions (at REAL NOT NULL, kind TEXT NOT NULL, beat_matched INTEGER NOT NULL,"
        " key_relation TEXT NOT NULL, from_track TEXT NOT NULL, to_track TEXT NOT NULL);"
        "INSERT INTO transitions VALUES (1.0, 'bass swap', 1, 'same', 'a', 'b');"
    )
    old.commit()
    old.close()
    (folder / "expected.json").write_text(json.dumps(expected, indent=1, sort_keys=True) + "\n")
    print("cache: python.db, prune.db, old.db")


def broadcast_golden(streams: dict[str, bytes], rng: np.random.Generator) -> dict[str, object]:
    """What the radio's Broadcaster does when it is fed these streams in chunks, and the ICY titles."""
    import asyncio
    import hashlib

    from yamusic_radio import dj

    def hub_for(name: str) -> dj.Broadcaster:
        if name == "mp3":
            return dj.Broadcaster()
        if name == "flac":
            return dj.Broadcaster(
                bytes_per_second=dj.FLAC_BYTES_PER_SECOND,
                frame_start=dj.flac_start,
                header_length=dj.flac_header_length,
                lag_seconds=dj.FLAC_LAG_SECONDS,
            )
        return dj.Broadcaster(
            bytes_per_second=dj.FLAC_BYTES_PER_SECOND,
            frame_start=dj.ogg_start,
            header_length=dj.ogg_header_length,
            lag_seconds=dj.OGG_LAG_SECONDS,
        )

    def drain(queue: asyncio.Queue) -> list[dict[str, object]]:
        items = []
        while not queue.empty():
            data = queue.get_nowait()
            items.append({"length": len(data), "sha1": hashlib.sha1(data).hexdigest()})
        return items

    async def scenario(name: str) -> dict[str, object]:
        data = streams[name]
        hub = hub_for(name)
        header_length = {"flac": dj.flac_header_length, "ogg": dj.ogg_header_length}.get(name)
        position = 0
        if header_length is not None:
            length = header_length(data)
            hub.header = data[:length]
            position = length
        first_join = hub.subscribe_at()  # nothing published yet
        first = {"items": drain(first_join[0]), "position": first_join[1]}
        hub.unsubscribe(first_join[0])
        sizes, fed_values, joins = [], [], []
        checkpoints = {0, 1, 2, 5, 10, 30, 60, 120}
        index = 0
        fed = 0.0
        while position < len(data):
            size = int(rng.integers(1000, 6000))
            chunk = data[position : position + size]
            position += len(chunk)
            fed += float(rng.choice([0.05, 0.1, 0.25]))
            hub.fed = fed
            hub.publish(chunk)
            sizes.append(len(chunk))
            fed_values.append(fed)
            if index in checkpoints or position >= len(data):
                queue, where = hub.subscribe_at()
                joins.append(
                    {
                        "after": index,
                        "items": drain(queue),
                        "position": where,
                        "buffered": hub.buffered_bytes,
                        "positions": [hub.position_of(o) for o in (0, position // 2, position - 1, position, position + 10)],
                    }
                )
                hub.unsubscribe(queue)
            index += 1
        hub.reset_position()
        return {"sizes": sizes, "fed": fed_values, "first": first, "joins": joins, "after_reset": hub.position_of(0)}

    async def overflow() -> dict[str, object]:
        hub = dj.Broadcaster()
        hub.header = b"HEADER"
        queue = hub.subscribe()
        kept = [chunk for chunk in (queue.get_nowait(),)]
        for i in range(300):
            hub.publish(bytes([i % 256]) * (i % 5 + 1))
        items = [queue.get_nowait() for _ in range(queue.qsize())]
        return {"header_first": kept[0].decode(), "length": len(items), "first": items[0].hex(), "last": items[-1].hex()}

    def icy() -> dict[str, object]:
        titles = ["", "Artist - Song", "It's here", "Привет ✓ мир", "x" * 5000, "a" * 239, "a" * 240, "a" * 241]
        blocks = [dj.icy_block(t).hex() for t in titles]
        audio = [bytes((i * 7 + k) % 251 for k in range(size)) for i, size in enumerate(rng.integers(500, 9000, 40))]
        change_at = {0: "First - One", 5: "Second - Two", 6: "Second - Two", 12: "It's - Third", 20: "", 33: "Привет"}
        pending = bytearray()
        sent = None
        title = ""
        out = bytearray()
        for i, chunk in enumerate(audio):
            title = change_at.get(i, title)
            pending += chunk
            while len(pending) >= dj.ICY_METAINT:
                block = b"\0"
                if title != sent:
                    sent = title
                    block = dj.icy_block(sent)
                out += bytes(pending[: dj.ICY_METAINT]) + block
                del pending[: dj.ICY_METAINT]
        return {
            "titles": titles,
            "blocks": blocks,
            "audio_sizes": [len(a) for a in audio],
            "change_at": {str(k): v for k, v in change_at.items()},
            "output_length": len(out),
            "output_sha1": hashlib.sha1(bytes(out)).hexdigest(),
            "first_block_at": dj.ICY_METAINT,
        }

    return {
        "streams": {name: asyncio.run(scenario(name)) for name in streams},
        "overflow": asyncio.run(overflow()),
        "icy": icy(),
    }


def write_streams() -> None:
    """Encoded streams (by the radio's own ffmpeg commands) and what the radio's frame readers say about cuts of them."""
    import subprocess

    from yamusic_radio import dj

    folder = OUT / "stream"
    folder.mkdir(parents=True, exist_ok=True)
    pcm = music(41, 55, 880, 4.0)
    (folder / "pcm.f32").write_bytes(np.ascontiguousarray(pcm, "<f4").tobytes())
    rng = np.random.default_rng(42)

    def encode(output_args: tuple[str, ...]) -> bytes:
        command = ["ffmpeg", "-nostdin", "-v", "error", "-f", "f32le", "-ac", "2", "-ar", str(SR), "-i", "pipe:0"]
        command += [*output_args, "-flush_packets", "1", "pipe:1"]
        return subprocess.run(command, input=pcm.astype("<f4").tobytes(), capture_output=True, check=True).stdout

    def safe(function: object, *args: object) -> object:
        try:
            return function(*args)
        except ValueError:
            return "error"

    streams = {"mp3": encode(dj.MP3_ARGS), "flac": encode(dj.FLAC_ARGS), "ogg": encode(dj.OGG_ARGS)}
    report: dict[str, object] = {}
    for name, data in streams.items():
        (folder / f"{name}.bin").write_bytes(data)
        offsets = sorted({0, 1, 2, 3, 7, 100, 1000, len(data) - 1, len(data), *map(int, rng.integers(0, len(data), 40))})
        cuts = [(o, min(len(data), o + int(n))) for o in offsets for n in (rng.integers(1, 3000), 10**9)]
        entry: dict[str, object] = {"length": len(data), "cuts": []}
        for begin, end in cuts:
            cut = data[begin:end]
            result = {"begin": begin, "end": end}
            if name == "mp3":
                result["start"] = dj.mp3_start(cut)
                result["frame"] = dj.mp3_frame_length(cut[:4])
            elif name == "flac":
                result["start"] = dj.flac_start(cut)
            else:
                result["start"] = dj.ogg_start(cut)
                result["page"] = [dj.ogg_page_length(cut, p) for p in (0, 1, 5, len(cut) - 1, len(cut), len(cut) + 3)]
            entry["cuts"].append(result)
        if name in ("flac", "ogg"):
            function = dj.flac_header_length if name == "flac" else dj.ogg_header_length
            full = function(data)
            sizes = sorted({0, 1, 3, 4, 5, 8, 30, 38, 40, 45, 100, 500, full - 1, full, full + 1, len(data)})
            entry["header"] = full
            entry["headers"] = [{"size": n, "result": safe(function, data[:n])} for n in sizes]
        report[name] = entry
    # bytes that are not any of these streams, with the patterns of each in them
    randoms = []
    for i in range(40):
        size = int(rng.integers(0, 300))
        raw = bytearray(rng.integers(0, 256, size, dtype=np.uint8).tobytes())
        for _ in range(int(rng.integers(0, 6))):
            if len(raw) > 6:
                at = int(rng.integers(0, len(raw) - 5))
                raw[at : at + 4] = [b"\xff\xfb\x90\x00", b"\xff\xf8\x69\x08", b"OggS\x00\x00", b"fLaC\x00\x00"][int(rng.integers(0, 4))]
        data = bytes(raw)
        randoms.append(
            {
                "hex": data.hex(),
                "mp3_start": dj.mp3_start(data),
                "flac_start": dj.flac_start(data),
                "ogg_start": dj.ogg_start(data),
                "flac_header": safe(dj.flac_header_length, data),
                "ogg_header": safe(dj.ogg_header_length, data),
                "ogg_page": [dj.ogg_page_length(data, p) for p in (0, 1, len(data), len(data) + 2)],
            }
        )
    (folder / "frames.json").write_text(json.dumps({"streams": report, "random": randoms}, indent=1) + "\n")
    (folder / "broadcast.json").write_text(json.dumps(broadcast_golden(streams, rng), indent=1) + "\n")
    print("stream: mp3", len(streams["mp3"]), "flac", len(streams["flac"]), "ogg", len(streams["ogg"]))


def write_spectrum() -> None:
    """The bands the radio sends to the page, frame by frame, and how the feed and the start positions behave."""
    import asyncio

    from yamusic_radio import spectrum as sp

    folder = OUT / "spectrum"
    folder.mkdir(parents=True, exist_ok=True)
    mix = np.concatenate([music(51, 55, 880, 3.0), music(52, 80, 3000, 3.0)])
    (folder / "mix.f32").write_bytes(np.ascontiguousarray(mix, "<f4").tobytes())
    sizes = [1000, 1500, 11025, 11025, 700, 3000, 11025, 22050, 5, 11025]
    sizes += [11025] * ((len(mix) - sum(sizes)) // 11025)
    sizes.append(len(mix) - sum(sizes))
    analyzer = sp.SpectrumAnalyzer()
    feeds, position = [], 0
    for size in sizes:
        frames = analyzer.feed(mix[position : position + size])
        position += size
        feeds.append(
            {
                "size": size,
                "position": analyzer.position,
                "frames": [{"t": f.t, "b": list(f.bands), "json": f.to_json()} for f in frames],
            }
        )
    numbers = [0.0, 1.0, 0.0004, 0.0005, 0.0015, 0.0025, 0.12349999, 0.1235, 12.0005, 1234.5678, 0.9995, 0.9999, 1 / 3, 2 / 3, 1e-9, 123456.7895]
    formats = [
        {"t": n, "b": [n, 1 - n if n < 1 else 0.5, 0.0, 1.0, 0.7071, 0.5, 0.25, 0.125, 0.0625, 0.03125, 0.015625, 0.0078125]}
        for n in numbers
    ]
    for entry in formats:
        entry["json"] = sp.Frame(entry["t"], tuple(entry["b"])).to_json()

    feed = sp.SpectrumFeed()
    batches = []
    clock = 0.0
    for size in [3, 1, 7, 50, 120, 200, 40, 1, 1]:
        batch = [sp.Frame(clock + i * 0.033, (float(i),)) for i in range(size)]
        clock += size * 0.033
        feed.publish(batch)
        batches.append(size)
    subscribed = []
    for since in (None, -1.0, 0.0, 5.0, clock - 1.0, clock + 5.0):
        _, backlog = feed.subscribe(since)
        subscribed.append({"since": since, "ts": [f.t for f in backlog]})
    queue, _ = feed.subscribe(None)
    for n in range(100):
        feed.publish([sp.Frame(float(n), (0.0,))])
    kept = []
    while not queue.empty():
        kept.append(queue.get_nowait()[0].t)
    feed.reset()
    _, after_reset = feed.subscribe(0.0)

    async def starts() -> dict[str, object]:
        positions = sp.StartPositions()
        for i in range(300):
            positions.set(f"n{i}", float(i))
        positions.set("n100", 1000.5)  # again: it is among the latest now
        for i in range(300, 330):
            positions.set(f"n{i}", float(i))
        return {f"n{i}": await positions.get(f"n{i}", 0) for i in range(340)}

    expected = {
        "sizes": sizes,
        "feeds": feeds,
        "formats": formats,
        "feed": {"batches": batches, "subscribed": subscribed, "kept": kept, "after_reset": len(after_reset)},
        "starts": asyncio.run(starts()),
    }
    (folder / "expected.json").write_text(json.dumps(expected) + "\n")
    print("spectrum:", sum(len(f["frames"]) for f in feeds), "frames")


def write_radio() -> None:
    """The stand of the radio: the library of fake tracks, the fake backend's answers, a first scenario."""
    import asyncio
    import tempfile

    import radio_harness as harness

    folder = OUT / "radio"
    folder.mkdir(parents=True, exist_ok=True)
    library = harness.build_library(folder / "library")
    (folder / "library.json").write_text(json.dumps(library, indent=1) + "\n")

    # The fake backend on a fixed series of calls: what Rust's must answer the same
    from yamusic_radio.backend import Feedback

    async def backend_calls() -> dict[str, object]:
        backend = harness.FakeBackend(folder / "library", library, fail_downloads=frozenset({"t5"}), supports_moods=True)
        results: list[dict[str, object]] = []
        work = Path(tempfile.mkdtemp())
        tracks = {}

        async def batch(exclude: list[str]) -> None:
            got = await backend.station_batch("fake", exclude_track_ids=set(exclude))
            tracks.update({t.track_id: t for t in got})
            results.append({"op": "batch", "exclude": exclude, "ids": [t.track_id for t in got]})

        await batch([])
        await batch(["t3"])
        await batch(["t6", "t7", "t0"])
        await batch([f"t{i}" for i in range(8)])  # everything is excluded: the station repeats
        for track_id in ("t0", "t5", "t7", "nothing"):
            ok = True
            try:
                track = tracks.get(track_id) or dataclasses_track(library, track_id)
                await backend.download(track, work / f"{track_id}.wav")
            except RuntimeError:
                ok = False
            size = (work / f"{track_id}.wav").stat().st_size if ok else None
            results.append({"op": "download", "id": track_id, "ok": ok, "size": size})
        for track_id, kind, seconds in (("t0", Feedback.STARTED, 0.0), ("t1", Feedback.FINISHED, 49.9996), ("t2", Feedback.SKIPPED, 12.3456)):
            await backend.feedback_track(tracks.get(track_id) or dataclasses_track(library, track_id), kind, "fake", seconds)
            results.append({"op": "feedback", "id": track_id, "kind": kind.value, "seconds": seconds})
        settings = await backend.station_settings("fake")
        await backend.set_station_settings("fake", "calm", "default", "russian")
        after = await backend.station_settings("fake")
        results.append({"op": "settings", "before": list(settings), "after": list(after)})
        return {"results": results, "calls": backend.calls}

    def dataclasses_track(lib: list[dict[str, object]], track_id: str) -> object:
        from yamusic_radio.backend import Track

        entry = next((e for e in lib if e["track_id"] == track_id), {"track_id": track_id, "title": "?", "artists": [], "codec": "wav"})
        return Track(
            track_id=str(entry["track_id"]), title=str(entry["title"]), artists=tuple(entry["artists"]), codec=str(entry["codec"])
        )

    (folder / "backend.json").write_text(json.dumps(asyncio.run(backend_calls()), indent=1) + "\n")

    with tempfile.TemporaryDirectory() as scratch:
        scratch_path = Path(scratch)
        smoke = asyncio.run(harness.smoke(folder / "library", library, scratch_path / "work", scratch_path / "cache.db"))
    (folder / "smoke.json").write_text(json.dumps(smoke, indent=1) + "\n")
    print("radio: library of", len(library), "tracks; smoke:", [c["title"] for c in smoke["pool"]], smoke["queue"]["now"]["title"])


def write_effects() -> None:
    """The effects, the limiter and blend(): the noise is recorded, and the Rust side replays it."""
    folder = OUT / "effects"
    folder.mkdir(parents=True, exist_ok=True)
    short = music(21, 55, 880, 3.0)
    long_ = music(22, 50, 600, 8.0)
    other = music(23, 70, 2000, 8.0)
    cases: list[dict[str, object]] = []

    def save(name: str, **arrays: np.ndarray) -> None:
        for part, data in arrays.items():
            (folder / f"{name}.{part}.f32").write_bytes(np.ascontiguousarray(data, "<f4").tobytes())

    def save_noise(name: str, recorder: Recorder) -> list[int]:
        flat = np.concatenate(recorder.draws) if recorder.draws else np.zeros(0)
        (folder / f"{name}.noise.f64").write_bytes(np.ascontiguousarray(flat, "<f8").tobytes())
        return [len(d) for d in recorder.draws]

    for name, frames in (("riser_short", 30000), ("riser_long", 90000)):
        recorder = Recorder(31)
        want = dj_mix.riser(frames, recorder)
        save(name, want=want)
        cases.append({"name": name, "function": "riser", "frames": frames, "draws": save_noise(name, recorder)})
    for name, frames in (("impact_short", 20000), ("impact_long", 120000)):
        recorder = Recorder(32)
        want = dj_mix.impact(frames, recorder)
        save(name, want=want)
        cases.append({"name": name, "function": "impact", "frames": frames, "draws": save_noise(name, recorder)})
    for name, cut_share, level in (("echo_a", 0.55, 0.8), ("echo_b", 0.3, 1.0)):
        cut = round(len(long_) * cut_share)
        save(name, **{"in": long_, "want": dj_mix.echo_out(long_, cut, level)})
        cases.append({"name": name, "function": "echo_out", "cut": cut, "level": level})
    for name, source, gain in (
        ("limit_loud", other, 3.5),
        ("limit_clear", short, 0.5),
        ("limit_edge", music(24, 60, 900, 2.0), 1.9),
    ):
        pcm = (source * np.float32(gain)).astype(np.float32)
        save(name, **{"in": pcm, "want": dj_mix.limit(pcm)})
        cases.append({"name": name, "function": "limit"})
    for name, drop, period, level in (("drop_a", 120000, 22050.0, 0.7), ("drop_b", 60000, 20000.0, 1.0)):
        recorder = Recorder(33)
        want = dj_mix.add_drop(long_, drop, period, level, recorder)
        save(name, **{"in": long_, "want": want})
        cases.append(
            {
                "name": name,
                "function": "add_drop",
                "drop": drop,
                "period": period,
                "level": level,
                "draws": save_noise(name, recorder),
            }
        )
    grid = [float(g) for g in np.arange(17) * 22050.0 * 0.98]
    for name, kind, period, effects, pieces, beat_grid in (
        ("blend_swap_drop", dj_mix.Transition.BASS_SWAP, 22050.0, 0.8, (long_, other), None),
        ("blend_eq_drop", dj_mix.Transition.EQ_BLEND, 22050.0, 1.0, (long_, other), None),
        ("blend_sweep_drop", dj_mix.Transition.FILTER_SWEEP, 22050.0, 0.6, (long_, other), None),
        ("blend_sweep_grid", dj_mix.Transition.FILTER_SWEEP, 21609.0, 0.9, (long_, other), grid),
        ("blend_sweep_echo", dj_mix.Transition.FILTER_SWEEP, 0.0, 0.8, (long_, other), None),
        ("blend_swap_echo", dj_mix.Transition.BASS_SWAP, 0.0, 0.5, (long_, other), None),
        ("blend_swap_plain", dj_mix.Transition.BASS_SWAP, 22050.0, 0.0, (long_, other), None),
        ("blend_short", dj_mix.Transition.EQ_BLEND, 22050.0, 0.8, (music(25, 60, 800, 1.5), music(26, 60, 900, 1.5)), None),
    ):
        recorder = Recorder(34)
        out, inc = pieces
        want = dj_mix.blend(
            kind,
            out,
            inc,
            period,
            effects=effects,
            rng=recorder,
            beat_grid=None if beat_grid is None else np.array(beat_grid),
        )
        save(name, out=out, **{"in": inc}, want=want)
        applied = dj_mix.applied_blend(kind, len(out), period, effects)
        cases.append(
            {
                "name": name,
                "function": "blend",
                "kind": kind.value,
                "period": period,
                "effects": effects,
                "grid": beat_grid,
                "draws": save_noise(name, recorder),
                "applied": [applied.kind.value, applied.effect.value],
            }
        )
    (folder / "index.json").write_text(json.dumps({"cases": cases}, indent=1) + "\n")
    print(f"effects: {len(cases)} cases")


def write_blends() -> None:
    """Blends of two pieces, to compare by the signal to noise ratio (the FFTs round differently)."""
    folder = OUT / "blend"
    folder.mkdir(parents=True, exist_ok=True)
    pieces = {
        "short": (music(1, 55, 880, 3.0), music(2, 65, 1320, 3.0)),
        "long": (music(3, 50, 600, 7.0), music(4, 70, 2000, 7.0)),
    }
    cases = [
        ("crossfade", "short", "crossfade", {}),
        ("bass_swap_beat", "long", "bass_swap", {"beat_period": SR * 0.5}),
        ("bass_swap_plain", "short", "bass_swap", {}),
        ("bass_swap_at", "long", "bass_swap", {"swap": (100000.0, 20000.0)}),
        ("eq_blend_beat", "long", "eq_blend", {"beat_period": SR * 0.46875}),
        ("eq_blend_at", "short", "eq_blend", {"swap": (60000.0, 11025.0)}),
        ("filter_sweep_short", "short", "filter_sweep", {}),
        ("filter_sweep_long", "long", "filter_sweep", {}),
    ]
    index = []
    for name, source, function, arguments in cases:
        out, inc = pieces[source]
        want = getattr(dj_mix, function)(out, inc, **arguments)
        for part, data in (("out", out), ("in", inc), ("want", want)):
            (folder / f"{name}.{part}.f32").write_bytes(np.ascontiguousarray(data, "<f4").tobytes())
        index.append({"name": name, "function": function, "frames": len(out), **{k: v for k, v in arguments.items()}})
        print(f"blend/{name}")
    # The tempo glide: the pieces are the ones the radio reads (the blend and a margin for the last frames).
    margin = dj_mix.GLIDE_MARGIN
    glides = []
    for name, bpm, speed, beats, seeds in [
        ("glide_up", 120.0, 0.95, 16, (11, 12)),
        ("glide_down", 126.0, 1.06, 16, (13, 14)),
        ("glide_still", 124.0, 1.0, 8, (15, 16)),
        ("glide_short", 128.0, 0.97, 8, (17, 18)),
    ]:
        period = SR * 60.0 / bpm
        length = round(beats * period)
        out = music(seeds[0], 55, 700, (length + margin) / SR + 0.1)[: length + margin]
        inc = music(seeds[1], 65, 1500, (length * speed + margin) / SR + 0.1)[: int(length * speed) + margin]
        a, b, grid = dj_mix.tempo_glide(out, inc, bpm, speed, length)
        for part, data in (("out", out), ("in", inc), ("want_out", a), ("want_in", b)):
            (folder / f"{name}.{part}.f32").write_bytes(np.ascontiguousarray(data, "<f4").tobytes())
        glides.append({"name": name, "bpm": bpm, "speed": speed, "length": length, "grid": [float(g) for g in grid]})
        print(f"blend/{name}: {len(a)} frames")
    mono = music(5, 60, 900, 6.0).mean(axis=1, dtype=np.float32)
    (folder / "attacks.in.f32").write_bytes(np.ascontiguousarray(mono, "<f4").tobytes())
    attacks = [int(i) for i in np.flatnonzero(dj_mix.find_attacks(mono))]
    swaps = [
        {
            "frames": int(frames),
            "beat_period": period,
            "grid": grid,
            "swap": list(dj_mix.swap_point(int(frames), period, None if grid is None else np.array(grid))),
        }
        for frames, period, grid in [
            (100000, 0, None),
            (100000, 22050.0, None),
            (131072, 19999.5, None),
            (100000, 22050.0, [1000.0, 23000.0, 45500.0, 68000.0, 90000.0]),
            (100000, 22050.0, [1000.0, 23000.0]),
            (100000, 22050.0, [51000.0]),
            (100000, 22050.0, [49000.0, 51000.0, 99000.0]),
        ]
    ]
    (folder / "index.json").write_text(json.dumps({"cases": index, "swaps": swaps, "glides": glides, "attacks": attacks}, indent=1) + "\n")


def write_logic() -> None:
    folder = OUT / "logic"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "choose.json").write_text(json.dumps(logic(), separators=(",", ":")) + "\n")
    print("logic/choose.json")


if __name__ == "__main__":
    main()
    write_logic()
    write_blends()
    write_effects()
    write_cache()
    write_streams()
    write_spectrum()
    write_radio()

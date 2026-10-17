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
        "scenarios": scenarios,
    }


def write_logic() -> None:
    folder = OUT / "logic"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "choose.json").write_text(json.dumps(logic(), separators=(",", ":")) + "\n")
    print("logic/choose.json")


if __name__ == "__main__":
    main()
    write_logic()

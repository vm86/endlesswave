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

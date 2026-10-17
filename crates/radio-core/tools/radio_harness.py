"""A stand for running the Python radio on fake tracks, with a clock that is set by hand.

The Rust radio is checked against the Python one by running the same scenario on both and comparing what they say
after each step (docs/rust-port-radio.md, task 3.0.4). This file is the Python half:

* `build_library(folder)`  eight synthetic tracks as WAV files (mono, 22.05 kHz: ffmpeg brings them to the 44.1 kHz
                           stereo of the radio) and the list of them for the Rust side;
* `FakeBackend`            a backend that does what `src/backend/fake.rs` does, call for call;
* `VirtualTime`            replaces `time` in the radio's module, so that the seconds of a track heard are set by
                           the scenario and not by the wall clock (the event loop is left alone);
* `smoke()`                a first scenario, to see that the stand works.

Run through `gen_golden.py`, which needs the radio's checkout (YAMUSIC_RADIO).
"""

from __future__ import annotations

import dataclasses
import shutil
import wave
from pathlib import Path

import numpy as np

SAMPLE_RATE = 22050
BPMS = [118.0, 120.0, 122.0, 124.0, 126.0, 128.0, 130.0, 132.0]
# Tonics (semitones above C) of the bass of each track, and the genres.
TONICS = [0, 7, 9, 2, 4, 11, 5, 9]
GENRES = ["house", "techno", "house", "dance", "techno", "house", "dance", "techno"]
SECONDS = 50.0


def track_audio(index: int) -> np.ndarray:
    """Kicks on the beat, a bass, a lead and noise: a track the analysis can find a tempo and a key in."""
    rng = np.random.default_rng(100 + index)
    t = np.arange(int(SAMPLE_RATE * SECONDS)) / SAMPLE_RATE
    bpm = BPMS[index]
    period = SAMPLE_RATE * 60 / bpm
    kicks = np.zeros_like(t)
    kick_t = np.arange(2200) / SAMPLE_RATE
    kick = np.sin(2 * np.pi * 55 * kick_t) * np.exp(-kick_t * 28) * 0.6
    beat, first = 0, int(0.3 * SAMPLE_RATE)
    while (start := int(first + beat * period)) < len(t) - 2200:
        kicks[start : start + 2200] += kick
        beat += 1
    hz = 55.0 * 2 ** (TONICS[index] / 12)
    bass = 0.25 * np.sin(2 * np.pi * hz * t) + 0.12 * np.sin(2 * np.pi * hz * 2 * t)
    lead = 0.08 * np.sin(2 * np.pi * hz * 8 * t) * (0.6 + 0.4 * np.sin(2 * np.pi * 0.5 * t))
    mono = kicks + bass + lead + rng.normal(0, 0.01, len(t))
    # the first and the last seconds are quiet: an intro and an outro
    envelope = np.clip(np.minimum(t, SECONDS - t) / 4.0, 0.15, 1.0)
    return (mono * envelope * 0.8).astype(np.float32)


def build_library(folder: Path) -> list[dict[str, object]]:
    """Write the tracks to `folder` and return their descriptions (a Track of the radio each, as JSON)."""
    folder.mkdir(parents=True, exist_ok=True)
    library = []
    for index in range(len(BPMS)):
        name = f"t{index}"
        samples = (np.clip(track_audio(index), -1, 1) * 32767).astype("<i2")
        with wave.open(str(folder / f"{name}.wav"), "wb") as out:
            out.setnchannels(1)
            out.setsampwidth(2)
            out.setframerate(SAMPLE_RATE)
            out.writeframes(samples.tobytes())
        library.append(
            {
                "track_id": name,
                "title": f"Song {index}",
                "artists": [f"Artist {index % 4}"],
                "codec": "wav",
                "bitrate_in_kbps": 0,
                "genre": GENRES[index],
                "cover": "",
                "batch_id": "",
                "mood": "",
                "source": "",
                "native_id": "",
                "file": f"{name}.wav",
            }
        )
    return library


class FakeBackend:
    """The backend of `src/backend/fake.rs`: the same tracks in the same order, the same log."""

    supports_moods: bool

    def __init__(
        self,
        folder: Path,
        library: list[dict[str, object]],
        *,
        batch_size: int = 3,
        supports_moods: bool = False,
        default_station: str = "fake",
        settings: tuple[str, str, str] = ("all", "default", "any"),
        fail_downloads: frozenset[str] = frozenset(),
    ) -> None:
        from yamusic_radio.backend import Track

        fields = {f.name for f in dataclasses.fields(Track)}
        self._tracks = [
            (Track(**{k: (tuple(v) if k == "artists" else v) for k, v in entry.items() if k in fields}), folder / str(entry["file"]))
            for entry in library
        ]
        self._batch_size = batch_size
        self.supports_moods = supports_moods
        self._default_station = default_station
        self._fail = fail_downloads
        self._cursor = 0
        self._settings = settings
        self.calls: list[str] = []

    authorized = True

    @property
    def auth_prompt(self) -> object:
        from yamusic_radio.backend import AuthPrompt

        return AuthPrompt()

    @property
    def default_station(self) -> str:
        return self._default_station

    async def wait_authorized(self) -> None:
        return None

    def start_auth(self) -> None:
        return None

    async def authorize(self, values: dict[str, str], /) -> None:
        from yamusic_radio.backend import AuthError

        raise AuthError("the fake backend needs no sign-in")

    async def station_batch(self, station_id: str, /, *, exclude_track_ids: set[str]) -> list[object]:
        n = len(self._tracks)
        picked = []
        scanned = 0
        while len(picked) < self._batch_size and scanned < n:
            track = self._tracks[self._cursor % n][0]
            self._cursor += 1
            scanned += 1
            if track.track_id not in exclude_track_ids:
                picked.append(track)
        if not picked and n:
            for _ in range(min(self._batch_size, n)):
                picked.append(self._tracks[self._cursor % n][0])
                self._cursor += 1
        ids = ",".join(t.track_id for t in picked)
        self.calls.append(f"batch {station_id} exclude={','.join(sorted(exclude_track_ids))} -> {ids}")
        return picked

    async def download(self, track: object, path: Path, /) -> None:
        source = next((f for t, f in self._tracks if t.track_id == track.track_id), None)
        ok = source is not None and track.track_id not in self._fail
        self.calls.append(f"download {track.track_id} {'ok' if ok else 'fail'}")
        if not ok:
            raise RuntimeError(f"cannot download {track.track_id}")
        shutil.copyfile(source, path)

    async def feedback_track(self, track: object, feedback: object, station_id: str, seconds: float, /) -> None:
        self.calls.append(f"feedback {track.track_id} {feedback.value} {seconds:.3f}")

    async def station_settings(self, station_id: str, /) -> tuple[str, str, str]:
        self.calls.append(f"settings {station_id}")
        return self._settings

    async def set_station_settings(self, station_id: str, mood_energy: str, diversity: str, language: str, /) -> None:
        self.calls.append(f"set_settings {station_id} {mood_energy} {diversity} {language}")
        self._settings = (mood_energy, diversity, language)

    async def aclose(self) -> None:
        return None


class VirtualTime:
    """Stands in for the `time` module of the radio: the clock moves only when the scenario says so."""

    def __init__(self) -> None:
        self.now = 1000.0
        self.wall = 1_700_000_000.0

    def monotonic(self) -> float:
        return self.now

    def time(self) -> float:
        return self.wall

    def advance(self, seconds: float) -> None:
        self.now += seconds
        self.wall += seconds


def install_time(dj_module: object) -> VirtualTime:
    """Give the radio's module a clock of its own (the event loop keeps the real one)."""
    virtual = VirtualTime()
    dj_module.time = virtual  # type: ignore[attr-defined]
    return virtual


async def smoke(folder: Path, library: list[dict[str, object]], workdir: Path, cache_path: Path) -> dict[str, object]:
    """Fill a radio from the fake backend and say what it looks like: the pool, the queue, the calls."""
    from yamusic_radio import dj
    from yamusic_radio.cache import TrackCache

    clock = install_time(dj)
    backend = FakeBackend(folder, library)
    cache = TrackCache(cache_path)
    workdir.mkdir(parents=True, exist_ok=True)
    radio = dj.Radio(backend, workdir, pool_size=4, cache=cache)
    await radio.fill(4)
    deck = await radio.next_deck(None)
    radio.started(deck)
    clock.advance(12.5)
    snapshot = {
        "pool": [
            {
                "title": c.title,
                "bpm": c.analysis.bpm,
                "key": c.analysis.key,
                "loudness_db": c.analysis.loudness_db,
                "mood": c.mood,
                "genre": c.genre,
            }
            for c in radio.pool
        ],
        "queue": radio.queue(),
        "calls": backend.calls,
    }
    cache.close()
    return snapshot

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
import json
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
        authorized: bool = True,
        only: tuple[str, ...] | None = None,
    ) -> None:
        from yamusic_radio.backend import Track

        library = [e for e in library if only is None or e["track_id"] in only]

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
        self.authorized = authorized
        self.failing = False
        import asyncio

        self._signed_in = asyncio.Event()
        if authorized:
            self._signed_in.set()

    def set_authorized(self, authorized: bool) -> None:
        self.authorized = authorized
        if authorized:
            self._signed_in.set()
        else:
            self._signed_in.clear()

    @property
    def auth_prompt(self) -> object:
        from yamusic_radio.backend import AuthField, AuthPrompt

        if self.authorized:
            return AuthPrompt()
        return AuthPrompt("https://example.test/login", "CODE-1234", (AuthField("token", "Token", True),))

    @property
    def default_station(self) -> str:
        return self._default_station

    async def wait_authorized(self) -> None:
        await self._signed_in.wait()

    def start_auth(self) -> None:
        self.calls.append("start_auth")

    async def authorize(self, values: dict[str, str], /) -> None:
        from yamusic_radio.backend import AuthError

        if self.authorized:
            raise AuthError("the fake backend needs no sign-in")
        if values.get("token") == "good":
            self.set_authorized(True)
            return
        raise AuthError("the token is wrong")

    async def station_batch(self, station_id: str, /, *, exclude_track_ids: set[str]) -> list[object]:
        from yamusic_radio.backend import BackendError

        if self.failing:
            self.calls.append(f"batch {station_id} fails")
            raise BackendError("the service is down")
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
        from yamusic_radio.backend import BackendError

        ok = source is not None and track.track_id not in self._fail and not self.failing
        self.calls.append(f"download {track.track_id} {'ok' if ok else 'fail'}")
        if not ok:
            raise BackendError(f"cannot download {track.track_id}")
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


# ---- Scenarios of the whole Radio (docs/rust-port-radio.md, task 3.1) ---------------------------------------------------
#
# A scenario is a list of steps. The Python radio and the Rust one run the same steps and tell, after each of them, what
# the radio looks like (a snapshot); the two lists of snapshots must be the same.
#
#   {"op": "fill", "size": 4}                                  fill the pool to that many
#   {"op": "next_deck", "into": "b", "after": "a"}             make the deck to play after deck a (after: null for the first)
#   {"op": "started", "deck": "a"} / {"op": "finished", "deck": "a"}
#   {"op": "transition", "kind": "bass swap", "effect": "none", "current": "a", "following": "b"}
#   {"op": "advance", "seconds": 12.5}                         the clock moves
#   {"op": "call", "method": "pause", "args": []}              a method of the radio (errors are told by name)


def _info(deck: object) -> dict[str, object]:
    return {
        "title": deck.title,
        "bpm": deck.analysis.bpm,
        "key": deck.analysis.key,
        "beat_matched": deck.beat_matched,
        "speed": deck.speed,
        "generation": deck.generation,
        "mood": deck.mood,
    }


SETTINGS = ("playlist", "queue_order", "genres", "blocked_genres", "moods", "language", "station_settings")


def _snapshot(radio: object, backend: FakeBackend, workdir: Path, taken: list[int], cache: object) -> dict[str, object]:
    calls = backend.calls[taken[0] :]
    taken[0] = len(backend.calls)
    snapshot = {
        "queue": radio.queue(),
        "pool": [c.title for c in radio.pool],
        "files": sorted(p.name for p in workdir.iterdir()),
        "generation": radio.generation,
        "calls": calls,
        "stats": radio.transition_stats()["session"],
        "key_pairs": radio.key_pair_stats()["session"],
        "blends": radio.recent_blends(),
        "order": list(radio._order),
        "settings": {key: cache.get_setting(key) for key in SETTINGS},
    }
    return snapshot


class StubBackends:
    """The set of backends that can be turned on and off, as the page sees it. The Rust test has the same one."""

    IDS = ("alpha", "beta")

    def __init__(self) -> None:
        self.on = {"alpha"}

    async def enable(self, backend_id: str) -> None:
        if backend_id not in self.IDS:
            raise ValueError(f"unknown music backend {backend_id!r}")
        self.on.add(backend_id)

    async def disable(self, backend_id: str) -> None:
        if backend_id not in self.IDS:
            raise ValueError(f"unknown music backend {backend_id!r}")
        self.on.discard(backend_id)

    def status(self) -> list[dict[str, object]]:
        return [
            {
                "id": i,
                "name": i.title(),
                "enabled": i in self.on,
                "authorized": i in self.on,
                "available": i in self.on,
                "url": None,
                "code": None,
                "fields": [],
                "settings": {},
                "setting_fields": [],
            }
            for i in self.IDS
        ]

    async def authorize_backend(self, backend_id: str, values: dict[str, str]) -> None:
        from yamusic_radio.backend import AuthError

        if backend_id not in self.on:
            raise AuthError(f"music backend {backend_id!r} is not on")
        if values.get("token") != "good":
            raise AuthError("the token is wrong")

    def unavailable(self) -> set[str]:
        return set(self.IDS) - self.on


async def run_scenario(
    scenario: dict[str, object], folder: Path, library: list[dict[str, object]], primed: Path, scratch: Path
) -> list[dict[str, object]]:
    """Run the steps on the Python radio and return what it looked like after each."""
    from yamusic_radio import dj
    from yamusic_radio.cache import TrackCache

    clock = install_time(dj)
    options = scenario.get("backend", {})
    backend = FakeBackend(
        folder,
        library,
        batch_size=options.get("batch_size", 3),
        supports_moods=options.get("supports_moods", False),
        fail_downloads=frozenset(options.get("fail_downloads", [])),
        authorized=options.get("authorized", True),
    )
    cache_path = scratch / f"{scenario['name']}.db"
    shutil.copyfile(primed, cache_path)
    cache = TrackCache(cache_path)
    workdir = scratch / f"{scenario['name']}-work"
    workdir.mkdir(parents=True)
    def make_radio() -> object:
        return dj.Radio(
            backend,
            workdir,
            pool_size=scenario.get("pool_size", 4),
            cache=cache,
            mood_arc=scenario.get("mood_arc", False),
            keep_playlist=scenario.get("keep_playlist", False),
        )

    radio = make_radio()
    decks: dict[str, object] = {}
    taken = [0]
    results = []
    client = None

    async def web_client() -> object:
        """The HTTP side of the radio, started when a step first asks for it."""
        from aiohttp.test_utils import TestClient, TestServer

        hub, lossless, ogg = dj.Broadcaster(), dj.Broadcaster(), dj.Broadcaster()
        backends = StubBackends() if scenario.get("backends") else None
        app = dj.make_app(hub, radio, backends, lossless, dj.SpectrumFeed(), ogg=ogg)
        started = TestClient(TestServer(app))
        await started.start_server()
        return started

    for step in scenario["steps"]:
        result: dict[str, object] = {}
        op = step["op"]
        try:
            if op == "fill":
                await radio.fill(step["size"])
            elif op == "next_deck":
                after = decks[step["after"]].analysis if step.get("after") else None
                decks[step["into"]] = await radio.next_deck(after)
                result["deck"] = _info(decks[step["into"]])
            elif op == "started":
                radio.started(decks[step["deck"]])
            elif op == "finished":
                radio.finished(decks[step["deck"]])
            elif op == "transition":
                kind = next(k for k in dj.dj_mix.Transition if k.value == step["kind"])
                effect = next(e for e in dj.dj_mix.Effect if e.value == step["effect"])
                radio.transition(kind, decks[step["current"]], decks[step["following"]], effect)
            elif op == "advance":
                clock.advance(step["seconds"])
            elif op == "http":
                client = client or await web_client()
                body = json.dumps(step["json"]).encode() if "json" in step else str(step.get("raw", "")).encode()
                async with client.request(step["method"], step["path"], data=body or None) as response:
                    text = await response.text()
                    result["status"] = response.status
                    result["content_type"] = response.headers.get("Content-Type")
                    if "json" in (response.content_type or ""):
                        result["json"] = json.loads(text)
                    else:
                        result["text"] = text
            elif op == "restart":
                # the program is stopped and started again: the files and the cache stay
                cache.close()
                cache = TrackCache(cache_path)
                radio = make_radio()
                decks.clear()
            elif op == "call":
                method = getattr(radio, step["method"])
                args = step.get("args", [])
                if step["method"] == "hold":
                    outcome = method(held=args[0])
                else:
                    outcome = method(*args)
                    if hasattr(outcome, "__await__"):
                        outcome = await outcome
                if isinstance(outcome, (dict, list, str, int, float, bool)):
                    result["returned"] = outcome
            else:
                raise ValueError(op)
        except (dj.SkipRefusedError, dj.QueueChangeRefusedError, dj.UnknownQueueTrackError, ValueError) as error:  # noqa: PERF203
            result["error"] = type(error).__name__
        await radio.wait_feedback()
        result["snapshot"] = _snapshot(radio, backend, workdir, taken, cache)
        results.append(result)
    if client is not None:
        await client.close()
    cache.close()
    return results


async def prime_cache(folder: Path, library: list[dict[str, object]], cache_path: Path, workdir: Path) -> None:
    """Analyze the whole library once into a cache, which the scenarios then start from."""
    from yamusic_radio import dj
    from yamusic_radio.cache import TrackCache

    install_time(dj)
    backend = FakeBackend(folder, library, batch_size=len(library))
    cache = TrackCache(cache_path)
    workdir.mkdir(parents=True)
    radio = dj.Radio(backend, workdir, pool_size=len(library), cache=cache)
    await radio.fill(len(library))
    cache.close()


# ---- Scenarios of the MultiBackend (docs/rust-port.md, task 5.1) ------------------------------------------------------


async def run_multi_scenario(scenario: dict[str, object], folder: Path, library: list[dict[str, object]], scratch: Path) -> list[dict[str, object]]:
    """Run the steps on the Python MultiBackend, over fake backends, and say what it looked like after each."""
    import asyncio

    from yamusic_radio.backend import AuthError, Feedback
    from yamusic_radio.multi import BackendSpec, MultiBackend

    clock = VirtualTime()
    fakes: dict[str, FakeBackend] = {}
    specs = []
    for entry in scenario["backends"]:
        fakes[entry["id"]] = FakeBackend(
            folder,
            library,
            batch_size=2,
            supports_moods=entry.get("supports_moods", False),
            default_station=f"{entry['id']}-station",
            authorized=entry.get("authorized", True),
            only=tuple(entry["tracks"]),
        )
        specs.append(
            BackendSpec(
                entry["id"],
                entry["name"],
                (lambda i=entry["id"]: fakes[i]),
                (lambda c=entry.get("configured", False): c),
                entry.get("plain_ids", False),
            )
        )
    changes: list[list[str]] = []
    multi = MultiBackend(
        specs,
        enabled=scenario.get("enabled", []),
        auto=scenario.get("auto", True),
        stations=scenario.get("stations", {}),
        on_change=lambda ids: changes.append(list(ids)),
        clock=clock.monotonic,
    )
    tracks: dict[str, object] = {}
    taken = {i: 0 for i in fakes}
    reported = [0]
    results = []

    def query() -> dict[str, object]:
        prompt = multi.auth_prompt
        return {
            "authorized": multi.authorized,
            "prompt": {"url": prompt.url, "code": prompt.code, "fields": [dataclasses.asdict(f) for f in prompt.fields]},
            "default_station": multi.default_station,
            "supports_moods": multi.supports_moods,
            "status": multi.status(),
            "unavailable": sorted(multi.unavailable()),
        }

    for step in scenario["steps"]:
        result: dict[str, object] = {}
        op = step["op"]
        try:
            if op == "enable":
                await multi.enable(step["id"])
            elif op == "disable":
                await multi.disable(step["id"])
            elif op == "batch":
                batch = await multi.station_batch("ignored", exclude_track_ids=set(step.get("exclude", [])))
                result["tracks"] = [dataclasses.asdict(t) for t in batch]
                tracks.update({t.track_id: t for t in batch})
            elif op == "download":
                await multi.download(tracks[step["track"]], scratch / f"{scenario['name']}-{step['track'].replace(':', '_')}.wav")
            elif op == "feedback":
                await multi.feedback_track(tracks[step["track"]], Feedback(step["kind"]), "ignored", step["seconds"])
            elif op == "settings":
                result["settings"] = list(await multi.station_settings("ignored"))
            elif op == "set_settings":
                await multi.set_station_settings("ignored", step["mood"], step["diversity"], step["language"])
            elif op == "advance":
                clock.advance(step["seconds"])
            elif op == "set":
                fake = fakes[step["backend"]]
                if "failing" in step:
                    fake.failing = step["failing"]
                if "authorized" in step:
                    fake.set_authorized(step["authorized"])
            elif op == "authorize":
                await multi.authorize(step["values"])
            elif op == "authorize_backend":
                await multi.authorize_backend(step["id"], step["values"])
            elif op == "start_auth":
                multi.start_auth()
            elif op == "persist":
                multi.persist()
            elif op == "wait_authorized":
                try:
                    await asyncio.wait_for(multi.wait_authorized(), 0.3)
                    result["waited"] = "returned"
                except TimeoutError:
                    result["waited"] = "blocked"
            elif op == "query":
                pass
            else:
                raise ValueError(op)
        except (ValueError, AuthError, RuntimeError, OSError, KeyError) as error:
            if op not in ("download", "feedback") or not isinstance(error, KeyError):
                result["error"] = f"{type(error).__name__}: {error}"
            else:
                raise
        except Exception as error:  # a BackendError of the backend that went through
            result["error"] = f"{type(error).__name__}: {error}"
        await asyncio.sleep(0)
        snapshot = query()
        snapshot["calls"] = {}
        for i, fake in fakes.items():
            snapshot["calls"][i] = fake.calls[taken[i] :]
            taken[i] = len(fake.calls)
        snapshot["changes"] = changes[reported[0] :]
        reported[0] = len(changes)
        result["snapshot"] = snapshot
        results.append(result)
    return results


# ---- The local backend (docs/rust-port.md, task 5.2) --------------------------------------------------------------------


def build_music(root: Path) -> None:
    """A folder of audio files made by ffmpeg, with tags (and with what is not to be taken: hidden, other, empty)."""
    import subprocess

    def make(name: str, tags: dict[str, str], *args: str) -> None:
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        command = ["ffmpeg", "-y", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=0.4"]
        for key, value in tags.items():
            command += ["-metadata", f"{key}={value}"]
        subprocess.run([*command, *args, str(path)], check=True)

    make("a.mp3", {"title": "Alpha", "artist": "A. Rtist", "genre": "Deep HOUSE"})
    make("B.FLAC", {"album_artist": "Group", "artist": "Solo", "genre": "Techno"})
    make("sub/c.ogg", {}, "-c:a", "flac", "-f", "ogg")
    make("sub/deep/d.wav", {"title": "Rock = Roll; #1 \\ back", "artist": "Ünï Çode ✓"})
    make("sub/h.m4a", {"title": "Hotel", "artist": "Dub"})
    make("Other Folder/e.opus", {"title": "  Spaced  ", "genre": ""}, "-c:a", "flac", "-f", "ogg")
    make(".hidden/f.mp3", {"title": "Hidden folder"})
    make("._g.mp3", {"title": "Mac leftover"})
    (root / "notes.txt").write_text("not music")
    (root / "noext").write_text("not music")
    (root / "empty.mp3").write_bytes(b"")
    (root / "sub" / "deep" / "nested.AIFF").write_bytes(b"not audio at all")
    if hasattr(root, "symlink_to"):
        (root / "link-to-sub").symlink_to(root / "sub")


def _plain(text: str, root: Path) -> str:
    """Hide where the folder is: it is /var/... or /private/var/... as the system says it."""
    return text.replace(str(root.resolve()), "<root>").replace(str(root), "<root>")


async def run_local_scenario(scenario: dict[str, object], music: Path, scratch: Path) -> list[dict[str, object]]:
    """Run the steps on the Python LocalBackend over a copy of the folder, and say what came of each."""
    import asyncio
    import random

    from yamusic_radio.backend import AuthError, BackendError, Track
    from yamusic_radio.local import LocalBackend, scan

    root = scratch / "music"
    shutil.copytree(music, root, symlinks=True)
    clock = VirtualTime()
    chosen: list[str] = []
    backend = LocalBackend(None, chosen.append, batch_size=scenario.get("batch_size", 3), rng=random.Random(5), clock=clock.monotonic)
    work = scratch / "downloads"
    work.mkdir()
    last: list[Track] = []
    results = []
    for step in scenario["steps"]:
        result: dict[str, object] = {}
        op = step["op"]
        try:
            if op == "authorize":
                typed = step["directory"].replace("<root>", str(root))
                await backend.authorize({"directory": typed} if typed is not None else {})
                result["chosen"] = [_plain(c, root) for c in chosen]
            elif op == "authorize_raw":
                await backend.authorize(step["values"])
            elif op == "batch":
                exclude = {hashid(root, root / rel) for rel in step.get("exclude", [])}
                if step.get("exclude_all"):
                    exclude = set(scan(root))
                batch = await backend.station_batch(step.get("station", "shuffle"), exclude_track_ids=exclude)
                last[:] = batch
                result["tracks"] = sorted((dataclasses.asdict(t) for t in batch), key=lambda t: t["track_id"])
            elif op == "download":
                if "path" in step:
                    track = Track(hashid(root, root / step["path"]), step["title"], ("x",), "mp3")
                else:
                    track = Track(step["id"], "Gone", ("x",), "mp3")
                target = work / step["name"]
                await backend.download(track, target)
                result["link"] = {"is_symlink": target.is_symlink(), "target": _plain(str(target.resolve()), root.resolve()), "size": target.stat().st_size}
            elif op == "write":
                (root / step["path"]).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(root / "a.mp3", root / step["path"])
            elif op == "remove":
                (root / step["path"]).unlink()
            elif op == "advance":
                clock.advance(step["seconds"])
            elif op == "query":
                pass
            else:
                raise ValueError(op)
        except (AuthError, BackendError) as error:
            result["error"] = _plain(f"{type(error).__name__}: {error}", root)
        await asyncio.sleep(0)
        result["state"] = {
            "authorized": backend.authorized,
            "settings": {k: _plain(v, root) for k, v in backend.settings.items()},
            "setting_fields": [dataclasses.asdict(f) for f in backend.setting_fields],
            "prompt": dataclasses.asdict(backend.auth_prompt),
            "default_station": backend.default_station,
            "supports_moods": backend.supports_moods,
        }
        results.append(result)
    return results


def hashid(root: Path, path: Path) -> str:
    from yamusic_radio.local import track_id

    return track_id(path.relative_to(root))

#!/usr/bin/env python3
"""Say what the cache of the Python radio reads from a database that the Rust cache wrote (tests/cache.rs).

  YAMUSIC_RADIO=~/devel/yamusic-radio ~/devel/yamusic-radio/.venv/bin/python tools/check_cache.py DB
"""

import dataclasses
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(os.environ["YAMUSIC_RADIO"]).expanduser()))
from yamusic_radio import dj_mix  # noqa: E402
from yamusic_radio.cache import TrackCache  # noqa: E402

cache = TrackCache(Path(sys.argv[1]))
ids = [(f"r:{i}", "mp3") for i in range(10)] + [("r:0", "flac")]
print(
    json.dumps(
        {
            "key_detector": dj_mix.key_detector(),
            "loudness_meter": dj_mix.loudness_meter(),
            "analyses": [
                {
                    "track_id": track_id,
                    "codec": codec,
                    "analysis": None
                    if (found := cache.get_analysis(track_id, codec)) is None
                    else dataclasses.asdict(found),
                    "genre": cache.get_genre(track_id, codec),
                }
                for track_id, codec in ids
            ],
            "settings": {key: cache.get_setting(key) for key in ("moods", "text", "gone")},
            "stats": cache.transition_stats(),
            "key_pairs": [list(p) for p in cache.key_pairs()],
            "known_genres": [list(g) for g in cache.known_genres()],
            "recently_played": sorted(cache.recently_played(3)),
        },
        sort_keys=True,
    )
)

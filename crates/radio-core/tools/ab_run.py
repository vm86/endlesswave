#!/usr/bin/env python3
"""Stage 7.1: run one engine on a folder of music and record what it streams.

  ab_run.py python|rust MUSIC_DIR OUT_DIR [SECONDS]

Writes OUT_DIR/stream.flac (the lossless stream), OUT_DIR/rss.csv (memory and CPU time of the process every 5 s),
OUT_DIR/stats.json, transitions.json (what the API says at the end) and log.txt. Compare with ab_analyze.py.
"""

import json
import os
import subprocess
import sys
import threading
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
PYTHON_BIN = Path("~/devel/yamusic-radio/.venv/bin/yamusic-dj").expanduser()
RUST_BIN = HERE.parent / "target/release/yamusic-radio"


def get(port: int, path: str):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=10) as response:
        return json.load(response)


def main() -> None:
    engine, music, out = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
    seconds = float(sys.argv[4]) if len(sys.argv) > 4 else 900
    out.mkdir(parents=True, exist_ok=True)
    port = 18101 if engine == "python" else 18102
    common = ["--host", "127.0.0.1", "--port", str(port), "--backend", "local", "--local-dir", str(music), "--no-cache"]
    if engine == "python":
        command = [str(PYTHON_BIN), "--foreground", "--config", str(out / "config.json"), *common]
    else:
        command = [str(RUST_BIN), "--config", str(out / "config.json"), *common]
    log = (out / "log.txt").open("w")
    process = subprocess.Popen(command, stdout=log, stderr=log, stdin=subprocess.DEVNULL)
    started = time.time()
    try:
        # up, and a track is playing
        for _ in range(600):
            try:
                if get(port, "/api/queue").get("now") or get(port, "/api/queue").get("current"):
                    break
            except Exception:
                pass
            time.sleep(1)
        ready = time.time() - started
        print(f"{engine}: first track after {ready:.0f} s", flush=True)

        stop = threading.Event()

        def record() -> None:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/stream.flac", timeout=60) as response, (out / "stream.flac").open("wb") as file:
                while not stop.is_set():
                    chunk = response.read(1 << 16)
                    if not chunk:
                        break
                    file.write(chunk)

        recorder = threading.Thread(target=record, daemon=True)
        recorder.start()
        begin = time.time()
        with (out / "rss.csv").open("w") as rss:
            rss.write("t,rss_kb,cpu\n")
            while time.time() - begin < seconds:
                ps = subprocess.run(["ps", "-o", "rss=,time=", "-p", str(process.pid)], capture_output=True, text=True).stdout.split()
                if ps:
                    rss.write(f"{time.time() - begin:.0f},{ps[0]},{ps[1]}\n")
                    rss.flush()
                time.sleep(5)
        stop.set()
        for name, path in (("stats", "/api/stats"), ("transitions", "/api/transitions"), ("queue", "/api/queue")):
            try:
                (out / f"{name}.json").write_text(json.dumps(get(port, path), indent=1))
            except Exception as error:
                (out / f"{name}.json").write_text(json.dumps({"error": str(error)}))
        recorder.join(5)
    finally:
        process.terminate()
        try:
            process.wait(30)
        except subprocess.TimeoutExpired:
            process.kill()


if __name__ == "__main__":
    main()

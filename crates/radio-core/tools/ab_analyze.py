#!/usr/bin/env python3
"""Stage 7.1: compare the recordings of ab_run.py. ab_analyze.py DIR [DIR...] (each with stream.flac, rss.csv, log.txt)."""

import json
import re
import subprocess
import sys
from pathlib import Path

import numpy as np

SR = 44100


def decode(path: Path) -> np.ndarray:
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", str(path), "-f", "f32le", "-ac", "2", "-ar", str(SR), "-"], capture_output=True, check=True).stdout
    return np.frombuffer(raw, dtype="<f4").reshape(-1, 2)


def loudness(path: Path) -> dict[str, float]:
    err = subprocess.run(["ffmpeg", "-nostats", "-i", str(path), "-af", "ebur128=peak=true", "-f", "null", "-"], capture_output=True, text=True).stderr
    summary = err[err.rfind("Summary:") :]
    pick = lambda pattern: float(re.search(pattern, summary).group(1))  # noqa: E731
    return {"lufs_i": pick(r"I:\s+(-?[\d.]+) LUFS"), "lra": pick(r"LRA:\s+(-?[\d.]+) LU"), "true_peak_db": pick(r"Peak:\s+(-?[\d.]+) dBFS")}


def clicks(audio: np.ndarray) -> int:
    """Jumps between neighbouring samples that stand out of their surroundings: a click, not music."""
    mono = audio.mean(axis=1)
    jump = np.abs(np.diff(mono))
    window = SR // 20
    pad = np.pad(jump, (window, window), mode="edge")
    local = np.sqrt(np.convolve(pad**2, np.ones(2 * window + 1) / (2 * window + 1), mode="valid"))[: len(jump)]
    hits = np.flatnonzero((jump > 0.3) & (jump > 12 * local))
    # one event is many samples
    return int(np.sum(np.diff(hits, prepend=-SR) > SR // 50)) if len(hits) else 0


def silences(audio: np.ndarray) -> int:
    quiet = np.abs(audio).max(axis=1) < 1e-4
    runs, count = 0, 0
    for flag in quiet:
        count = count + 1 if flag else 0
        runs += count == SR // 5
    return runs


def short_term_spread(audio: np.ndarray) -> float:
    mono = audio.mean(axis=1)
    step = SR * 3
    levels = [20 * np.log10(np.sqrt(np.mean(mono[i : i + step] ** 2)) + 1e-9) for i in range(0, len(mono) - step, step)]
    return float(np.std(levels))


def memory(path: Path) -> dict[str, float]:
    rows = [line.split(",") for line in path.read_text().splitlines()[1:]]
    rss = [int(r[1]) / 1024 for r in rows]
    cpu = rows[-1][2]
    parts = [float(x) for x in cpu.replace(":", " ").split()]
    seconds = sum(p * f for p, f in zip(reversed(parts), (1, 60, 3600)))
    return {"rss_mb_max": max(rss), "rss_mb_mean": sum(rss) / len(rss), "cpu_seconds": seconds}


def log_facts(path: Path) -> dict[str, object]:
    text = path.read_text()
    glides = [float(x) for x in re.findall(r"glides by ([\d.]+)%", text)]
    return {
        "transitions": len(re.findall(r"\bNext:", text)),
        "glide_pct_mean": round(sum(glides) / len(glides), 1) if glides else None,
        "glide_pct_max": max(glides) if glides else None,
        "errors": len(re.findall(r"ERROR|Traceback", text)),
        "warnings": len(re.findall(r"WARN", text)),
    }


def main() -> None:
    report = {}
    for folder in map(Path, sys.argv[1:]):
        audio = decode(folder / "stream.flac")
        report[folder.name] = {
            "seconds": round(len(audio) / SR, 1),
            "peak": round(float(np.abs(audio).max()), 4),
            "clipped_samples": int(np.sum(np.abs(audio) >= 0.999)),
            **loudness(folder / "stream.flac"),
            "clicks": clicks(audio),
            "silences_over_0.2s": silences(audio),
            "short_term_db_std": round(short_term_spread(audio), 2),
            **{k: round(v, 1) for k, v in memory(folder / "rss.csv").items()},
            **log_facts(folder / "log.txt"),
        }
    print(json.dumps(report, indent=1, ensure_ascii=False))


if __name__ == "__main__":
    main()

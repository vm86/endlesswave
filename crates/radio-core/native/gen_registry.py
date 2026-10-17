#!/usr/bin/env python3 -I
"""Make what build.rs needs to compile the part of Essentia that the radio uses.

Essentia registers its algorithms in a generated file (waf makes it from the headers). There is no waf here, so
this script does the same for the algorithms in ALGORITHMS and writes
  native/essentia_algorithms_reg.cpp   the registration of those algorithms,
  native/sources.txt                   the .cpp files of Essentia to compile, relative to vendor/essentia/src.

  python3 -I native/gen_registry.py vendor/essentia
"""

import sys
from pathlib import Path

# KeyExtractor and LoudnessEBUR128 and what they are made of (found by linking until nothing was missing).
ALGORITHMS = [
    "KeyExtractor",
    "LoudnessEBUR128",
    "LoudnessEBUR128Filter",
    "FrameCutter",
    "Windowing",
    "Spectrum",
    "SpectralPeaks",
    "SpectralWhitening",
    "HPCP",
    "Key",
    "Mean",
    "UnaryOperatorStream",
    "NoiseAdder",
    "Magnitude",
    "PeakDetection",
    "StereoDemuxer",
    "IIR",
    "BinaryOperatorStream",
    "FFTK",
    "FFT",
    "VectorInput",
]

here = Path(__file__).resolve().parent
root = Path(sys.argv[1]).resolve()
sys.path.insert(0, str(root))
from utils.algorithms_info import create_registration_cpp, get_all_algorithms  # noqa: E402

found = get_all_algorithms(str(root / "src" / "algorithms"), root_dir=str(root / "src"))
chosen = {}
missing = []
for name in ALGORITHMS:
    if name in found:
        chosen[name] = found[name]
    elif name not in ("FFT", "VectorInput"):
        missing.append(name)
if missing:
    sys.exit(f"not found in Essentia: {missing}")

create_registration_cpp(chosen, str(here / "essentia_algorithms_reg.cpp"), use_streaming=True)
sources = sorted(algo["source"] for algo in chosen.values())
(here / "sources.txt").write_text("\n".join(sources) + "\n")
print(f"{len(chosen)} algorithms:", ", ".join(chosen))

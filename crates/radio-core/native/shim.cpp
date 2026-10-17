// The part of Essentia that the radio uses, behind a C interface: the key and the integrated loudness.
// Task 1.5 of docs/rust-port.md. The Python radio asks the same algorithms (dj_mix.py: _KEY_EXTRACTOR,
// measure_loudness) with the same parameters.

#include <algorithmfactory.h>
#include <essentia.h>
#include <types.h>

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <string>
#include <vector>

using namespace essentia;
using namespace essentia::standard;

namespace {

std::once_flag started;

void start() {
  std::call_once(started, [] {
    essentia::init();
    // After a loudness is measured, the key extractor says "No network created" for every frame it takes
    // (thousands of lines a track); the result is the same.
    essentia::warningLevelActive = false;
    essentia::infoLevelActive = false;
  });
}

}  // namespace

extern "C" {

// The key of a mono signal with the profile made for electronic music (edma).
// name: at least 4 bytes, the tonic as Essentia spells it ("C", "C#", "Db", ...). *minor: 1 for a minor key.
// Returns 0, or -1 when Essentia fails.
int ew_key(const float* mono, size_t frames, int sample_rate, char* name, int* minor, float* strength) {
  try {
    start();
    std::vector<Real> signal(mono, mono + frames);
    Algorithm* extractor =
        AlgorithmFactory::create("KeyExtractor", "profileType", "edma", "sampleRate", (Real)sample_rate);
    std::string key, scale;
    Real found = 0;
    extractor->input("audio").set(signal);
    extractor->output("key").set(key);
    extractor->output("scale").set(scale);
    extractor->output("strength").set(found);
    extractor->compute();
    delete extractor;
    std::strncpy(name, key.c_str(), 3);
    name[3] = 0;
    *minor = scale == "minor" ? 1 : 0;
    *strength = found;
    return 0;
  } catch (std::exception& e) {
    if (std::getenv("ESSENTIA_SHIM_DEBUG")) std::fprintf(stderr, "essentia: %s\n", e.what());
    return -1;
  } catch (...) {
    return -1;
  }
}

// The integrated loudness (EBU R128, LUFS) of a stereo signal, interleaved. Returns 0, or -1 on a failure.
int ew_loudness(const float* stereo, size_t frames, int sample_rate, double* integrated) {
  try {
    start();
    std::vector<StereoSample> signal(frames);
    for (size_t i = 0; i < frames; i++) {
      signal[i].first = stereo[2 * i];
      signal[i].second = stereo[2 * i + 1];
    }
    Algorithm* meter = AlgorithmFactory::create("LoudnessEBUR128", "sampleRate", (Real)sample_rate);
    std::vector<Real> momentary, shortTerm;
    Real total = 0, range = 0;
    meter->input("signal").set(signal);
    meter->output("momentaryLoudness").set(momentary);
    meter->output("shortTermLoudness").set(shortTerm);
    meter->output("integratedLoudness").set(total);
    meter->output("loudnessRange").set(range);
    meter->compute();
    delete meter;
    *integrated = total;
    return 0;
  } catch (std::exception& e) {
    if (std::getenv("ESSENTIA_SHIM_DEBUG")) std::fprintf(stderr, "essentia: %s\n", e.what());
    return -1;
  } catch (...) {
    return -1;
  }
}

}  // extern "C"

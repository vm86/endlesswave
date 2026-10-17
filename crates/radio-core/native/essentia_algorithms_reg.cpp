#include "algorithmfactory.h"
#include "algorithms/extractor/keyextractor.h"
#include "algorithms/temporal/loudnessebur128.h"
#include "algorithms/temporal/loudnessebur128filter.h"
#include "algorithms/standard/framecutter.h"
#include "algorithms/standard/windowing.h"
#include "algorithms/standard/spectrum.h"
#include "algorithms/spectral/spectralpeaks.h"
#include "algorithms/spectral/spectralwhitening.h"
#include "algorithms/spectral/hpcp.h"
#include "algorithms/tonal/key.h"
#include "algorithms/stats/mean.h"
#include "algorithms/standard/unaryoperatorstream.h"
#include "algorithms/standard/noiseadder.h"
#include "algorithms/complex/magnitude.h"
#include "algorithms/standard/peakdetection.h"
#include "algorithms/standard/stereodemuxer.h"
#include "algorithms/filters/iir.h"
#include "algorithms/standard/binaryoperatorstream.h"
#include "algorithms/standard/fftk.h"

namespace essentia {
namespace standard {

ESSENTIA_API void registerAlgorithm() {
    AlgorithmFactory::Registrar<KeyExtractor> regKeyExtractor;
    AlgorithmFactory::Registrar<LoudnessEBUR128> regLoudnessEBUR128;
    AlgorithmFactory::Registrar<FrameCutter> regFrameCutter;
    AlgorithmFactory::Registrar<Windowing> regWindowing;
    AlgorithmFactory::Registrar<Spectrum> regSpectrum;
    AlgorithmFactory::Registrar<SpectralPeaks> regSpectralPeaks;
    AlgorithmFactory::Registrar<SpectralWhitening> regSpectralWhitening;
    AlgorithmFactory::Registrar<HPCP> regHPCP;
    AlgorithmFactory::Registrar<Key> regKey;
    AlgorithmFactory::Registrar<Mean> regMean;
    AlgorithmFactory::Registrar<UnaryOperatorStream> regUnaryOperatorStream;
    AlgorithmFactory::Registrar<NoiseAdder> regNoiseAdder;
    AlgorithmFactory::Registrar<Magnitude> regMagnitude;
    AlgorithmFactory::Registrar<PeakDetection> regPeakDetection;
    AlgorithmFactory::Registrar<StereoDemuxer> regStereoDemuxer;
    AlgorithmFactory::Registrar<IIR> regIIR;
    AlgorithmFactory::Registrar<BinaryOperatorStream> regBinaryOperatorStream;
    AlgorithmFactory::Registrar<FFTK> regFFTK;
}}}



namespace essentia {
namespace streaming {

ESSENTIA_API void registerAlgorithm() {
    AlgorithmFactory::Registrar<KeyExtractor, essentia::standard::KeyExtractor> regKeyExtractor;
    AlgorithmFactory::Registrar<LoudnessEBUR128, essentia::standard::LoudnessEBUR128> regLoudnessEBUR128;
    AlgorithmFactory::Registrar<LoudnessEBUR128Filter> regLoudnessEBUR128Filter;
    AlgorithmFactory::Registrar<FrameCutter, essentia::standard::FrameCutter> regFrameCutter;
    AlgorithmFactory::Registrar<Windowing, essentia::standard::Windowing> regWindowing;
    AlgorithmFactory::Registrar<Spectrum, essentia::standard::Spectrum> regSpectrum;
    AlgorithmFactory::Registrar<SpectralPeaks, essentia::standard::SpectralPeaks> regSpectralPeaks;
    AlgorithmFactory::Registrar<SpectralWhitening, essentia::standard::SpectralWhitening> regSpectralWhitening;
    AlgorithmFactory::Registrar<HPCP, essentia::standard::HPCP> regHPCP;
    AlgorithmFactory::Registrar<Key, essentia::standard::Key> regKey;
    AlgorithmFactory::Registrar<Mean, essentia::standard::Mean> regMean;
    AlgorithmFactory::Registrar<UnaryOperatorStream, essentia::standard::UnaryOperatorStream> regUnaryOperatorStream;
    AlgorithmFactory::Registrar<NoiseAdder, essentia::standard::NoiseAdder> regNoiseAdder;
    AlgorithmFactory::Registrar<Magnitude, essentia::standard::Magnitude> regMagnitude;
    AlgorithmFactory::Registrar<PeakDetection, essentia::standard::PeakDetection> regPeakDetection;
    AlgorithmFactory::Registrar<StereoDemuxer, essentia::standard::StereoDemuxer> regStereoDemuxer;
    AlgorithmFactory::Registrar<IIR, essentia::standard::IIR> regIIR;
    AlgorithmFactory::Registrar<BinaryOperatorStream, essentia::standard::BinaryOperatorStream> regBinaryOperatorStream;
    AlgorithmFactory::Registrar<FFTK, essentia::standard::FFTK> regFFTK;
}}}

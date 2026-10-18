//! `decode_compare FILE...`: the native decoder against ffmpeg (stage 8.1): the length, the SNR at no shift, and what the
//! analysis says of each.
use radio_core::mix::{
    analyze,
    decode::{NativeError, decode_native, decode_with_ffmpeg},
};
use std::path::Path;

fn main() {
    for path in std::env::args().skip(1) {
        let path = Path::new(&path);
        let name = path.file_name().unwrap().to_string_lossy();
        let reference = decode_with_ffmpeg(Path::new("ffmpeg"), path);
        let native = decode_native(path);
        let (Ok(reference), native) = (reference, native) else {
            println!("{name}: ffmpeg fails");
            continue;
        };
        let native = match native {
            Ok(native) => native,
            Err(NativeError::Unsupported(why)) => {
                println!("{name}: UNSUPPORTED {why}");
                continue;
            }
            Err(NativeError::Failed(why)) => {
                println!("{name}: FAILED {why}");
                continue;
            }
        };
        let n = reference.len().min(native.len());
        let (mut signal, mut noise) = (0f64, 0f64);
        for i in 0..n {
            let (r, d) = (
                f64::from(reference[i]),
                f64::from(native[i]) - f64::from(reference[i]),
            );
            signal += r * r;
            noise += d * d;
        }
        let snr = 10.0 * (signal / noise.max(1e-30)).log10();
        if snr < 60.0 {
            // a shift?
            let best = (-400i64..=400)
                .map(|lag| {
                    let (mut sg, mut ns) = (0f64, 0f64);
                    for i in (1000..400_000usize.min(n - 1000)).step_by(2) {
                        let j = (i as i64 + lag * 2) as usize;
                        let (r, d) = (
                            f64::from(reference[i]),
                            f64::from(native[j]) - f64::from(reference[i]),
                        );
                        sg += r * r;
                        ns += d * d;
                    }
                    (10.0 * (sg / ns.max(1e-30)).log10(), lag)
                })
                .fold((f64::MIN, 0), |a, b| if b.0 > a.0 { b } else { a });
            println!("  best shift {} frames: snr {:.1} dB", best.1, best.0);
        }
        let (a, b) = (analyze(&reference), analyze(&native));
        println!(
            "{name}: frames {} vs {} ({:+}), snr {snr:.1} dB, bpm {:.1}/{:.1}, loud {:.2}/{:.2}, key {}/{}",
            native.len() / 2,
            reference.len() / 2,
            native.len() as i64 / 2 - reference.len() as i64 / 2,
            b.bpm,
            a.bpm,
            b.loudness_db,
            a.loudness_db,
            b.key,
            a.key
        );
    }
}

//! Builds the part of Essentia that the radio uses (feature `essentia`), from vendor/essentia, without waf:
//! the core, the algorithms of native/sources.txt, KissFFT and the C interface of native/shim.cpp.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native");
    #[cfg(feature = "essentia")]
    essentia();
}

#[cfg(feature = "essentia")]
fn essentia() {
    use std::{fs, path::Path};

    let root = Path::new("vendor/essentia/src");
    let eigen = Path::new("vendor/eigen");
    assert!(
        root.join("essentia/essentia.cpp").exists() && eigen.join("Eigen").exists(),
        "vendor/essentia and vendor/eigen are missing: see docs/rust-port.md, task 1.5"
    );

    // The core: everything in essentia/, but what needs libraries that are not used (libav, yaml) and the
    // extractors, which need algorithms that are not built.
    let mut sources = Vec::new();
    collect(&root.join("essentia"), &mut sources);
    sources.retain(|path| {
        let name = path.to_string_lossy();
        !(name.contains("audiocontext") || name.contains("yaml") || name.contains("jsonconvert") || name.contains("extractor_"))
    });
    for line in fs::read_to_string("native/sources.txt").unwrap().lines().filter(|l| !l.is_empty()) {
        sources.push(root.join(line));
    }
    sources.push(root.join("3rdparty/kiss_fft130/kiss_fft.c"));
    sources.push(root.join("3rdparty/kiss_fft130/tools/kiss_fftr.c"));

    let mut cpp = cc::Build::new();
    cpp.cpp(true).std("c++14").warnings(false).opt_level(3);
    let mut c = cc::Build::new();
    c.warnings(false).opt_level(3);
    for build in [&mut cpp, &mut c] {
        for dir in [
            "",
            "essentia",
            "essentia/scheduler",
            "essentia/streaming",
            "essentia/streaming/algorithms",
            "essentia/utils",
            "3rdparty",
            "3rdparty/kiss_fft130",
            "3rdparty/kiss_fft130/tools",
            "algorithms",
        ] {
            build.include(root.join(dir));
        }
        build.include("native").include(eigen);
        build.define("EIGEN_MPL2_ONLY", None).define("EIGEN_PERMANENTLY_DISABLE_STUPID_WARNINGS", None);
        build.define("__STDC_CONSTANT_MACROS", None);
        build.define("HAVE_KISS", None); // FFT by KissFFT
    }
    let (c_files, cpp_files): (Vec<_>, Vec<_>) = sources.iter().partition(|p| p.extension().is_some_and(|e| e == "c"));
    cpp.files(cpp_files).file("native/essentia_algorithms_reg.cpp").file("native/shim.cpp").compile("essentia_part");
    c.files(c_files).compile("essentia_kiss");
}

#[cfg(feature = "essentia")]
fn collect(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, found);
        } else if path.extension().is_some_and(|e| e == "cpp") {
            found.push(path);
        }
    }
}

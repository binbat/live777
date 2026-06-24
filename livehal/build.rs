use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Detect the C++ standard library name preferred by the active C++ compiler.
///
/// On most Linux targets the default compiler is g++ which links libstdc++.
/// On clang-based toolchains or macOS the preferred name is `c++` (libc++ on
/// Apple platforms, often still libstdc++ on Linux clang).  This avoids the
/// previous hard-coded `stdc++` which fails on libc++-only systems.
/// Result of probing a pkg-config library in a specific environment.
struct ProbedLibrary {
    include_paths: Vec<PathBuf>,
    link_paths: Vec<PathBuf>,
    libs: Vec<String>,
}

/// Probe `libcamera` via pkg-config inside the given sysroot without mutating
/// the current process environment.
fn probe_libcamera_in_sysroot(sysroot: &Path, triplet: &str) -> Option<ProbedLibrary> {
    let pkg_config_path = sysroot.join(format!("usr/lib/{triplet}/pkgconfig"));

    let output = Command::new("pkg-config")
        .env("PKG_CONFIG_SYSROOT_DIR", sysroot.as_os_str())
        .env("PKG_CONFIG_PATH", pkg_config_path.as_os_str())
        .env("PKG_CONFIG_ALLOW_CROSS", "1")
        .args(["--cflags", "--libs", "libcamera"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut result = ProbedLibrary {
        include_paths: Vec::new(),
        link_paths: Vec::new(),
        libs: Vec::new(),
    };

    for token in stdout.split_whitespace() {
        if let Some(path) = token.strip_prefix("-I") {
            result.include_paths.push(PathBuf::from(path));
        } else if let Some(path) = token.strip_prefix("-L") {
            result.link_paths.push(PathBuf::from(path));
        } else if let Some(lib) = token.strip_prefix("-l") {
            result.libs.push(lib.to_string());
        }
    }

    Some(result)
}

fn detect_cpp_stdlib(target_os: &str) -> String {
    // Allow explicit override for cross-compilation environments.
    if let Ok(name) = env::var("LIVEHAL_CXX_STDLIB") {
        return name;
    }

    let compiler = cc::Build::new().cpp(true).get_compiler();
    let path = compiler.path().to_string_lossy().to_lowercase();

    if path.contains("clang") {
        // On Apple platforms clang links libc++ via the `c++` surrogate.
        // On Linux, clang typically defaults to libstdc++ unless
        // `-stdlib=libc++` is explicitly passed.  Use the Cargo target OS
        // rather than the build host OS so cross-compilation picks the right
        // library.
        if target_os == "macos" {
            "c++".to_string()
        } else {
            "stdc++".to_string()
        }
    } else {
        // g++ and most other Linux toolchains default to libstdc++.
        "stdc++".to_string()
    }
}

fn main() {
    // Rebuild when environment variables that affect sysroot/linker selection
    // change.
    println!("cargo:rerun-if-env-changed=PI_SYSROOT");
    println!("cargo:rerun-if-env-changed=RDK_SYSROOT");
    println!("cargo:rerun-if-env-changed=LIVEHAL_CXX_STDLIB");
    println!("cargo:rerun-if-env-changed=LIVEHAL_RDK_ALLOW_UNDEFINED");

    let has_capture_libcamera = env::var("CARGO_FEATURE_CAPTURE_LIBCAMERA").is_ok();
    let has_capture_v4l2 = env::var("CARGO_FEATURE_CAPTURE_V4L2").is_ok();
    let has_encoder_v4l2_m2m = env::var("CARGO_FEATURE_ENCODER_V4L2_M2M").is_ok();
    let has_encoder_rdk = env::var("CARGO_FEATURE_ENCODER_RDK").is_ok();

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // Native C++ backends (V4L2, libcamera, encoder) require Linux
    // kernel headers.  On non-Linux hosts (macOS, Windows), skip CMake
    // entirely to avoid compiling kernel-dependent code.
    if target_os != "linux" {
        if has_capture_libcamera || has_capture_v4l2 || has_encoder_v4l2_m2m || has_encoder_rdk {
            println!(
                "cargo:warning=native backend requires Linux (current: {target_os}); \
                 CMake build skipped. Use a Linux target or omit native features."
            );
        }
        return;
    }

    // RDK X5 backend requires aarch64 Linux (ARM NEON + Horizon SDK).
    // On x86_64 Linux (e.g. CI all-features), RDK is disabled but
    // host-safe backends (v4l2, v4l2-m2m) still compile.
    let rdk_available = has_encoder_rdk && target_arch == "aarch64";
    if has_encoder_rdk && !rdk_available {
        println!(
            "cargo:warning=encoder-rdk requires aarch64 (current: {target_arch}); \
             falling back to generic-v4l2."
        );
    }

    // Target triplet for sysroot pkg-config paths.
    let target_triplet = match target_arch.as_str() {
        "aarch64" => "aarch64-linux-gnu",
        "arm" => "arm-linux-gnueabihf",
        "x86_64" => "x86_64-linux-gnu",
        other => {
            println!(
                "cargo:warning=unknown target architecture '{other}', \
                 defaulting triplet to x86_64-linux-gnu"
            );
            "x86_64-linux-gnu"
        }
    };

    // Encoder-only without capture: warn and skip CMake.
    // The SourcePipeline requires a capture backend — encoder-only builds
    // have no standalone pipeline.  Use a native-* preset or enable a
    // capture-* feature alongside the encoder.
    if !has_capture_libcamera && !has_capture_v4l2 {
        if has_encoder_v4l2_m2m || has_encoder_rdk {
            println!(
                "cargo:warning=encoder feature(s) enabled without any capture-* feature; \
                 CMake build skipped."
            );
            println!(
                "cargo:warning=Use a native-* preset (e.g. native-rdk) or enable \
                 capture-v4l2 / capture-libcamera alongside the encoder."
            );
        }
        return;
    }

    // Native backend selection — inferred from enabled capture/encoder features.
    // libcamera is Pi-specific; rdk-x5 is aarch64-specific; otherwise generic-v4l2.
    // capture-libcamera and encoder-rdk are mutually exclusive presets; if both
    // are enabled (e.g. `cargo --all-features`), prefer the libcamera backend and
    // ignore encoder-rdk instead of panicking.
    let native_backend = if has_capture_libcamera {
        if has_encoder_rdk {
            println!(
                "cargo:warning=capture-libcamera and encoder-rdk are incompatible; \
                 encoder-rdk will be ignored for this build"
            );
        }
        "rpi".to_string()
    } else if has_encoder_rdk && rdk_available {
        "rdk-x5".to_string()
    } else if has_capture_v4l2 || has_encoder_v4l2_m2m {
        "generic-v4l2".to_string()
    } else {
        return;
    };

    // Rerun-if-changed — all source files that affect the native build
    println!("cargo:rerun-if-changed=native-pipeline/CMakeLists.txt");

    // Core pipeline files
    println!("cargo:rerun-if-changed=native-pipeline/src/pipeline/source_pipeline.cpp");
    println!("cargo:rerun-if-changed=native-pipeline/src/pipeline/backend_factory.cpp");
    println!("cargo:rerun-if-changed=native-pipeline/include/source_pipeline_ffi.h");
    println!("cargo:rerun-if-changed=native-pipeline/include/capture_backend.h");
    println!("cargo:rerun-if-changed=native-pipeline/include/encoder_backend.h");
    println!("cargo:rerun-if-changed=native-pipeline/include/media_types.h");

    if rdk_available {
        println!("cargo:rerun-if-changed=native-pipeline/encoder_rdk.cpp");
    }
    if has_capture_v4l2 && rdk_available {
        println!("cargo:rerun-if-changed=native-pipeline/v4l2_capture_rdk.cpp");
    }
    if has_capture_libcamera {
        println!("cargo:rerun-if-changed=native-pipeline/camera.cpp");
    }
    if has_encoder_v4l2_m2m {
        println!("cargo:rerun-if-changed=native-pipeline/encoder.cpp");
    }
    if has_capture_v4l2 && !rdk_available {
        println!("cargo:rerun-if-changed=native-pipeline/v4l2_capture.cpp");
    }

    // Track whether libcamera link flags were already emitted via pkg-config.
    let mut libcamera_linked = false;

    // Setup sysroot paths
    if native_backend == "rdk-x5" {
        if let Ok(sysroot) = env::var("RDK_SYSROOT") {
            let sysroot = PathBuf::from(sysroot);
            println!(
                "cargo:rustc-link-search=native={}",
                sysroot.join("usr/lib").display()
            );
            println!(
                "cargo:rustc-link-search=native={}",
                sysroot.join("lib").display()
            );
        }
    } else if native_backend == "rpi" {
        if let Ok(sysroot) = env::var("PI_SYSROOT") {
            let sysroot = PathBuf::from(sysroot);
            println!(
                "cargo:rustc-link-search=native={}",
                sysroot.join(format!("usr/lib/{target_triplet}")).display()
            );

            // Find libcamera via pkg-config inside the sysroot without
            // mutating the global process environment.
            if let Some(lib) = probe_libcamera_in_sysroot(&sysroot, target_triplet) {
                for path in lib.include_paths {
                    println!("cargo:include={}", path.display());
                }
                for path in lib.link_paths {
                    println!("cargo:rustc-link-search=native={}", path.display());
                }
                for lib_name in lib.libs {
                    println!("cargo:rustc-link-lib=dylib={}", lib_name);
                }
                libcamera_linked = true;
            }
        } else {
            // No sysroot: fall back to host pkg-config and link against it.
            let mut config = pkg_config::Config::new();
            config.atleast_version("0.1");
            if let Ok(lib) = config.probe("libcamera") {
                for path in lib.include_paths {
                    println!("cargo:include={}", path.display());
                }
                for path in lib.link_paths {
                    println!("cargo:rustc-link-search=native={}", path.display());
                }
                for lib_name in lib.libs {
                    println!("cargo:rustc-link-lib=dylib={}", lib_name);
                }
                libcamera_linked = true;
            }
        }
    }

    // Build the C++ bridge library using CMake
    let mut cmake_config = cmake::Config::new("native-pipeline");
    cmake_config.define("CMAKE_POSITION_INDEPENDENT_CODE", "ON");
    if native_backend == "rdk-x5"
        && let Ok(sysroot) = env::var("RDK_SYSROOT")
    {
        cmake_config.define("RDK_SYSROOT", sysroot);
    }

    match native_backend.as_str() {
        "rpi" => {
            cmake_config.define("ENABLE_BACKEND_PI", "ON");
            cmake_config.define("ENABLE_BACKEND_RDK_X5", "OFF");
            cmake_config.define(
                "ENABLE_CAPTURE_LIBCAMERA",
                if has_capture_libcamera { "ON" } else { "OFF" },
            );
            cmake_config.define(
                "ENABLE_CAPTURE_V4L2",
                if has_capture_v4l2 { "ON" } else { "OFF" },
            );
            cmake_config.define(
                "ENABLE_ENCODER_V4L2_M2M",
                if has_encoder_v4l2_m2m { "ON" } else { "OFF" },
            );
            cmake_config.define("ENABLE_ENCODER_RDK_X5", "OFF");
        }
        "rdk-x5" => {
            cmake_config.define("ENABLE_BACKEND_PI", "OFF");
            cmake_config.define(
                "ENABLE_BACKEND_RDK_X5",
                if rdk_available { "ON" } else { "OFF" },
            );
            cmake_config.define("ENABLE_CAPTURE_LIBCAMERA", "OFF");
            cmake_config.define(
                "ENABLE_CAPTURE_V4L2",
                if has_capture_v4l2 { "ON" } else { "OFF" },
            );
            cmake_config.define(
                "ENABLE_ENCODER_V4L2_M2M",
                if has_encoder_v4l2_m2m { "ON" } else { "OFF" },
            );
            cmake_config.define(
                "ENABLE_ENCODER_RDK_X5",
                if rdk_available { "ON" } else { "OFF" },
            );
        }
        "generic-v4l2" => {
            cmake_config.define("ENABLE_BACKEND_PI", "OFF");
            cmake_config.define("ENABLE_BACKEND_RDK_X5", "OFF");
            cmake_config.define("ENABLE_CAPTURE_LIBCAMERA", "OFF");
            cmake_config.define(
                "ENABLE_CAPTURE_V4L2",
                if has_capture_v4l2 { "ON" } else { "OFF" },
            );
            cmake_config.define(
                "ENABLE_ENCODER_V4L2_M2M",
                if has_encoder_v4l2_m2m { "ON" } else { "OFF" },
            );
            cmake_config.define("ENABLE_ENCODER_RDK_X5", "OFF");
        }
        other => panic!(
            "unsupported native backend '{other}'. \
             Expected 'rpi', 'rdk-x5', or 'generic-v4l2'"
        ),
    }

    let dst = cmake_config.build();

    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=native-pipeline");

    // Link C++ standard library.  Prefer the compiler's default (g++ ->
    // libstdc++, clang++ -> libc++ / libstdc++ depending on platform) rather
    // than hard-coding libstdc++.
    let cpp_stdlib = detect_cpp_stdlib(&target_os);
    println!("cargo:rustc-link-lib=dylib={}", cpp_stdlib);

    // Platform-specific native libraries
    if native_backend == "rdk-x5" {
        let rdk_sysroot = env::var("RDK_SYSROOT").unwrap_or_else(|_| {
            panic!(
                "RDK_SYSROOT must be set for RDK X5 builds. \
                 Point it to the Horizon SDK sysroot."
            );
        });
        let rdk_sysroot = PathBuf::from(rdk_sysroot);
        println!(
            "cargo:rustc-link-search=native={}",
            rdk_sysroot.join("usr/hobot/lib").display()
        );
        println!(
            "cargo:rustc-link-search=native={}",
            rdk_sysroot.join("usr/lib").display()
        );
        println!(
            "cargo:rustc-link-search=native={}",
            rdk_sysroot.join("lib").display()
        );
        println!("cargo:rustc-link-lib=dylib=multimedia");
        println!("cargo:rustc-link-lib=dylib=hbmem");
        println!("cargo:rustc-link-lib=dylib=vpf");

        // By default we want link-time errors to surface missing symbols.
        // In cross-compilation scenarios where the sysroot is incomplete,
        // allow relaxed linking via LIVEHAL_RDK_ALLOW_UNDEFINED=1.
        if env::var("LIVEHAL_RDK_ALLOW_UNDEFINED").is_ok() {
            println!("cargo:rustc-link-arg=-Wl,--allow-shlib-undefined");
            println!("cargo:rustc-link-arg=-Wl,--unresolved-symbols=ignore-in-shared-libs");
        }
    } else if has_capture_libcamera && !libcamera_linked {
        println!("cargo:rustc-link-lib=dylib=camera");
        println!("cargo:rustc-link-lib=dylib=camera-base");
        println!("cargo:rustc-link-lib=dylib=event");
    }
}

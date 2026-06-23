use std::env;
use std::path::PathBuf;

fn main() {
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
        _ => "aarch64-linux-gnu",
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
    println!("cargo:rerun-if-changed=libcamera-bridge/CMakeLists.txt");

    // Core pipeline files
    println!("cargo:rerun-if-changed=libcamera-bridge/src/pipeline/source_pipeline.cpp");
    println!("cargo:rerun-if-changed=libcamera-bridge/src/pipeline/backend_factory.cpp");
    println!("cargo:rerun-if-changed=libcamera-bridge/include/source_pipeline_ffi.h");
    println!("cargo:rerun-if-changed=libcamera-bridge/include/capture_backend.h");
    println!("cargo:rerun-if-changed=libcamera-bridge/include/encoder_backend.h");
    println!("cargo:rerun-if-changed=libcamera-bridge/include/media_types.h");

    if rdk_available {
        println!("cargo:rerun-if-changed=libcamera-bridge/encoder_rdk.cpp");
    }
    if has_capture_v4l2 && rdk_available {
        println!("cargo:rerun-if-changed=libcamera-bridge/v4l2_capture_rdk.cpp");
    }
    if has_capture_libcamera {
        println!("cargo:rerun-if-changed=libcamera-bridge/camera.cpp");
    }
    if has_encoder_v4l2_m2m {
        println!("cargo:rerun-if-changed=libcamera-bridge/encoder.cpp");
    }
    if has_capture_v4l2 && !rdk_available {
        println!("cargo:rerun-if-changed=libcamera-bridge/v4l2_capture.cpp");
    }

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
            unsafe {
                env::set_var("PKG_CONFIG_ALLOW_CROSS", "1");
            }
        }
    } else if native_backend == "rpi" {
        if let Ok(sysroot) = env::var("PI_SYSROOT") {
            let sysroot = PathBuf::from(sysroot);
            let pkg_config_path = sysroot.join(format!("usr/lib/{target_triplet}/pkgconfig"));
            unsafe {
                env::set_var("PKG_CONFIG_SYSROOT_DIR", &sysroot);
                env::set_var("PKG_CONFIG_PATH", pkg_config_path);
                env::set_var("PKG_CONFIG_ALLOW_CROSS", "1");
            }
            println!(
                "cargo:rustc-link-search=native={}",
                sysroot.join(format!("usr/lib/{target_triplet}")).display()
            );
        }

        // Find libcamera via pkg-config (RPi only)
        let mut config = pkg_config::Config::new();
        config.atleast_version("0.1");
        if let Ok(lib) = config.probe("libcamera") {
            for path in lib.include_paths {
                println!("cargo:include={}", path.display());
            }
        }
    }

    // Build the C++ bridge library using CMake
    let mut cmake_config = cmake::Config::new("libcamera-bridge");
    cmake_config.define("CMAKE_POSITION_INDEPENDENT_CODE", "ON");

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
    println!("cargo:rustc-link-lib=static=cambridge");

    // Link C++ standard library
    println!("cargo:rustc-link-lib=dylib=stdc++");

    // Platform-specific native libraries
    if native_backend == "rdk-x5" {
        println!("cargo:rustc-link-search=native=/usr/hobot/lib");
        println!("cargo:rustc-link-search=native=/usr/lib");
        println!("cargo:rustc-link-lib=dylib=multimedia");
        println!("cargo:rustc-link-lib=dylib=hbmem");
        println!("cargo:rustc-link-lib=dylib=vpf");
        println!("cargo:rustc-link-arg=-Wl,--allow-shlib-undefined");
        println!("cargo:rustc-link-arg=-Wl,--unresolved-symbols=ignore-in-shared-libs");
    } else if has_capture_libcamera {
        println!("cargo:rustc-link-lib=dylib=camera");
        println!("cargo:rustc-link-lib=dylib=camera-base");
        println!("cargo:rustc-link-lib=dylib=event");
    }
}

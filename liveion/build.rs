use std::env;
use std::path::PathBuf;

fn main() {
    let has_libcamera = env::var("CARGO_FEATURE_SOURCE_LIBCAMERA").is_ok();
    let has_v4l2 = env::var("CARGO_FEATURE_SOURCE_V4L2").is_ok();
    let has_rdk = env::var("CARGO_FEATURE_BACKEND_RDK_X5").is_ok();

    // No native source features enabled — skip CMake entirely
    if !has_libcamera && !has_v4l2 && !has_rdk {
        return;
    }

    // Determine native backend explicitly — NEVER infer from TARGET
    let native_backend = if has_rdk {
        "rdk-x5".to_string()
    } else if has_libcamera {
        env::var("LIVE777_NATIVE_BACKEND").unwrap_or_else(|_| "rpi".into())
    } else if has_v4l2 {
        env::var("LIVE777_NATIVE_BACKEND").unwrap_or_else(|_| {
            panic!(
                "LIVE777_NATIVE_BACKEND must be set when using source-v4l2 without source-libcamera.\n\
                 Supported values: 'rpi', 'generic-v4l2', 'rdk-x5'"
            )
        })
    } else {
        return;
    };

    // Rerun-if-changed based on enabled features
    println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/CMakeLists.txt");

    if native_backend == "rdk-x5" {
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/encoder_rdk.cpp");
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/v4l2_capture_rdk.cpp");
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/bridge_v4l2_rdk_ffi.cpp");
    }
    if has_libcamera || native_backend == "rpi" {
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/camera.cpp");
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/encoder.cpp");
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/bridge_ffi.cpp");
    }
    if has_v4l2 {
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/v4l2_capture.cpp");
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/bridge_v4l2_ffi.cpp");
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
            let pkg_config_path = sysroot.join("usr/lib/arm-linux-gnueabihf/pkgconfig");
            unsafe {
                env::set_var("PKG_CONFIG_SYSROOT_DIR", &sysroot);
                env::set_var("PKG_CONFIG_PATH", pkg_config_path);
                env::set_var("PKG_CONFIG_ALLOW_CROSS", "1");
            }
            println!(
                "cargo:rustc-link-search=native={}",
                sysroot.join("usr/lib/arm-linux-gnueabihf").display()
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
    let mut cmake_config = cmake::Config::new("../livesrc/libcamera-bridge");
    cmake_config.define("CMAKE_POSITION_INDEPENDENT_CODE", "ON");

    match native_backend.as_str() {
        "rpi" => {
            cmake_config.define("ENABLE_BACKEND_PI", "ON");
            cmake_config.define("ENABLE_BACKEND_RDK_X5", "OFF");
            cmake_config.define("ENABLE_CAPTURE_LIBCAMERA", "ON");
            cmake_config.define("ENABLE_CAPTURE_V4L2", "ON");
            cmake_config.define("ENABLE_ENCODER_V4L2_M2M", "ON");
            cmake_config.define("ENABLE_ENCODER_RDK_X5", "OFF");
        }
        "rdk-x5" => {
            cmake_config.define("ENABLE_BACKEND_PI", "OFF");
            cmake_config.define("ENABLE_BACKEND_RDK_X5", "ON");
            cmake_config.define("ENABLE_CAPTURE_LIBCAMERA", "OFF");
            cmake_config.define("ENABLE_CAPTURE_V4L2", "ON");
            cmake_config.define("ENABLE_ENCODER_V4L2_M2M", "OFF");
            cmake_config.define("ENABLE_ENCODER_RDK_X5", "ON");
        }
        "generic-v4l2" => {
            cmake_config.define("ENABLE_BACKEND_PI", "OFF");
            cmake_config.define("ENABLE_BACKEND_RDK_X5", "OFF");
            cmake_config.define("ENABLE_CAPTURE_LIBCAMERA", "OFF");
            cmake_config.define("ENABLE_CAPTURE_V4L2", "ON");
            cmake_config.define("ENABLE_ENCODER_V4L2_M2M", "ON");
            cmake_config.define("ENABLE_ENCODER_RDK_X5", "OFF");
        }
        other => panic!(
            "unsupported LIVE777_NATIVE_BACKEND={other}. \
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
    } else {
        println!("cargo:rustc-link-lib=dylib=camera");
        println!("cargo:rustc-link-lib=dylib=camera-base");
        println!("cargo:rustc-link-lib=dylib=event");
    }
}

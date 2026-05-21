use std::env;
use std::path::PathBuf;

fn main() {
    // Only run if the source-libcamera feature is enabled
    if env::var("CARGO_FEATURE_SOURCE_LIBCAMERA").is_err() {
        return;
    }

    let target = env::var("TARGET").unwrap_or_default();
    let is_rdk = target.contains("aarch64");

    // 1. Rerun-if-changed for all platforms
    println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/bridge_ffi.h");
    if is_rdk {
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/encoder_rdk.cpp");
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/v4l2_capture_rdk.cpp");
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/bridge_v4l2_rdk_ffi.cpp");
    } else {
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/camera.cpp");
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/encoder.cpp");
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/v4l2_capture.cpp");
        println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/bridge_v4l2_ffi.cpp");
    }

    // 2. Setup Sysroot handles
    if is_rdk {
         if let Ok(sysroot) = env::var("RDK_SYSROOT") {
            let sysroot = PathBuf::from(sysroot);
            println!("cargo:rustc-link-search=native={}", sysroot.join("usr/lib").display());
            println!("cargo:rustc-link-search=native={}", sysroot.join("lib").display());
            unsafe { env::set_var("PKG_CONFIG_ALLOW_CROSS", "1"); }
         }
    } else if let Ok(sysroot) = env::var("PI_SYSROOT") {
        let sysroot = PathBuf::from(sysroot);
        let pkg_config_path = sysroot.join("usr/lib/arm-linux-gnueabihf/pkgconfig");
        unsafe {
            env::set_var("PKG_CONFIG_SYSROOT_DIR", &sysroot);
            env::set_var("PKG_CONFIG_PATH", pkg_config_path);
            env::set_var("PKG_CONFIG_ALLOW_CROSS", "1");
        }
        println!("cargo:rustc-link-search=native={}", sysroot.join("usr/lib/arm-linux-gnueabihf").display());
    }

    // 3. Find libcamera (RPi Only)
    if !is_rdk {
        let mut config = pkg_config::Config::new();
        config.atleast_version("0.1");
        if let Ok(lib) = config.probe("libcamera") {
            for path in lib.include_paths { println!("cargo:include={}", path.display()); }
        }
    }

    // 4. Build the C++ bridge library using CMake
    let mut cmake_config = cmake::Config::new("../livesrc/libcamera-bridge");
    cmake_config.define("CMAKE_POSITION_INDEPENDENT_CODE", "ON");
    
    if is_rdk {
        cmake_config.define("PLATFORM_RDK", "ON");
    }

    let dst = cmake_config.build();

    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=cambridge");

    // 5. Link CORE dependencies
    println!("cargo:rustc-link-lib=dylib=stdc++");
    if is_rdk {
        // Direct path for RDK X5 firmware libraries
        println!("cargo:rustc-link-search=native=/usr/hobot/lib");
        println!("cargo:rustc-link-search=native=/usr/lib");
        
        println!("cargo:rustc-link-lib=dylib=multimedia");
        println!("cargo:rustc-link-lib=dylib=hbmem");
        println!("cargo:rustc-link-lib=dylib=vpf");
    } else {
        println!("cargo:rustc-link-lib=dylib=camera");
        println!("cargo:rustc-link-lib=dylib=camera-base");
        println!("cargo:rustc-link-lib=dylib=event");
    }
    
    println!("cargo:rustc-link-arg=-Wl,--allow-shlib-undefined");
    println!("cargo:rustc-link-arg=-Wl,--unresolved-symbols=ignore-in-shared-libs");
}

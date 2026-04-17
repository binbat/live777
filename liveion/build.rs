use std::env;
use std::path::PathBuf;

fn main() {
    // Only run if the source-libcamera feature is enabled
    if env::var("CARGO_FEATURE_SOURCE_LIBCAMERA").is_err() {
        return;
    }

    println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/bridge_ffi.cpp");
    println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/bridge_ffi.h");
    println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/camera.cpp");
    println!("cargo:rerun-if-changed=../livesrc/libcamera-bridge/encoder.cpp");

    // 1. Setup Mini-Sysroot if path is provided
    // Users can set PI_SYSROOT to point to ~/pi-sysroot
    if let Ok(sysroot) = env::var("PI_SYSROOT") {
        let sysroot = PathBuf::from(sysroot);
        let pkg_config_path = sysroot.join("usr/lib/arm-linux-gnueabihf/pkgconfig");
        
        unsafe {
            env::set_var("PKG_CONFIG_SYSROOT_DIR", &sysroot);
            env::set_var("PKG_CONFIG_PATH", pkg_config_path);
            env::set_var("PKG_CONFIG_ALLOW_CROSS", "1");
        }
        
        // Help the linker find the libraries in sysroot
        println!("cargo:rustc-link-search=native={}", sysroot.join("usr/lib/arm-linux-gnueabihf").display());
    }

    // 2. Find libcamera and libevent using pkg-config
    let mut config = pkg_config::Config::new();
    config.atleast_version("0.1");
    
    match config.probe("libcamera") {
        Ok(lib) => {
            for path in lib.include_paths {
                println!("cargo:include={}", path.display());
            }
        }
        Err(e) => {
            // If pkg-config fails, we might still proceed if we're not cross-compiling
            // or if the user has manually set up include paths.
            println!("cargo:warning=pkg-config failed to find libcamera: {}", e);
        }
    }

    // 3. Build the C++ bridge library using CMake
    let dst = cmake::Config::new("../livesrc/libcamera-bridge")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .build();

    println!("cargo:rustc-link-search=native={}/lib", dst.display());
    println!("cargo:rustc-link-lib=static=cambridge");

    // 4. Link only CORE dependencies
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-lib=dylib=camera");
    println!("cargo:rustc-link-lib=dylib=camera-base");
    println!("cargo:rustc-link-lib=dylib=event");
    
    // THE NUCLEAR OPTION: Ignore all undefined symbols inside shared libraries
    // This is valid because we know these libraries exist on the target Raspberry Pi.
    // This stops the linker from recursive dependency checking on the host.
    println!("cargo:rustc-link-arg=-Wl,--allow-shlib-undefined");
    println!("cargo:rustc-link-arg=-Wl,--unresolved-symbols=ignore-in-shared-libs");
}

use std::env;
use std::path::PathBuf;
#[cfg(feature = "webui")]
use std::process::Command;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(riscv_mode)");

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    println!("cargo:DEBUG=build.rs: target_arch={}", target_arch);

    if target_arch.starts_with("riscv") {
        println!("cargo:DEBUG=build.rs: entering riscv mode");
        println!("cargo:rustc-cfg=riscv_mode");
        println!(
            "info: 'riscv' target detected, configuring link against libmilkv_stream.so and its dependencies."
        );

        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        let lib_dir = manifest_dir.join("dl_lib");

        if !lib_dir.exists() {
            panic!(
                "The 'dl_lib' directory does not exist at '{}'. \
                Please create it and place all required .so files inside.",
                lib_dir.display()
            );
        }

        println!("cargo:rustc-link-search=native={}", lib_dir.display());

        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");

        println!("cargo:rustc-link-lib=dylib=milkv_stream");
        println!("cargo:rustc-link-lib=dylib=sys");
        println!("cargo:rustc-link-lib=dylib=vi");
        println!("cargo:rustc-link-lib=dylib=vo");
        println!("cargo:rustc-link-lib=dylib=vpss");
        println!("cargo:rustc-link-lib=dylib=gdc");
        println!("cargo:rustc-link-lib=dylib=rgn");
        println!("cargo:rustc-link-lib=dylib=ini");
        println!("cargo:rustc-link-lib=dylib=sns_full");
        println!("cargo:rustc-link-lib=dylib=sample");
        println!("cargo:rustc-link-lib=dylib=isp");
        println!("cargo:rustc-link-lib=dylib=vdec");
        println!("cargo:rustc-link-lib=dylib=venc");
        println!("cargo:rustc-link-lib=dylib=awb");
        println!("cargo:rustc-link-lib=dylib=ae");
        println!("cargo:rustc-link-lib=dylib=af");
        println!("cargo:rustc-link-lib=dylib=cvi_bin_isp");
        println!("cargo:rustc-link-lib=dylib=cvi_bin");
        println!("cargo:rustc-link-lib=dylib=z"); // libz.so.1
        println!("cargo:rustc-link-lib=dylib=cvi_rtsp");
        println!("cargo:rustc-link-lib=dylib=misc");
        println!("cargo:rustc-link-lib=dylib=isp_algo");
        println!("cargo:rustc-link-lib=dylib=cvikernel");
        println!("cargo:rustc-link-lib=dylib=cvimath");
        println!("cargo:rustc-link-lib=dylib=cviruntime");
        println!("cargo:rustc-link-lib=dylib=opencv_core");
        println!("cargo:rustc-link-lib=dylib=opencv_imgcodecs");
        println!("cargo:rustc-link-lib=dylib=opencv_imgproc");
        println!("cargo:rustc-link-lib=dylib=cvi_ive");

        println!("cargo:rerun-if-changed=build.rs");
        println!("cargo:rerun-if-changed=dl_lib");
    } else {
        println!("cargo:DEBUG=build.rs: non-riscv, skipping milkv links");
    }

    println!("cargo:rerun-if-env-changed=TARGET_ARCH");

    #[cfg(feature = "webui")]
    {
        println!("cargo:rerun-if-changed=../web/livecam");

        let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        let web_dir = manifest_dir.parent().unwrap().join("web/livecam");
        let assets_dir = manifest_dir.join("assets/livecam");

        std::fs::create_dir_all(&assets_dir).unwrap();

        if web_dir.exists() {
            println!("cargo:info=Building WebUI...");
            let output = Command::new("npm")
                .args(["run", "build:livecam"])
                .current_dir(manifest_dir.parent().unwrap())
                .output()
                .expect("Failed to build WebUI");

            if !output.status.success() {
                panic!(
                    "WebUI build failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}

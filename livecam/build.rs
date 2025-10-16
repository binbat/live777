use reqwest::blocking::get;
use std::env;
use std::fs::File;
use std::io::Write;
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
            println!("cargo:info=dl_lib directory not found, downloading from GitHub Release...");
            download_libs_from_github(&lib_dir);
        } else {
            println!("cargo:info=dl_lib directory already exists, skipping download.");
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

fn download_libs_from_github(lib_dir: &PathBuf) {
    std::fs::create_dir_all(lib_dir).expect("Failed to create dl_lib directory");

    let url = "https://github.com/Marsyew/milkv-so-libs/releases/download/v1.0.0/dl_lib.zip";
    println!("cargo:info=Downloading .so files from {}", url);

    let response = get(url).expect("Failed to download dl_lib.zip from GitHub Release");
    let zip_path = lib_dir.join("dl_lib.zip");
    let mut file = File::create(&zip_path).expect("Failed to create dl_lib.zip");
    file.write_all(&response.bytes().expect("Failed to read response"))
        .expect("Failed to write dl_lib.zip");

    println!("cargo:info=Extracting dl_lib.zip to {}", lib_dir.display());
    let file = File::open(&zip_path).expect("Failed to open dl_lib.zip");
    let mut archive = zip::ZipArchive::new(file).expect("Failed to read zip archive");
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .expect("Failed to access file in archive");
        let file_name = file
            .name()
            .split_once('/')
            .map(|(_, name)| name)
            .unwrap_or(file.name());
        let outpath = lib_dir.join(file_name);
        if file.name().ends_with('/') {
            std::fs::create_dir_all(&outpath).expect("Failed to create directory");
        } else {
            if let Some(p) = outpath.parent()
                && !p.exists()
            {
                std::fs::create_dir_all(p).expect("Failed to create parent directory");
            }
            let mut outfile = File::create(&outpath).expect("Failed to create output file");
            std::io::copy(&mut file, &mut outfile).expect("Failed to copy file contents");
        }
    }

    std::fs::remove_file(&zip_path).expect("Failed to delete dl_lib.zip");
}

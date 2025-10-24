use std::env;

#[cfg(feature = "webui")]
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
        println!("info: 'riscv' target detected, milkv-libs will handle library linking.");
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

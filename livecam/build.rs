use std::env;

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
}

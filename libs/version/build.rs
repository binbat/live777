fn main() {
    println!("cargo:rerun-if-env-changed=LIVE777_VERSION");
    shadow_rs::ShadowBuilder::builder().build().unwrap();
}

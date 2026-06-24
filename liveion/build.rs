fn main() {
    // Native compilation (native-pipeline) is handled by
    // livehal's build.rs when capture-* features are enabled through
    // the native-* presets. liveion no longer builds C++ directly.
    println!("cargo:rerun-if-changed=../assets/liveion");
}

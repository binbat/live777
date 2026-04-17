fn main() {
    // Native compilation (libcamera-bridge / cambridge) is handled by
    // livesrc's build.rs when capture-* features are enabled through
    // the native-* presets. liveion no longer builds C++ directly.
    println!("cargo:rerun-if-changed=../assets/liveion");
}

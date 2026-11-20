fn main() {
    // The frontend is embedded into the binary at compile time; rebuild when it changes.
    println!("cargo:rerun-if-changed=../ui");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities");
    tauri_build::build()
}

fn main() {
    // Only the desktop shell has a Tauri side to build. The library — which is what
    // the CLI's `--compare` mode links — compiles with the `desktop` feature off and
    // never sees `tauri.conf.json`.
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        tauri_build::build()
    }
}

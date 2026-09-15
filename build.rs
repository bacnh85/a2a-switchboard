use std::path::Path;

fn main() {
    // rust-embed resolves ui/dist at compile time (release) and runtime
    // (debug). Stub it when the console hasn't been built yet so
    // `cargo build` / `cargo test` work without a Node toolchain.
    let dist = Path::new("ui/dist");
    if !dist.exists() {
        std::fs::create_dir_all(dist).expect("create ui/dist stub dir");
        std::fs::write(
            dist.join("index.html"),
            "<!doctype html><title>a2a-switchboard</title><p>Console UI not built yet — run \
             <code>npm --prefix ui ci &amp;&amp; npm --prefix ui run build</code>.</p>",
        )
        .expect("write ui/dist stub");
    }
    println!("cargo:rerun-if-changed=ui/dist");
    println!("cargo:rerun-if-changed=build.rs");
}

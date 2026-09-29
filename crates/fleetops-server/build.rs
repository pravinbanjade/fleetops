//! rust-embed needs web/dist to exist at compile time. Until the web UI is
//! built (plan P3), write a placeholder page so the server still compiles.
use std::{fs, path::PathBuf};

const PLACEHOLDER: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>FleetOps</title></head>\
<body><h1>FleetOps</h1><p>The web UI has not been built yet. The API is available under <code>/api/v1</code>.</p></body></html>\n";

fn main() {
    let dist = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../../web/dist");
    if !dist.join("index.html").exists() {
        fs::create_dir_all(&dist).expect("create web/dist");
        fs::write(dist.join("index.html"), PLACEHOLDER).expect("write placeholder index.html");
    }
    println!("cargo:rerun-if-changed=../../web/dist");
}

// Generates Rust types from the .proto sources at compile time.
// Output lands in $OUT_DIR/meeting_companion.v1.rs and is `include!`'d
// from src/lib.rs — no committed generated code in the Rust tree.
//
// Requires `protoc` available on PATH at build time. CI installs it
// via `apt-get install -y protobuf-compiler` on Linux runners; locally
// `brew install protobuf` does the job.

use std::path::PathBuf;

fn main() {
    let proto_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("proto");

    let protos = [
        proto_root.join("meeting_companion/v1/common.proto"),
        proto_root.join("meeting_companion/v1/intents.proto"),
        proto_root.join("meeting_companion/v1/events.proto"),
    ];

    for p in &protos {
        println!("cargo:rerun-if-changed={}", p.display());
    }

    let mut config = prost_build::Config::new();

    // Wire the `serde` feature through to the generated types so the
    // server's REST handlers can serialize them as JSON without
    // maintaining a parallel hand-written shape. The cfg_attr keeps
    // the generated code free of an unconditional serde dep — clients
    // who only care about prost don't pay.
    if cfg!(feature = "serde") {
        config.type_attribute(
            ".meeting_companion.v1",
            "#[derive(::serde::Serialize, ::serde::Deserialize)]\
             #[serde(rename_all = \"snake_case\")]",
        );
    }

    config
        .compile_protos(&protos, &[proto_root])
        .expect("prost-build failed; is protoc on PATH?");
}

use std::io::Result;

// The two protoc sources are mutually exclusive.
// Checking this with `cfg` rather than a runtime assertion
// turns the misconfiguration into a compile error that names the problem.
#[cfg(all(feature = "protoc-from-src", feature = "protoc-vendored"))]
compile_error!(
    "It looks like you've enabled both protoc-from-src and protoc-vendored at the same time. You probably want to pick just one.\n\n(Hint: did you forget to add default-features = false in your Cargo.toml?)"
);

fn main() -> Result<()> {
    println!("cargo:rerun-if-env-changed=PROTOC");
    // Watch the whole directory so that changes to imported files trigger a rebuild too.
    println!("cargo:rerun-if-changed=proto");
    let mut config = prost_build::Config::new();

    #[cfg(feature = "protoc-from-src")]
    config.protoc_executable(protobuf_src::protoc());

    #[cfg(feature = "protoc-vendored")]
    match protoc_bin_vendored::protoc_bin_path() {
        Ok(path) => {
            config.protoc_executable(path);
        }
        Err(err) => println!(
            "cargo:warning=vendored protoc unavailable ({err}); using PROTOC or protoc on PATH"
        ),
    }
    config.compile_protos(&["proto/gtfs-realtime.proto"], &["proto/"])?;
    Ok(())
}

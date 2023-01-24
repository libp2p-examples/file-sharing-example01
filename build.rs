fn main() {
    println!("cargo:rerun-if-changed=src/abi.proto");

    prost_build::Config::new()
        .out_dir("src/network")
        .compile_protos(&["src/abi.proto"], &["src"])
        .unwrap();
}

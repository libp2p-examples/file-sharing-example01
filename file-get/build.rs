fn main() {
    println!("cargo:rerun-if-changed=src/file-get.proto");

    prost_build::Config::new()
    .out_dir("src")
        .compile_protos(&["src/file-get.proto"], &["src"])
        .unwrap()
}
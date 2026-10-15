fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=../../proto/sapwood.proto");
    println!("cargo:rerun-if-changed=../../proto/sapwood_objects.proto");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    tonic_prost_build::configure()
        .file_descriptor_set_path(out.join("sapwood_descriptor.bin"))
        .compile_protos(&["../../proto/sapwood.proto", "../../proto/sapwood_objects.proto"], &["../../proto"])?;
    Ok(())
}

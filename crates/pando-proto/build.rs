fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/pando.proto");
    println!("cargo:rerun-if-changed=proto/pando_objects.proto");
    println!("cargo:rerun-if-changed=proto/pando_platform.proto");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR")?);
    tonic_prost_build::configure().file_descriptor_set_path(out.join("pando_descriptor.bin")).compile_protos(
        &["proto/pando.proto", "proto/pando_objects.proto", "proto/pando_platform.proto"],
        &["proto"],
    )?;
    Ok(())
}

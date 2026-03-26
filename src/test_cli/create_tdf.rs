use anyhow::{Context, Result};
use std::path::Path;

pub fn create_tdf_bytes(attribute_fqn: &str, data: &[u8]) -> Result<Vec<u8>> {
    use opentdf::prelude::*;

    let mut builder = PolicyBuilder::new()
        .id_auto()
        .dissemination(["test@tdf-iroh-s3"]);

    if !attribute_fqn.is_empty() {
        builder = builder
            .attribute_fqn(attribute_fqn)
            .context("Invalid attribute FQN")?;
    }

    let policy = builder.build().context("Failed to build policy")?;

    let tdf_bytes = Tdf::encrypt(data)
        .kas_url("https://kas.example.com")
        .policy(policy)
        .to_bytes()
        .context("Failed to create TDF")?;

    Ok(tdf_bytes)
}

pub fn create_tdf_file(attribute_fqn: &str, data: &[u8], output_path: &Path) -> Result<()> {
    let tdf_bytes = create_tdf_bytes(attribute_fqn, data)?;
    std::fs::write(output_path, &tdf_bytes)
        .with_context(|| format!("Failed to write TDF to {}", output_path.display()))?;

    let hash = blake3::hash(&tdf_bytes);
    println!("Created TDF: {}", output_path.display());
    println!("BLAKE3 hash: {}", hash.to_hex());
    println!("Size: {} bytes", tdf_bytes.len());

    Ok(())
}

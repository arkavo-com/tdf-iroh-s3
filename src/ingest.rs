use anyhow::{Context, Result};
use bytes::Bytes;
use tracing::info;

use crate::config::ValidationConfig;
use crate::store::s3::S3Client;
use crate::validation;

/// Result of a successful ingest operation.
pub struct IngestResult {
    /// BLAKE3 hash of the blob (hex-encoded).
    pub hash_hex: String,
    /// Size of the blob in bytes.
    pub size: u64,
}

/// Ingest a blob: validate it as a TDF, then store it in S3.
pub async fn ingest_blob(
    data: &[u8],
    validation_config: &ValidationConfig,
    s3_client: &S3Client,
) -> Result<IngestResult> {
    let size = data.len() as u64;

    // Step 1: Validate through TDF pipeline
    validation::validate_blob(data, validation_config)
        .context("Blob rejected by TDF validation")?;

    // Step 2: Compute BLAKE3 hash
    let hash = blake3::hash(data);
    let hash_hex = hash.to_hex().to_string();

    // Step 3: Check if already stored
    if s3_client.has_blob(&hash_hex).await? {
        info!(hash = %hash_hex, "Blob already exists in S3, skipping upload");
        return Ok(IngestResult { hash_hex, size });
    }

    // Step 4: Upload blob to S3
    s3_client
        .put_blob(&hash_hex, Bytes::copy_from_slice(data))
        .await
        .context("Failed to upload blob to S3")?;

    info!(hash = %hash_hex, size, "Blob ingested and stored to S3");
    Ok(IngestResult { hash_hex, size })
}

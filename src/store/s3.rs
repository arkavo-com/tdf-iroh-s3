use anyhow::{Context, Result};
use aws_sdk_s3::Client;
use bytes::Bytes;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub struct S3Client {
    client: Client,
    bucket: String,
    prefix: String,
}

impl S3Client {
    /// Create a new S3 client using the default AWS credential chain (IAM role on EC2).
    pub async fn new(bucket: &str, region: &str, prefix: &str) -> Result<Self> {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new(region.to_string()))
            .load()
            .await;
        let client = Client::new(&config);
        Ok(Self {
            client,
            bucket: bucket.to_string(),
            prefix: prefix.to_string(),
        })
    }

    /// Create a mock S3 client for testing (no real AWS calls).
    pub fn new_mock(bucket: &str, region: &str, prefix: &str) -> Self {
        let config = aws_sdk_s3::config::Builder::new()
            .region(aws_sdk_s3::config::Region::new(region.to_string()))
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .build();
        let client = Client::from_conf(config);
        Self {
            client,
            bucket: bucket.to_string(),
            prefix: prefix.to_string(),
        }
    }

    /// The configured key prefix (e.g. `"env/prod/"`).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn blob_key(&self, hash_hex: &str) -> String {
        format!("{}blobs/{}", self.prefix, hash_hex)
    }

    pub fn outboard_key(&self, hash_hex: &str) -> String {
        format!("{}outboards/{}", self.prefix, hash_hex)
    }

    pub fn tag_key(&self, tag_name: &str) -> String {
        format!("{}tags/{}", self.prefix, tag_name)
    }

    pub async fn put_blob(&self, hash_hex: &str, data: Bytes) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.blob_key(hash_hex))
            .body(data.into())
            .send()
            .await
            .context("Failed to PUT blob to S3")?;
        Ok(())
    }

    pub async fn put_outboard(&self, hash_hex: &str, data: Bytes) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.outboard_key(hash_hex))
            .body(data.into())
            .send()
            .await
            .context("Failed to PUT outboard to S3")?;
        Ok(())
    }

    pub async fn get_blob(&self, hash_hex: &str) -> Result<Bytes> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.blob_key(hash_hex))
            .send()
            .await
            .context("Failed to GET blob from S3")?;
        let data = resp
            .body
            .collect()
            .await
            .context("Failed to read blob body from S3")?;
        Ok(data.into_bytes())
    }

    pub async fn get_outboard(&self, hash_hex: &str) -> Result<Bytes> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.outboard_key(hash_hex))
            .send()
            .await
            .context("Failed to GET outboard from S3")?;
        let data = resp
            .body
            .collect()
            .await
            .context("Failed to read outboard body from S3")?;
        Ok(data.into_bytes())
    }

    pub async fn has_blob(&self, hash_hex: &str) -> Result<bool> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(self.blob_key(hash_hex))
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(e) => {
                if e.as_service_error()
                    .is_some_and(|se| se.is_not_found())
                {
                    Ok(false)
                } else {
                    Err(anyhow::anyhow!("Failed to HEAD blob in S3: {}", e))
                }
            }
        }
    }

    pub async fn delete_blob(&self, hash_hex: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(self.blob_key(hash_hex))
            .send()
            .await
            .context("Failed to DELETE blob from S3")?;
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(self.outboard_key(hash_hex))
            .send()
            .await
            .context("Failed to DELETE outboard from S3")?;
        Ok(())
    }

    pub async fn put_tag(&self, tag_name: &str, hash_hex: &str) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(self.tag_key(tag_name))
            .body(Bytes::from(hash_hex.to_string()).into())
            .send()
            .await
            .context("Failed to PUT tag to S3")?;
        Ok(())
    }

    pub async fn get_tag(&self, tag_name: &str) -> Result<Option<String>> {
        match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(self.tag_key(tag_name))
            .send()
            .await
        {
            Ok(resp) => {
                let data = resp.body.collect().await?;
                let hash_hex = String::from_utf8(data.into_bytes().to_vec())?;
                Ok(Some(hash_hex))
            }
            Err(e) => {
                if e.as_service_error()
                    .is_some_and(|se| se.is_no_such_key())
                {
                    Ok(None)
                } else {
                    Err(anyhow::anyhow!("Failed to GET tag from S3: {}", e))
                }
            }
        }
    }

    pub async fn delete_tag(&self, tag_name: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(self.tag_key(tag_name))
            .send()
            .await
            .context("Failed to DELETE tag from S3")?;
        Ok(())
    }

    /// PUT raw bytes at an arbitrary key (i.e. not the BLAKE3-keyed blob path).
    pub async fn put_object_bytes(&self, key: &str, data: Bytes) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(data.into())
            .send()
            .await
            .with_context(|| format!("Failed to PUT object '{key}' to S3"))?;
        Ok(())
    }

    /// HEAD a generic key. Returns `true` if the object exists, `false` for 404.
    pub async fn head_object(&self, key: &str) -> Result<bool> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(e) => {
                if e.as_service_error().is_some_and(|se| se.is_not_found()) {
                    Ok(false)
                } else {
                    Err(anyhow::anyhow!("Failed to HEAD object '{key}' in S3: {e}"))
                }
            }
        }
    }

    /// GET a generic key. Returns `None` for 404.
    pub async fn get_object_bytes(&self, key: &str) -> Result<Option<Bytes>> {
        match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(resp) => {
                let data = resp
                    .body
                    .collect()
                    .await
                    .with_context(|| format!("Failed to read object '{key}' body"))?;
                Ok(Some(data.into_bytes()))
            }
            Err(e) => {
                if e.as_service_error().is_some_and(|se| se.is_no_such_key()) {
                    Ok(None)
                } else {
                    Err(anyhow::anyhow!("Failed to GET object '{key}' from S3: {e}"))
                }
            }
        }
    }

    /// PUT a value serialized as pretty JSON with content-type `application/json`.
    pub async fn put_json<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let body = serde_json::to_vec_pretty(value)
            .with_context(|| format!("Failed to serialize JSON for key '{key}'"))?;
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type("application/json")
            .body(Bytes::from(body).into())
            .send()
            .await
            .with_context(|| format!("Failed to PUT JSON '{key}' to S3"))?;
        Ok(())
    }

    /// GET a JSON value. Returns `None` if the object does not exist.
    pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let Some(bytes) = self.get_object_bytes(key).await? else {
            return Ok(None);
        };
        let parsed: T = serde_json::from_slice(&bytes)
            .with_context(|| format!("Failed to parse JSON at key '{key}'"))?;
        Ok(Some(parsed))
    }

    /// List object keys with the given prefix. Pages internally.
    pub async fn list_keys(&self, prefix: &str) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            let req = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix);
            let req = match &continuation {
                Some(token) => req.continuation_token(token),
                None => req,
            };
            let resp = req
                .send()
                .await
                .with_context(|| format!("Failed to LIST objects under '{prefix}'"))?;
            if let Some(objects) = resp.contents {
                for obj in objects {
                    if let Some(k) = obj.key {
                        keys.push(k);
                    }
                }
            }
            if resp.is_truncated.unwrap_or(false) {
                continuation = resp.next_continuation_token;
            } else {
                break;
            }
        }
        Ok(keys)
    }
}

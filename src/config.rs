use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub iroh: IrohConfig,
    pub s3: S3Config,
    #[serde(default)]
    pub validation: ValidationConfig,
    #[serde(default)]
    pub catalog: CatalogConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Deserialize)]
pub struct IrohConfig {
    #[serde(default = "default_bind_port")]
    pub bind_port: u16,
    #[serde(default = "default_secret_key_param")]
    pub secret_key_param: String,
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
}

impl Default for IrohConfig {
    fn default() -> Self {
        Self {
            bind_port: default_bind_port(),
            secret_key_param: default_secret_key_param(),
            data_dir: default_data_dir(),
        }
    }
}

fn default_bind_port() -> u16 {
    11204
}

fn default_secret_key_param() -> String {
    "/tdf-iroh-s3/node-secret-key".to_string()
}

fn default_data_dir() -> String {
    "/var/lib/tdf-iroh-s3/data".to_string()
}

#[derive(Debug, Deserialize)]
pub struct S3Config {
    pub bucket: String,
    pub region: String,
    #[serde(default)]
    pub prefix: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct ValidationConfig {
    #[serde(default)]
    pub required_attributes: Vec<String>,
    #[serde(default)]
    pub assertion: AssertionConfig,
}

#[derive(Debug, Default, Deserialize)]
pub struct AssertionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub trusted_public_keys: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CatalogConfig {
    #[serde(default = "default_catalog_data_dir")]
    pub data_dir: String,
}

impl Default for CatalogConfig {
    fn default() -> Self {
        Self {
            data_dir: default_catalog_data_dir(),
        }
    }
}

fn default_catalog_data_dir() -> String {
    "/var/lib/tdf-iroh-s3/docs".to_string()
}

#[derive(Debug, Deserialize, Clone)]
pub struct AuthConfig {
    /// URL of the COSE_KeySet endpoint (`application/cose-key-set+cbor`).
    /// For arkavo: `https://identity.arkavo.net/.well-known/cose-keys`.
    pub cose_keys_url: String,
    pub issuer: String,
    #[serde(default = "default_refresh_interval_secs")]
    pub refresh_interval_secs: u64,
    #[serde(default = "default_clock_skew_secs")]
    pub clock_skew_secs: i64,
}

fn default_refresh_interval_secs() -> u64 {
    300
}

fn default_clock_skew_secs() -> i64 {
    60
}

impl Config {
    pub fn from_file(path: &PathBuf) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }
}

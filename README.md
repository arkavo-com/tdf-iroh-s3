# tdf-iroh-s3

A persistent Iroh peer node that validates incoming blobs as OpenTDF files
and stores them in Amazon S3. Serves stored blobs back to any requesting peer.

## How It Works

1. Peers send blobs over Iroh's P2P QUIC protocol
2. Each blob is validated as a valid TDF (Trusted Data Format):
   - ZIP structure with manifest.json and payload
   - Required attributes present in the policy
   - Optional assertion signature verification
3. Valid blobs are stored in S3, keyed by BLAKE3 content hash
4. Any peer can retrieve blobs by hash — the TDF encryption layer handles access control

## Build

```bash
cargo build --release
```

## Configuration

Create a config file (TOML):

```toml
[iroh]
bind_port = 11204

[s3]
bucket = "my-tdf-store"
region = "us-east-1"

[validation]
required_attributes = [
    "https://example.com/attr/storage/value/permanent"
]

[validation.assertion]
enabled = false
trusted_public_keys = []
```

## Run

```bash
./target/release/tdf-iroh-s3 --config config.toml
```

## Deploy (AMI)

Build the AMI with Packer:

```bash
cargo build --release
cd packer
packer build -var "binary_path=../target/release/tdf-iroh-s3" ami.pkr.hcl
```

Launch an EC2 instance from the AMI, passing config as user-data.

## Test

```bash
cargo test
```

# iroh-docs Catalog with CWT-Gated Writes

## Problem

Two coupled changes to the catalog layer:

1. **Storage substrate.** The publish event log currently lives in S3 as one
   JSON object per event (`creators/{creator_id}/events/{seq:020}.json`).
   That's a private, single-writer log: any peer that wants to learn what a
   creator has published must talk to *this* node and read its S3 bucket.
   The catalog is supposed to be a shared, replicated artifact across peer
   nodes, so the event log needs to move into a substrate that syncs natively
   over iroh — `iroh-docs` is the obvious fit.

2. **Write authorization.** Today there is no authorization on the publish
   path at all: anyone who can reach the node's API can append an event for
   any `creator_id`. We need writes to be gated by a CWT (COSE_Sign1, RFC
   8392) signed by an issuer whose verification key comes from a
   **COSE_KeySet endpoint** (`application/cose-key-set+cbor`) configured on
   the node. The CWT's claims bind the event to a specific creator and
   campaign. The reference issuer is `https://identity.arkavo.net`, whose
   discovery document advertises this endpoint as `arkavo_cose_keys_uri`.

Blob push/get over `iroh-blobs` is **out of scope** for CWT gating — that
layer keeps its current NodeId-based access pattern through the existing
`ProviderMessage` hooks.

## Canonical model

**The event log is the catalog.** Each event in the iroh-docs replica is
the authoritative record of one publish, authenticated by the CWT
embedded in it. Anything else that calls itself a "catalog" — the
in-memory `Catalog` struct returned by `build_catalog`, a JSON blob
served over HTTP, a sorted slice handed to a UI — is a *projection* of
the event log: derived at read time, disposable, never written back, and
not required to be byte-identical between callers.

Consequences for this spec:

- No `version`, `generated_at`, or `signature` on the projection. Per-event
  authenticity is already provided by the CWT in the event payload; a
  signature over a derived view adds no security and invites confusion
  about which form is canonical.
- No catalog snapshot writes. The replica is the single source of truth.
- `regenerate_catalog` is renamed `project_catalog` to make
  disposability obvious in the code.
- Two callers asking for "the catalog" at the same replica state are
  free to return different shapes (e.g., filtered by visibility, sorted
  by title rather than `published_at`) without disagreeing about
  canonical state.

## Design

### Catalog in iroh-docs

Add a single, node-local **catalog replica** (an `iroh-docs` document) that
holds every publish event the node has authorized. The replica replaces the
S3 event log; the per-creator `latest.json` / snapshot files in S3 are
removed because the catalog is now *derived on demand* from the replica
contents — there's nothing externally published.

**Key layout inside the replica.** Mirror the existing S3 key shape so the
build/dedup logic in `crate::catalog::build_catalog` is reused unchanged:

```
creators/{creator_id}/events/{seq:020}
```

The value at each key is the canonical JSON of a `PublishEvent`. `seq` is
allocated by listing the existing keys under `creators/{creator_id}/events/`
in the replica, taking `max(seq) + 1`, and retrying on author-collision —
same conditional-write pattern as today, but using `iroh-docs`' optimistic
write semantics instead of S3 `If-None-Match`.

**Replica identity.** The replica's `NamespaceId` is generated once on first
boot, stored as a parameter alongside the existing node secret (SSM in
production, file in dev), and reused on subsequent boots. The node
publishes the replica's `NamespaceId` and capability ticket via a small
read-only iroh protocol (`tdf-iroh-s3/catalog/v1`) so peers can subscribe.

**Authoring.** The node owns a single iroh-docs `AuthorId`, also persisted
alongside the node secret. Every event written to the replica is signed by
this author. The CWT that authorized the write is embedded *inside the
event payload* (see `PublishEvent.authorization` below) so the chain of
custody is auditable from the replica alone — a verifier doesn't need to
trust the node's author key, only the issuer's JWKS.

**Catalog projection.** `crate::catalog::build_catalog` keeps its
dedup/sort semantics but its return type loses `version`,
`generated_at`, and `signature`. A new
`project_catalog(replica, creator_id) -> CatalogView` reads all entries
under `creators/{creator_id}/events/`, deserializes them into
`PublishEvent`, calls `build_catalog`, and returns the in-memory
`CatalogView { creator_id, entries }`. The result is never written back
to the replica and has no on-the-wire identity — callers that need a
versionable handle should reference the event log directly (e.g. by max
seq observed).

### CWT verifier

A new `crate::auth` module verifies COSE_Sign1 CWTs against a
COSE_KeySet fetched from a configured URL. The wire format is CBOR
end-to-end — no JSON or base64url anywhere in the auth path.

**Token shape.** The verifier requires:

- COSE_Sign1 protected header: `alg = ES256` (only algorithm accepted in
  this iteration).
- Standard CWT claims: `iss` (must match config), `sub` (= `creator_id`),
  `iat`, `exp` (must be in the future), `cti` (random nonce, logged for
  audit but not currently checked against a replay cache).
- Custom claims: `campaign_id` (string), `scope` (string, must contain
  `"catalog.write"`).
- Optional confirmation claim `cnf.iroh_node_id` (hex NodeId) — if present,
  the verifier requires the CWT to have been presented over a connection
  whose remote NodeId matches. If absent, the CWT is bearer-style.

**Key cache.** One `cose_keys_url` in `[auth]` config. The verifier holds
the parsed key set (`kid: Vec<u8> → p256::ecdsa::VerifyingKey`) behind an
`ArcSwap` and refreshes on a tokio interval (default 300s) plus on first
cache miss (`kid` not found triggers an immediate refetch with a 1s
minimum gap to avoid stampedes). Parser accepts only `kty = EC2`,
`crv = P-256`; other shapes are dropped at parse time with a warn log.

**API.**

```rust
pub struct Verifier { /* JWKS cache, issuer, clock */ }

pub struct VerifiedClaims {
    pub creator_id: String,
    pub campaign_id: String,
    pub raw_cwt: Bytes,        // for embedding in PublishEvent.authorization
    pub cti: String,
    pub exp: i64,
}

impl Verifier {
    pub fn verify(&self, cwt: &[u8], bound_node_id: Option<NodeId>) -> Result<VerifiedClaims>;
}
```

### Wiring the gate

`publish_content` becomes:

```rust
pub async fn publish_content(
    metadata: ContentMetadata,
    payload: Bytes,
    auth: &VerifiedClaims,        // <-- replaces the bare creator_id arg
    replica: &CatalogReplica,
    s3: &S3Client,
) -> Result<PublishOutcome>;
```

The S3 client is still used for the **payload** and the per-content
manifest (those stay in S3 as before — see "Out of Scope" below). What
moves is the *event log* and the *catalog snapshot* writes.

The publish RPC entry point (currently only invoked from
`test_cli/push.rs` — there is no network-facing publish handler yet) gains
a CWT argument; the test CLI mints a test CWT with a configurable signing
key fixture. Production callers obtain CWTs out of band from the issuer.

### Event payload change

`PublishEvent` gains:

```rust
pub struct PublishEvent {
    // ... existing fields ...
    pub authorization: EventAuthorization,
}

pub struct EventAuthorization {
    pub cwt_b64: String,          // base64 of the raw CWT bytes
    pub issuer: String,           // copy of CWT iss claim, for index/filter
    pub cti: String,              // copy of CWT cti, for replay analysis
}
```

The catalog projection (`CatalogEntry`) does **not** include the CWT —
that's audit-layer data, not catalog-consumer data.

### Config

```toml
[catalog]
data_dir = "/var/lib/tdf-iroh-s3/docs"   # iroh-docs storage

[auth]
cose_keys_url = "https://identity.arkavo.net/.well-known/cose-keys"
issuer        = "https://identity.arkavo.net"
refresh_interval_secs = 300
# Optional, defaults to 60 seconds in either direction.
clock_skew_secs = 60
```

### Migration

There is no production data yet (the per-creator catalog feature shipped in
`9a6b589` is unreleased), so no S3→docs backfill is needed. The S3 keys
under `creators/{creator_id}/events/` and `creators/{creator_id}/catalog/`
stop being written; existing keys are left untouched for forensic value
and can be cleaned up later.

## Files Changed

| File | Change |
|------|--------|
| `Cargo.toml` | Add `iroh-docs`, `coset`, `ciborium`, `p256`, `arc-swap`, `reqwest`, `base64` |
| `src/config.rs` | Add `CatalogConfig` and `AuthConfig` sections |
| `src/auth/mod.rs` | New module |
| `src/auth/cwt.rs` | COSE_Sign1 parse + verify |
| `src/auth/cose_keys.rs` | COSE_KeySet fetch (CBOR), parse, cache, refresh |
| `src/auth/test_signer.rs` (cfg(test)) | In-test issuer for unit + integration tests |
| `src/catalog/mod.rs` | `EventAuthorization` field; `build_catalog` unchanged |
| `src/catalog/types.rs` | Add `EventAuthorization`; replace `Catalog` (versioned + signed) with `CatalogView` (entries only); keep `CatalogEntry` wire-compatible |
| `src/catalog/replica.rs` | New: `CatalogReplica` wrapper around `iroh-docs` doc |
| `src/catalog/publish.rs` | Rewrite: take `VerifiedClaims`, write to replica instead of S3 event log |
| `src/catalog/keys.rs` | Replica key helpers (drop S3-specific catalog snapshot keys) |
| `src/node.rs` | Load/persist `NamespaceId` + `AuthorId`; open replica; hold `Verifier`; register `tdf-iroh-s3/catalog/v1` ALPN |
| `src/secret_key.rs` | Generalize parameter store helper to cover author key + namespace id |
| `src/test_cli/push.rs` | Mint test CWT, pass to publish |
| `tests/catalog_event_log_test.rs` | Rewrite for replica instead of S3 |
| `tests/auth_cwt_test.rs` | New: end-to-end verify path with a test issuer |
| `tests/catalog_publish_auth_test.rs` | New: publish is rejected without CWT, accepted with valid CWT, rejected with wrong-creator CWT |

## Out of Scope

- **CWT on blob push / get.** The `iroh-blobs` `ProviderMessage` hooks
  remain the only gate on the blob protocol; this spec does not change
  them. Anyone who knows a blob hash and can reach the node can still
  fetch the blob (access control is the TDF encryption layer's job, per
  `CLAUDE.md`).
- **Multi-issuer / multi-key-endpoint.** Single issuer, single
  COSE_KeySet URL.
- **Replay cache for `cti`.** The verifier logs `cti` but does not store
  it. Replay protection is left to short `exp` lifetimes and `cnf`
  binding.
- **Cross-node replica sync UX.** This spec specifies *how* a peer can
  subscribe (the `tdf-iroh-s3/catalog/v1` ALPN serves the
  `NamespaceId` + ticket) but does not specify the peer-side subscription
  loop — that's a future plan.
- **Backfill of pre-existing S3 events.** None exist in production.
- **Removing the S3 catalog snapshot writes from the codebase
  completely.** They go away in `publish.rs` rewrite, but the S3 key
  helpers in `keys.rs` are kept (still used by the payload + per-content
  manifest paths).
- **Signing the projection.** Not needed — the event log is canonical
  and each event is CWT-authenticated. The placeholder
  `CatalogSignature` / `CatalogDraft::finalize` machinery is removed
  along with `Catalog.version` and `Catalog.generated_at`.

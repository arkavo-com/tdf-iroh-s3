# Reader-side CWT + redb event log

**Status:** approved 2026-05-26
**Supersedes:** [`2026-05-26-iroh-docs-catalog-and-cwt.md`](2026-05-26-iroh-docs-catalog-and-cwt.md)
**Branch:** `claude/iroh-arkavo-dev-plan-9obEF`
**Authoritative CWT contract:** Arkavo CWT — Read Token Contract (v1), reproduced as Appendix A.

## Why

The current branch shipped a CWT/COSE verifier (`src/auth/`), an iroh-docs catalog
replica (`src/catalog/`), and a `publish_content` write path. A code review
found:

- The CWT was framed as a **write-side** gate (publisher → node), but the
  ingest path validates TDFs via attributes/assertions and needs no external
  authorization. The node IS the writer.
- `publish_content` and `verifier` had zero production call sites.
- The iroh-docs/iroh-gossip ALPNs were registered without ACLs, exposing the
  event log (with verbatim CWT bytes) to any peer.
- Multiple correctness bugs in the bespoke CWT verifier (time-handling,
  signature-malleability surface, kid/alg confusion) that the platform's own
  `pep_check` reference implementation does not have.

The right model is **reader-side CWT**: the node ingests TDFs on its own
authority, appends a content event to a local log, and gates *reads* of that
log via a CWT-authenticated `tdf/catalog/1` ALPN. Filtering uses
`opentdf::pdp::AccessPdp` driven by `authorization_details` claims per the
Arkavo CWT v1 contract.

Because v1 is single-author (no federation), the event log needs no
replication substrate. iroh-docs and iroh-gossip are removed; the log lives in
a local `redb` database.

## Non-goals (v1)

- **Multi-node writer federation.** Acknowledged in code; resolved in v2.
- **Write-side CWT gating.** TDF attribute/assertion validation already
  authorizes ingest. No external publisher API in v1.
- **Long-lived CWT refresh on a live stream.** Server drops on `exp`, reader
  reconnects with `after_seq` cursor.
- **Audit-log sink.** Tracing/journald only — operators ship logs via the
  existing systemd path.

## Architecture

```
INGEST (existing path + small addition)
  peer pushes TDF blob via iroh-blobs ALPN
    → BlobsProtocol → FsStore
    → wait_and_ingest:
        validate (structure/attrs/assertion)              [existing]
        BLAKE3 + S3 upload                                [existing]
        extract attribute-value FQNs from manifest        [new]
        EventStore::append(ContentEvent)                  [new — node's own writer]

READ (new)
  reader opens stream on tdf/catalog/1 ALPN
    → sends CatalogSubscribe { cwt, after_seq? }
    → node verifies CWT (pep_check) binding cnf.iroh_node_id to peer NodeId
    → derives Entitlements from authorization_details
    → backfill phase: EventStore::list_from(after_seq) → AccessPdp::check → Entry
    → live phase: EventStore::subscribe() (broadcast receiver) → AccessPdp::check → Entry
    → heartbeat every 30s
    → emit TokenExpiringSoon at exp-60s, then Error+close at exp
```

The CatalogReplica + iroh-docs Doc/Gossip are gone. EventStore is a redb
wrapper.

## Components

### `src/catalog/store.rs` — EventStore

```rust
pub struct EventStore {
    db: Arc<redb::Database>,
    tx: tokio::sync::broadcast::Sender<ContentEvent>,
}

impl EventStore {
    pub async fn open(path: &Path) -> Result<Self>;
    pub async fn append(&self, e: NewContentEvent) -> Result<ContentEvent>;
    pub async fn list_from(&self, after_seq: u64) -> Result<impl Stream<Item = Result<ContentEvent>>>;
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ContentEvent>;
    pub fn current_tail(&self) -> u64;
}
```

- Single redb table `TableDefinition::<u64, &[u8]>::new("events_v1")`; value
  is canonical CBOR of `ContentEvent`.
- A second table `TableDefinition::<&str, u64>::new("meta_v1")` holds
  `("last_seq", u64)` so `current_tail` and `append` don't scan.
- `append` runs in `spawn_blocking`; takes a write transaction, increments
  `meta_v1["last_seq"]`, writes the event, commits, then broadcasts.
- Broadcast channel capacity: 1024 (slow subscribers receive `RecvError::Lagged`
  and the ALPN handler closes their stream cleanly).

### `src/catalog/types.rs` — ContentEvent

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentEvent {
    pub seq: u64,
    pub content_id: String,                  // BLAKE3 hex
    pub manifest_ref: String,                // S3 key
    pub attribute_value_fqns: Vec<String>,   // denormalized from TDF policy at ingest
    pub ingested_at: String,                 // RFC3339
}

pub struct NewContentEvent { /* same minus seq */ }
```

All previous types (`PublishEvent`, `EventAuthorization`, `CatalogEntry`,
`ContentManifest`, `Visibility`, `PublishEventKind`, `PublishOutcome`,
`CatalogView`) are deleted.

### `src/auth/` — CWT verification

- **Vendor or re-use `pep_check`** from
  [`arkavo-org/opentdf-rs/examples/pep_check.rs`](https://github.com/arkavo-org/opentdf-rs).
  If pep_check is not importable as a library module, vendor it as
  `src/auth/pep_check.rs` with attribution.
- `src/auth/cwt.rs` becomes a thin wrapper:
  ```rust
  pub async fn verify(
      cwt: &[u8],
      bound_node_id: &str,         // not Option — caller MUST bind
  ) -> Result<VerifiedClaims, VerifyError>;
  ```
- `src/auth/cose_keys.rs`: replaced by pep_check's JWKS handling if available;
  otherwise wrapped with our stale-while-revalidate cache (recording refresh
  timestamps **only on success**).
- `src/auth/entitlements.rs`:
  ```rust
  pub fn cwt_to_entitlements(claims: &VerifiedClaims) -> Entitlements;
  ```

### `src/pdp/cache.rs` — AccessPdpCache

```rust
pub struct AccessPdpCache {
    url: String,
    http: reqwest::Client,
    pdp: ArcSwap<Arc<AccessPdp>>,
    last_force_refresh: Mutex<Option<Instant>>,
}

impl AccessPdpCache {
    pub async fn spawn(url: String, refresh_interval: Duration, http: reqwest::Client) -> Result<Arc<Self>>;
    pub fn load(&self) -> Arc<AccessPdp>;
    pub async fn force_refresh(&self) -> bool;  // single-flight via Mutex; stamp on success only
}
```

- **Boot:** hard-fail if first fetch fails. PDP without definitions = 100%
  silent denial; better to crashloop visibly.
- **Steady state:** background refresh on interval; on error, keep previous
  `Arc<AccessPdp>` (stale-while-revalidate). Warn-log.

### `src/protocol/catalog_read.rs` — tdf/catalog/1

```rust
pub const ALPN: &[u8] = b"tdf/catalog/1";

pub async fn handle(
    conn: iroh::endpoint::Connection,
    node: Arc<TdfIrohNode>,
) -> Result<()>;
```

Wire format (CBOR, length-prefixed):

```rust
struct CatalogSubscribe {
    cwt: ByteBuf,
    after_seq: Option<u64>,
}

enum CatalogStreamMsg {
    Entry(ContentEvent),
    CaughtUp { seq: u64 },
    Heartbeat,
    TokenExpiringSoon { exp: i64 },
    Error { code: ErrorCode, message: String },
}

enum ErrorCode {
    BadRequest, PdpUnavailable, Internal, TooManySubscriptions,
}
```

`Entry` carries a `ContentEvent` directly — no wire-only `ContentEntry` type
in v1. If the on-disk and on-wire shapes need to diverge in v2 (e.g. to add
an attribution field on the wire but not in storage), a wire-only struct
gets introduced then.

Handler responsibilities:
1. Read one `CatalogSubscribe`. Reject oversized frames (>64KB).
2. Verify CWT with `bound_node_id = conn.remote_node_id()`. On **any**
   verification failure, **close the connection silently** (per contract §4 —
   do not return an `Error` frame; that leaks token validity).
3. Build entitlements via `cwt_to_entitlements`. On unknown action name, also
   silently close (per contract §6).
4. Per-NodeId concurrent-subscription cap (default 4). Per-process cap
   (default 256). Excess → `Error { TooManySubscriptions }` + close.
5. Backfill: `EventStore::list_from(after_seq.unwrap_or(0))` → PDP check →
   emit `Entry` on Allow. Emit `CaughtUp { seq: current_tail }`.
6. Live: `EventStore::subscribe()` → on each new event, PDP check, emit
   `Entry`. Spawn a heartbeat tick.
7. CWT expiry: when `now > exp - 60` emit `TokenExpiringSoon`; when `now > exp`
   emit `Error { BadCwt }` and close.
8. On broadcast `Lagged`: emit `Error { Internal }` and close (reader reconnects).
9. On server shutdown (`cancel` token) or peer disconnect: drop the stream.

### `src/node.rs` — TdfIrohNode

```rust
pub struct TdfIrohNode {
    router: Router,
    store: FsStore,
    endpoint: Endpoint,
    pub s3_client: Arc<S3Client>,
    pub config: Arc<Config>,
    pub catalog: Arc<EventStore>,
    pub verifier: Arc<Verifier>,     // wraps pep_check
    pub pdp: Arc<AccessPdpCache>,
    cancel: CancellationToken,
}
```

Router accepts only `iroh_blobs::ALPN` and `tdf/catalog/1` — no docs, no
gossip. `wait_and_ingest` is extended: on successful `ingest_from_store`,
extract attribute-value FQNs from the TDF manifest and call
`catalog.append(NewContentEvent { ... })`.

### `src/config.rs`

```toml
[s3]
bucket = "..."
region = "..."

[auth]
cose_keys_url      = "https://identity.arkavo.net/.well-known/cose-keys"
issuer             = "https://identity.arkavo.net"
refresh_interval_secs = 300         # default; reject 0
clock_skew_secs    = 60             # default

[pdp]
attribute_defs_url = "https://identity.arkavo.net/.well-known/attributes"
refresh_interval_secs = 300         # default; reject 0

[catalog]
data_dir              = "/var/lib/tdf-iroh-s3/catalog"      # redb path's parent
max_subscriptions_per_peer  = 4
max_subscriptions_total     = 256
```

- `AuthConfig` and `PdpConfig` get `Default` impls with empty URL strings.
  `Config` annotates both with `#[serde(default)]`.
- Boot path fail-closes when either URL is empty, with a clear error pointing
  at the offending config key. Fixes finding #2 without breaking deserialize
  for existing `config.toml` files lacking the sections.
- `refresh_interval_secs == 0` is rejected at parse time (custom deserialize
  helper). Fixes finding #12.

## Data flow

### Ingest (per successful push)

```
wait_and_ingest(hash):
  ingest_from_store(hash) → IngestResult { hash_hex, size }
  data = store.get_bytes(hash).await
  manifest = opentdf::TdfArchive::read(data)?.manifest
  fqns = manifest_attr_value_fqns(&manifest)   // new helper
  catalog.append(NewContentEvent {
      content_id: hash_hex,
      manifest_ref: s3_client.manifest_key(&hash_hex),
      attribute_value_fqns: fqns,
      ingested_at: now_rfc3339(),
  }).await
```

The `append` future returns once the redb write commits and the broadcast
send fires (lossy — slow subscribers handle Lagged in their own task).

### Read (one subscriber)

```
on incoming stream (tdf/catalog/1):
  cwt_query = read_cbor_frame(stream, max=64KB).await
  claims = verifier.verify(&cwt_query.cwt, &remote_node_id).await
              .map_err(|_| close_silent())
  ents = cwt_to_entitlements(&claims)
              .map_err(|_| close_silent())  // unknown action → close
  enforce_subscription_caps(&remote_node_id)?
  pdp = pdp_cache.load()
  send_loop:
    let mut live = catalog.subscribe()
    let mut backfill = catalog.list_from(cwt_query.after_seq.unwrap_or(0))
    while let Some(ev) = backfill.next().await {
        if pdp.check(&ents, &Action::new("read"), &ev.attribute_value_fqns)?.is_allow() {
            send(CatalogStreamMsg::Entry(ev.into())).await?;
        }
    }
    let tail = catalog.current_tail();
    send(CatalogStreamMsg::CaughtUp { seq: tail }).await?;
    let mut hb = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            ev = live.recv() => { ... PDP check ... send Entry ... }
            _ = hb.tick() => { send(Heartbeat).await?; check_exp_warning_or_drop(&claims).await?; }
            _ = cancel.cancelled() => break,
        }
    }
```

## Tradeoffs

**Why not keep iroh-docs as a single-replica local store?** The cost in v1
(gossip task, blob-store sharing surface, larger dep tree, two more findings
to keep mitigated) outweighs the migration cost when v2 federation lands.
When v2 arrives, the natural shape (per [the note at the bottom](#federation-v2-sketch))
is iroh-blobs content-addressed events + gossip-of-hashes, **not** iroh-docs.
Going through iroh-docs would be a wrong-shape stopover.

**Why CBOR not JSON on the wire?** Consistent with the existing CWT/COSE
stack. Deterministic encoding. Readers are Rust clients that already speak
ciborium. JSON would be debuggable but unnecessary.

**Why hard-fail boot on PDP/COSE fetch?** The alternatives are silent denial
(every PDP check defaults Deny) or fail-open (skip auth). Both are worse than
crashloop. Operators see the failure immediately.

**Why drop the `End`-terminated one-shot mode?** Continuous sync is a wire
contract, not an optimization. Once `tdf/catalog/1` ships with a terminal
`End`, switching to subscription is a v2 ALPN. Cheaper to bake in now.

**Why single broadcast channel, not one per subject?** Per-subject channels
duplicate state. The PDP `check` per-event-per-subscriber is microsecond-scale
(per opentdf docs). Filtering at fan-out is fine through v2's expected scale.

## Federation v2 sketch

(Not in scope; written down so v1 decisions don't paint v2 into a corner.)

- Replace `seq → ContentEvent` redb table with content-addressed iroh-blobs:
  each event blob is BLAKE3-hashed canonical CBOR; key = hash.
- Attribution: each event includes a signed field `attribution: { node_id,
  ed25519_sig }` — never a path component, never an unverified string. This is
  the explicit fix for the finding #10 surface coming back at federation time.
- Index: per-node gossip topic publishing `(hash, ingested_at)` tuples. Each
  peer maintains its own redb cache `hash → ingested_at` for ordering.
- ALPN: federation peers open `tdf/catalog-sync/1` with a node-CWT proving
  membership in the federation; reuses the same pep_check primitives.

## Findings resolution

| # | Resolution |
|---|---|
| 1 | publish_content deleted; verifier wired into `tdf/catalog/1` ALPN |
| 2 | AuthConfig/PdpConfig get `Default`; boot fail-closes on empty URL |
| 3 | `EventStore::append` takes typed event; no CWT param by design |
| 4 | Dissolved — iroh-docs/iroh-gossip removed entirely |
| 5 | Closed — event bytes live in redb, never in FsStore |
| 6 | Closed — no docs/gossip ALPN registered externally |
| 7 | Verifier replaced by pep_check |
| 8 | Verifier replaced by pep_check |
| 9 | `Verifier::verify` signature takes `&str` (not `Option`); ALPN handler always passes peer NodeId |
| 10 | creator_id removed from event model; federation v2 reintroduces attribution as a signed field |
| 11 | Closed — redb is single-writer by construction |
| 12 | Config rejects `refresh_interval_secs == 0` at parse time |
| 13 | Verifier replaced by pep_check |
| 14 | AccessPdpCache (and any COSE key cache we keep) records refresh timestamp only on success; single-flight via Mutex |
| 15 | Dissolved — typed redb schema, no untyped prefix scan |

## Testing strategy

- **Unit:** EventStore append/list/subscribe roundtrip; ContentEvent CBOR
  roundtrip; cwt_to_entitlements grant translation (incl. unknown type skip,
  unknown action reject); refresh_interval==0 config rejection.
- **Integration:** end-to-end via `auth::test_signer` (updated to mint v1
  contract-conforming tokens with `authorization_details`) — mint a CWT with
  attrs A+B, ingest TDFs with policies (A), (B), (A∩B), (C), subscribe,
  assert correct filtered listing. Cover backfill + live phases, CaughtUp
  marker, expiry TokenExpiringSoon, BadCwt close-silent, TooManySubscriptions
  cap.
- **Property:** subscribe-then-append vs append-then-subscribe yields the
  same set of granted entries.

## Migration

The current branch has not shipped. No data migration. The Packer AMI on the
new branch:
- creates `/var/lib/tdf-iroh-s3/catalog/events.redb` on first boot
- requires `[auth].cose_keys_url`, `[auth].issuer`, `[pdp].attribute_defs_url`
  in `config.toml` — bootstrap.sh template updated.

## Dependencies (Cargo.toml deltas)

```
+ redb = "2"
- iroh-docs = "0.97"
- iroh-gossip = "0.97"
~ opentdf = { git = "https://github.com/arkavo-org/opentdf-rs", tag = "v0.12.0", default-features = false }
```

Keep: iroh, iroh-blobs, irpc, coset, ciborium, p256, arc-swap, reqwest,
blake3, tokio, anyhow, thiserror, bytes, hex, rand, futures-lite, tracing,
serde, serde_json, toml, time, base64, clap.

`coset`, `ciborium`, `p256`, `arc-swap` may become redundant once pep_check
absorbs COSE/CBOR/signature/cache duties; review and drop after the auth
rewrite lands.

---

## Appendix A — Arkavo CWT v1 contract (verbatim)

> Audience: tdf-iroh-s3 and other read-side PEPs.
> Issuer: the Arkavo authnz service (`authnz-rs`).
> Status: v1, last reviewed 2026-05-26.

### A.1 Token format
- **COSE_Sign1** (RFC 8392 CWT). Not a JWT.
- Signed with the platform's CWT signing key. Verification key set published
  at `https://<arkavo-platform>/.well-known/cose-keys` (JSON, `kid`-indexed
  COSE_Key).
- Decoder + verifier reference: `opentdf-rs/examples/pep_check.rs` — vendor
  or `cargo add opentdf-rs` and reuse it. Do not roll your own COSE_Sign1
  parser.

### A.2 Required claims

| Claim | CBOR key | Type | Required | Notes |
|---|---|---|---|---|
| `iss` | 1 | text | ✓ | Exact-match the configured platform issuer string. |
| `sub` | 2 | text | ✓ | Stable subject id. Surface to audit logs. |
| `iat` | 6 | int | ✓ | Reject tokens with `iat` more than 60s in the future (clock skew). |
| `exp` | 4 | int | ✓ | Reject if `now >= exp`. Reject if `exp - iat > 3600`. |
| `scope` | text key | text | ✓ | Space-separated. Must contain `catalog.read`. |
| `cnf` | 8 | map | conditional | Required when the connection is iroh-authenticated; see A.4. |
| `authorization_details` | text key | array | ✓ | RFC 9396; see A.3. |

Claims not in this table are ignored on the read path (e.g. `campaign_id`
is a publisher-side concept — accept but do not enforce).

### A.3 `authorization_details` (RFC 9396)

CBOR array of grant maps. Example (CBOR diagnostic notation):

```
"authorization_details" : [
  { "type":    "tdf_attribute",
    "fqn":     "https://acme.com/attr/classification/value/topsecret",
    "actions": ["read"] },
  { "type":    "tdf_attribute",
    "fqn":     "https://acme.com/attr/dept/value/eng",
    "actions": ["read"] }
]
```

v1 schema rules:
- `type` allowlist: `"tdf_attribute"`. **Unknown types: skip silently** —
  keeps clients forward-compatible when the platform adds new grant types.
- `actions` allowlist: `["read"]`. **Unknown action names: reject the whole
  token** — the issuer promised this allowlist and a violation indicates
  either a mis-minted token or a downgrade attempt.
- `fqn`: canonical OpenTDF attribute-value FQN
  (`https://<authority>/attr/<name>/value/<value>`). Case-sensitive. No
  normalization.
- Optional fields reserved for future use (clients must accept and ignore):
  `locations`, `obligations`.

After parsing, collapse to `Entitlements`
(`HashMap<String, Vec<String>>`) keyed by FQN.

### A.4 Channel binding (`cnf.iroh_node_id`)

When the read ALPN handler has an authenticated peer NodeId:
- `cnf.iroh_node_id` (bytes, 32) **must** be present and **must** equal the
  QUIC peer's NodeId.
- On mismatch: close the iroh connection. Do **not** return a 403 over an
  authenticated channel — that leaks token validity.
- On absent `cnf` over an iroh-authenticated channel: reject as malformed.

For non-iroh callers (e.g. local debugging tools), `cnf` enforcement is off
and the token verifies on standard claims alone. Production read paths
always pass `Some(connection_node_id)` and so are bound by construction.

### A.5 Decision-time PDP

The token gives you `Entitlements`. To turn that into a yes/no for a
specific object, pick the right PDP:

- **Resources tagged with a mandatory set of FQNs (every tag must match):**
  use `opentdf_rs::pdp::access::local::decide_any()`. Stateless,
  allocation-free, hot-path safe.
- **Resources whose attribute definitions use ANY_OF or HIERARCHY:** use
  `opentdf_rs::pdp::AccessPdp` with a locally loaded attribute-definition
  catalog. `access::local` cannot evaluate rule semantics from flat grants
  and will be subtly wrong for these cases.

v1 default: `AccessPdp` — the cost is a catalog load at startup.

### A.6 Failure modes — required behavior

| Condition | Action |
|---|---|
| Signature invalid / unknown `kid` | Reject. Log `kid` and source NodeId. |
| `exp` in the past | Reject. No client-side refresh — token-exchange is the platform's job. |
| `iat` > now + 60s | Reject. |
| `scope` missing `catalog.read` | Reject. |
| `cnf.iroh_node_id` mismatch (iroh path) | Close connection. |
| `authorization_details` missing or empty array | Reject — token carries no read rights. |
| `authorization_details[].type` unknown | Skip that entry. Do **not** reject. |
| `authorization_details[].actions` contains unknown action | Reject the whole token. |
| `authorization_details[].fqn` not parseable as a URL | Skip that entry. |
| PDP decision = deny | Return the iroh-equivalent of 403 (after the connection is established and authorized at the channel layer). |

### A.7 Versioning

Field-level: additive only. New optional fields and new `type` values land
in this document and propagate to clients on their own cadence — that's why
A.3 mandates skipping unknown types.

Breaking changes (action allowlist contraction, claim removal, signature
alg change): bumped to v2 with a new `scope` value (`catalog.read.v2`) so
old and new clients can coexist during rollout.

### A.8 Reference integration sketch

```rust
let claims = pep_check::verify_cwt(&cwt_bytes, &jwks, expected_iss, expected_node_id)?;
let grants = pep_check::parse_authorization_details(claims.payload())?;
let ents: Entitlements = grants_to_entitlements(&grants);
let decision = AccessPdp::new(catalog).check(&ents, &resource_fqns)?;
```

That is the entire integration. Anything more elaborate is probably wrong.

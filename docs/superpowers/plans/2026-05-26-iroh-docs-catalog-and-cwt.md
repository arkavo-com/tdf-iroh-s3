# iroh-docs Catalog with CWT-Gated Writes — Implementation Plan

> **For agentic workers:** Use `superpowers:subagent-driven-development`
> (recommended) or `superpowers:executing-plans` to implement task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the S3-backed publish event log with an `iroh-docs`
replica, and gate every write to that replica with a CWT (COSE_Sign1)
verified against a COSE_KeySet endpoint (`application/cose-key-set+cbor`)
configured in the node. The reference issuer is
`https://identity.arkavo.net`; its discovery document advertises the
endpoint as `arkavo_cose_keys_uri`.

**Architecture:** One node-local `iroh-docs` replica holds all publish
events under `creators/{creator_id}/events/{seq:020}` keys. **The event
log is the canonical catalog** — every other "catalog" in the codebase
is a disposable projection of it. The node owns a single iroh-docs
author identity. A `crate::auth::Verifier` checks incoming CWTs
(algorithm ES256, issuer from config, claims pinned to `creator_id` +
`campaign_id` + `catalog.write` scope). On success the node authors the
event and embeds the raw CWT in the event payload for audit. Nothing is
written back to the replica besides events — no snapshots, no signed
projection.

**Tech Stack:** `iroh-docs` 0.97 (pairs with `iroh` 0.97 / `iroh-blobs`
0.99), `coset` 0.4 (COSE_Sign1 + COSE_KeySet), `ciborium` (CBOR),
`p256` (ES256 verify), `arc-swap` (key cache), `reqwest` (HTTP fetch
of `application/cose-key-set+cbor`), `base64` (embedded CWT in event
payload).

**Spec:** `docs/superpowers/specs/2026-05-26-iroh-docs-catalog-and-cwt.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `Cargo.toml` | Modify | Add `iroh-docs`, `coset`, `ciborium`, `p256`, `arc-swap`, `base64` |
| `src/config.rs` | Modify | New `[catalog]` and `[auth]` config sections |
| `src/auth/mod.rs` | Create | Re-export `Verifier`, `VerifiedClaims`, `CoseKeyCache` |
| `src/auth/cwt.rs` | Create | COSE_Sign1 parse + signature verify + claim checks |
| `src/auth/cose_keys.rs` | Create | COSE_KeySet (CBOR) fetch, parse, cache, refresh |
| `src/auth/test_signer.rs` | Create (cfg test/feature) | Test fixture that mints valid CWTs |
| `src/catalog/types.rs` | Modify | Add `EventAuthorization`; extend `PublishEvent`; replace `Catalog` with `CatalogView { creator_id, entries }`; delete `CatalogSignature`, `CatalogDraft` |
| `src/catalog/mod.rs` | Modify | `build_catalog` now returns `CatalogView`; delete `finalize`, `canonical_json`, `sign_placeholder` |
| `src/catalog/keys.rs` | Modify | Add replica-key helpers; remove catalog-snapshot S3 keys |
| `src/catalog/replica.rs` | Create | `CatalogReplica` wrapper: open, write event, list events |
| `src/catalog/publish.rs` | Rewrite | Take `VerifiedClaims`, write to replica, drop S3 event-log writes |
| `src/node.rs` | Modify | Open replica, hold `Verifier` and `CatalogReplica`, persist author + namespace |
| `src/secret_key.rs` | Modify | Generalize so author key + namespace id are persisted via the same SSM/file pattern |
| `src/lib.rs` | Modify | Add `pub mod auth;` |
| `src/test_cli/push.rs` | Modify | Mint test CWT, pass to publish |
| `tests/catalog_event_log_test.rs` | Rewrite | Replica-based assertions instead of S3 |
| `tests/auth_cwt_test.rs` | Create | Verifier accepts valid, rejects wrong issuer/sig/exp |
| `tests/catalog_publish_auth_test.rs` | Create | Publish rejected without CWT; accepted with valid; rejected when CWT `sub` mismatches |

---

## Phase A — Auth module (depends on nothing else)

### Task A1: Wire dependencies

**Files:** `Cargo.toml`

- [x] **Step 1: Add crates**

  In `[dependencies]`, add:
  ```toml
  coset = "0.4"
  ciborium = "0.2"
  p256 = { version = "0.13", features = ["ecdsa"] }
  arc-swap = "1"
  base64 = "0.22"
  iroh-docs = "0.97"   # pairs with iroh 0.97 / iroh-blobs 0.99
  reqwest = { version = "0.13", default-features = false, features = ["rustls", "json"] }
  ```

- [x] **Step 2: Verify it builds**

  Run `cargo build`. Must compile with no code changes yet.

### Task A2: Config sections

**Files:** `src/config.rs`, `tests/config_test.rs`

- [x] **Step 1: Failing tests for `[auth]` and `[catalog]` parsing**

  Add tests that load a TOML containing `[auth] cose_keys_url = "..."`,
  `issuer = "..."`, and `[catalog] data_dir = "..."`. Assert fields parse;
  assert defaults (`refresh_interval_secs = 300`, `clock_skew_secs = 60`,
  `data_dir = "/var/lib/tdf-iroh-s3/docs"`). `[auth]` is **required** —
  add a test that a TOML without it fails to parse so the node cannot
  silently boot without a verifier.

- [x] **Step 2: Add `AuthConfig` and `CatalogConfig` structs**

  ```rust
  #[derive(Debug, Deserialize, Clone)]
  pub struct AuthConfig {
      /// URL of the COSE_KeySet endpoint (`application/cose-key-set+cbor`).
      pub cose_keys_url: String,
      pub issuer: String,
      #[serde(default = "default_refresh_interval_secs")]
      pub refresh_interval_secs: u64,
      #[serde(default = "default_clock_skew_secs")]
      pub clock_skew_secs: i64,
  }
  ```
  Add `pub catalog: CatalogConfig` and `pub auth: AuthConfig` to `Config`.

- [x] **Step 3: Tests pass**

### Task A3: COSE_Sign1 verify

**Files:** `src/auth/cwt.rs`, `src/auth/mod.rs`, `src/lib.rs`,
`src/auth/test_signer.rs`, `tests/auth_cwt_test.rs`

- [x] **Step 1: Implement `test_signer`**

  Behind `#[cfg(any(test, feature = "test-fixtures"))]`. Generates a
  P-256 keypair, exposes:
  ```rust
  pub fn cose_key_set(&self) -> Vec<u8>;     // CBOR bytes for the endpoint
  pub fn cose_key_cache(&self) -> Arc<CoseKeyCache>; // bypass HTTP for tests
  pub fn mint(&self, claims: TestClaims) -> Vec<u8>;
  ```
  `mint` builds a COSE_Sign1 with protected header `alg = ES256`,
  `kid = <test-kid>` (bytes), and CBOR-encoded claims map per RFC 8392.

- [x] **Step 2: Failing test — verifier accepts a freshly minted CWT**

  In `tests/auth_cwt_test.rs`, spin up the test signer, build a static
  `CoseKeyCache` (no HTTP) plus one separate test using a
  `tokio::net::TcpListener` to serve the CBOR `CoseKeySet` over real
  HTTP, point a `Verifier` at each, mint a CWT, call `verify`, assert
  claims.

- [x] **Step 3: Implement `Verifier::verify`**

  - Parse with `coset::CoseSign1::from_slice` (accept bare and tag(18)).
  - Pull `kid` (bytes) from protected header; look up in
    `CoseKeyCache`; refresh-once on miss.
  - Verify signature with `p256::ecdsa::VerifyingKey` over the
    `Sig_structure` bytes (`CoseSign1::verify_signature` builds them).
  - Decode payload CBOR map via `coset::cwt::ClaimsSet::from_cbor_value`.
  - Check `iss == config.issuer`, `exp > now - clock_skew`,
    `iat < now + clock_skew`, `scope.contains("catalog.write")`,
    `sub` and `campaign_id` present and non-empty.
  - If `cnf.iroh_node_id` present and `bound_node_id` provided, require
    equality.
  - Return `VerifiedClaims`.

- [x] **Step 4: Failing tests — verifier rejects bad cases**

  Wrong issuer; expired; tampered signature; missing scope; `sub` empty;
  unknown `kid` (after one refresh attempt). Add each as a separate
  `#[test]` so failure messages are precise.

- [x] **Step 5: Implement rejections; tests pass**

### Task A4: COSE_KeySet cache and refresh

**Files:** `src/auth/cose_keys.rs`

- [x] **Step 1: Background refresh task**

  `CoseKeyCache::spawn(url, interval, http_client) -> Arc<Self>` returns a
  handle; an internal tokio task refreshes on the configured interval and
  swaps the parsed `HashMap<Kid, VerifyingKey>` via `ArcSwap`.

- [x] **Step 2: On-demand refresh**

  `force_refresh()` rate-limited to one-per-second using
  `tokio::sync::Mutex<Instant>`. Called by the verifier when `kid` is not
  in the current snapshot.

- [x] **Step 3: Test — `force_refresh` rate-limit**

  Hammer 10 concurrent `force_refresh` calls against a local
  `TcpListener` serving an empty `CoseKeySet`; assert at most 2 fetches
  landed (initial + at most one within the 1s gate).

- [x] **Step 4: Test — parses the real arkavo CoseKeySet bytes**

  Embed the 113-byte response captured from
  `https://identity.arkavo.net/.well-known/cose-keys` as a fixture and
  assert it parses into exactly one P-256 key. This guards against
  regressions if `coset` or `p256` changes anything load-bearing.

---

## Phase B — Catalog replica (parallelizable with Phase A)

### Task B1: Persist namespace identity

**Files:** `src/catalog/replica.rs`, `src/node.rs`, `src/config.rs`

**As-built note:** the plan originally proposed generalizing
`secret_key.rs` to also persist a catalog author secret and namespace
secret in SSM. After exploring iroh-docs 0.97, that turned out to be
overreach: the docs runtime's persistent storage (`docs.redb` under
`[catalog] data_dir`) already manages its own author and namespace
secrets and exposes `DocsApi::author_default()` / `open(id)` to retrieve
them. We only need to persist the *public* `NamespaceId` (32 bytes) so
subsequent boots know which replica to reopen. That's handled in
`replica.rs::open_or_create` via a small `catalog.namespace_id` file
under the catalog data dir.

- [x] **Step 1: Inline namespace-id file persistence**

  `read_namespace_id`/`write_namespace_id` helpers in
  `catalog/replica.rs`. On first boot the file doesn't exist, we call
  `docs.create()` and write the 32-byte id. On subsequent boots we read
  the file and call `docs.open(id)`.

- [x] **Step 2: Test — namespace id persists across reopens**

  `namespace_id_persists_via_id_file` in
  `tests/catalog_event_log_test.rs`: open the replica twice through the
  same `Docs` runtime, assert the returned `namespace_id()` matches,
  and assert the on-disk file is 32 bytes equal to `id.as_bytes()`.

### Task B2: `CatalogReplica` wrapper

**Files:** `src/catalog/replica.rs`, `src/catalog/mod.rs`

- [x] **Step 1: Skeleton**

  ```rust
  pub struct CatalogReplica { /* iroh-docs Doc + Author */ }

  impl CatalogReplica {
      pub async fn open_or_create(
          docs: &iroh_docs::Docs,
          namespace: NamespaceSecret,
          author: AuthorId,
      ) -> Result<Self>;
      pub async fn append_event(&self, event: &PublishEvent) -> Result<u64>;
      pub async fn list_events(&self, creator_id: &str) -> Result<Vec<PublishEvent>>;
      pub fn namespace_id(&self) -> NamespaceId;
  }
  ```

- [x] **Step 2: Failing test — append then list round-trips**

  In `tests/catalog_event_log_test.rs` (rewrite), use a temp-dir iroh-docs
  store, append three events with different `creator_id`s, assert
  `list_events("creator_1")` returns exactly the ones with that id, in
  seq order.

- [x] **Step 3: Implement `append_event`**

  - Compute `next_seq` = `max(parse_event_seq(key)) + 1` over existing
    keys under `creators/{creator_id}/events/`.
  - Build key with `keys::replica_event_key(creator_id, next_seq)`.
  - Serialize event JSON.
  - Write via `Doc::set_bytes(author, key, value)`.
  - On `iroh-docs` author-collision (concurrent writer chose the same
    `seq`) retry up to 32 times — mirrors `MAX_EVENT_APPEND_RETRIES`.

- [x] **Step 4: Implement `list_events`**

  `Doc::get_many` with prefix `creators/{creator_id}/events/`, deserialize
  each entry's bytes into `PublishEvent`, return sorted by `seq`.

- [x] **Step 5: All tests pass**

### Task B3: Rewrite `publish_content`

**Files:** `src/catalog/publish.rs`, `src/catalog/types.rs`

- [x] **Step 1: Add `EventAuthorization`**

  ```rust
  pub struct EventAuthorization {
      pub cwt_b64: String,
      pub issuer: String,
      pub cti: String,
  }
  ```
  Add `pub authorization: EventAuthorization` to `PublishEvent`.

- [x] **Step 2: Rewrite signature**

  ```rust
  pub async fn publish_content(
      metadata: ContentMetadata,
      payload: Bytes,
      auth: &VerifiedClaims,
      replica: &CatalogReplica,
      s3: &S3Client,
  ) -> Result<PublishOutcome>;
  ```
  The `creator_id` is `auth.creator_id` — no longer a free parameter.

- [x] **Step 3: Body**

  1. Write payload to S3 (idempotent, as today).
  2. Write per-content manifest to S3 (as today).
  3. Build `CatalogEntry` (as today).
  4. Build `PublishEvent` with `authorization = EventAuthorization {
     cwt_b64: base64(auth.raw_cwt), issuer: ..., cti: auth.cti }`.
  5. `replica.append_event(&event)` → returns `seq`.
  6. Return `PublishOutcome { content_id, seq }`. No `version` field —
     the event log is canonical; projections are reader-side and
     disposable.

- [x] **Step 4: Delete obsolete code**

  Remove `load_events`, `next_event_seq`, S3 event-key writes,
  `catalog_snapshot_key`, `catalog_latest_key`, `CatalogSignature`,
  `CatalogDraft`, `sign_placeholder`, `canonical_json`, and all related
  tests. Keep `content_payload_key`, `content_manifest_key`. Update
  `build_catalog` to return `CatalogView` and adjust the existing
  pure-function tests accordingly (they should now assert on entries
  only, not signatures).

- [x] **Step 5: Failing tests, then passing**

  Tests in `tests/catalog_publish_auth_test.rs`:
  - Publish with valid CWT for `creator_1` → succeeds, event in replica.
  - Publish with CWT whose `sub = creator_2` but caller passes
    `creator_1` → reject (the new signature makes this representationally
    impossible; assert at the verifier-bound boundary instead — i.e.
    `auth.creator_id` is the source of truth).

  **As-built note:** the structural test ("sub mismatch is
  representationally impossible") is now compile-time: the
  `publish_content` signature only accepts `auth.creator_id`, no
  free-parameter creator_id. The full end-to-end success test requires
  a working S3 backend; the repo's existing test pattern (`new_mock` +
  no real S3) doesn't cover the put_object path, and adding a real S3
  mock just for one test inflates surface area. Deferred to **Phase C**
  where the full node wiring lands and one e2e test can exercise the
  whole pipeline against `S3Client::new_mock` or a localstack
  container.
  - Publish with expired CWT → verifier rejects before `publish_content`
    is reached. Cover this in `tests/auth_cwt_test.rs` not here.

---

## Phase C — Integration (depends on A + B)

### Task C1: Wire `Verifier` and `CatalogReplica` into `TdfIrohNode`

**Files:** `src/node.rs`, `src/config.rs`

- [x] **Step 1: Hold both on the node**

  ```rust
  pub struct TdfIrohNode {
      // ...existing...
      pub catalog: Arc<CatalogReplica>,
      pub verifier: Arc<Verifier>,
  }
  ```

- [x] **Step 2: Spawn-time setup**

  Inside `TdfIrohNode::spawn`:
  - Spawn `Gossip::builder().spawn(endpoint.clone())`.
  - Spawn `Docs::persistent(&config.catalog.data_dir).spawn(endpoint,
    blobs, gossip)`.
  - Open the replica via `CatalogReplica::open_or_create(&docs,
    blobs_store, namespace_id_path)` where `namespace_id_path` is
    `{config.catalog.data_dir}/catalog.namespace_id`.
  - Spawn `CoseKeyCache` from `config.auth.cose_keys_url`. Initial
    fetch failures are logged-and-tolerated so a brief upstream outage
    or a bogus test URL doesn't block node boot.
  - Build `Verifier::new(keys, issuer, clock_skew_secs)`.
  - Register all three ALPNs (`iroh_blobs::ALPN`, `iroh_docs::ALPN`,
    `iroh_gossip::ALPN`) on the router.

- [x] **Step 3: Smoke test**

  `tests/node_ingest_test.rs`: `test_push_blob_stored_in_node` now
  asserts `node.catalog.namespace_id() != [0; 32]`. The `test_config`
  helper takes a tmp `Path` and derives separate `iroh` and `docs`
  subdirectories so the catalog data dir doesn't collide with
  `/var/lib/tdf-iroh-s3/docs`.

### Task C2: CLI publish path — **deferred**

**Files:** `src/test_cli/push.rs`

This task can't ship in Phase C as originally drafted. The plan
envisaged the CLI calling `publish_content` directly after verifying a
CWT, but:

- `publish_content` needs a `&CatalogReplica`, which is a handle to the
  *running node's* docs runtime. Two processes cannot share one
  `docs.redb` (it's a `redb` lockfile).
- A CLI binary that opens its own docs runtime against the same data
  dir would deadlock with a running node, and a node-spawning CLI
  binary defeats the point of having a separate test client.

The right shape is a custom RPC ALPN on the node — something like
`tdf-iroh-s3/publish/v1` — that accepts a `(cwt, metadata, blob_hash)`
tuple, verifies the CWT server-side, and calls `publish_content`. That
ALPN is its own design + tests + wire format and belongs in a follow-up
plan.

For now, **`push_tdf` is unchanged**: it pushes a blob over
`iroh-blobs` as before. No catalog event is recorded — the catalog
publish path is reachable only from in-process callers of
`publish_content`, which lets us land the auth + replica machinery
without committing to a wire format prematurely.

- [ ] **(deferred)** Custom publish-RPC ALPN
- [ ] **(deferred)** `--cwt <PATH>` / `--cwt-test` flags on the CLI
- [ ] **(deferred)** End-to-end CLI → node publish smoke test

---

## Phase D — Tests + tidy

### Task D1: Rewrite catalog integration tests

**Files:** `tests/catalog_event_log_test.rs`,
`tests/catalog_publish_auth_test.rs` (new)

- [ ] All existing S3-event-log assertions are gone. New assertions exercise
  the replica directly: append, list, sequence allocation under concurrent
  writers, prefix-scoping by creator.

### Task D2: Audit dead code

- [ ] `cargo build` clean.
- [ ] `cargo clippy -- -D warnings` clean (or document any deferred lint).
- [ ] No `dead_code` warnings on the catalog/snapshot helpers we kept (if
  any of them are now unused, delete them).
- [ ] Confirm `keys::events_prefix`, `keys::event_key`,
  `keys::parse_event_seq` are either deleted or repurposed for the
  replica's in-replica key layout — do not leave both an S3 version and a
  replica version with the same name.

### Task D3: Doc strings

- [ ] Top-of-module doc on `src/catalog/replica.rs` explaining the
  one-replica-per-node model and the `creators/{creator_id}/events/{seq}`
  key layout, with the same level of detail as the current `mod.rs` doc.
  Lead with the canonical-model statement: "The event log in this replica
  is the catalog. `CatalogView` is a disposable projection."
- [ ] Top-of-module doc on `src/auth/mod.rs` listing supported algorithms
  (ES256 only) and explicitly noting the unverified surface (no `cti`
  replay cache).

---

## Verification (manual, before pushing)

- [ ] `cargo test` — all green.
- [ ] `cargo test --test catalog_publish_auth_test` — verifies the gate.
- [ ] Run `cargo run --bin tdf-iroh-s3-test -- push --cwt-test ...`
  against a local node configured with a JWKS URL serving the test
  signer's public key; confirm the event appears under
  `creators/<sub>/events/00000000000000000001` in the replica.
- [ ] Restart the node and confirm the replica persists (namespace +
  author keys reload, event still readable).

## Rollback

The replica state is node-local and disposable. Rolling back amounts to:
delete `config.catalog.data_dir`, revert the commit, deploy. No S3 cleanup
needed — the S3 event-log keys we stop writing were unreleased, and the
S3 keys we keep writing (payload + manifest) are unchanged.

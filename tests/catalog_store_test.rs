use tdf_iroh_s3::catalog::store::EventStore;
use tdf_iroh_s3::catalog::types::NewContentEvent;

fn sample(content_id: &str) -> NewContentEvent {
    NewContentEvent {
        content_id: content_id.to_string(),
        manifest_ref: format!("manifests/{content_id}.json"),
        attribute_value_fqns: vec!["https://example/attr/a/value/x".to_string()],
        ingested_at: "2026-05-26T00:00:00Z".to_string(),
    }
}

#[tokio::test]
async fn append_assigns_monotonic_seq_starting_at_1() {
    let dir = tempfile::tempdir().unwrap();
    let store = EventStore::open(&dir.path().join("events.redb")).await.unwrap();

    let e1 = store.append(sample("aaa")).await.unwrap();
    let e2 = store.append(sample("bbb")).await.unwrap();
    let e3 = store.append(sample("ccc")).await.unwrap();

    assert_eq!(e1.seq, 1);
    assert_eq!(e2.seq, 2);
    assert_eq!(e3.seq, 3);
    assert_eq!(store.current_tail(), 3);
}

#[tokio::test]
async fn current_tail_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.redb");

    {
        let s = EventStore::open(&path).await.unwrap();
        s.append(sample("x")).await.unwrap();
        s.append(sample("y")).await.unwrap();
    }

    let reopened = EventStore::open(&path).await.unwrap();
    assert_eq!(reopened.current_tail(), 2);

    let e3 = reopened.append(sample("z")).await.unwrap();
    assert_eq!(e3.seq, 3);
}

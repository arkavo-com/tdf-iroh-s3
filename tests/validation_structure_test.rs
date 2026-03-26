use tdf_iroh_s3::validation::structure::validate_tdf_structure;

#[test]
fn test_valid_tdf_accepted() {
    let tdf_bytes = create_test_tdf();
    let result = validate_tdf_structure(&tdf_bytes);
    assert!(
        result.is_ok(),
        "Valid TDF should be accepted: {:?}",
        result.err()
    );
}

#[test]
fn test_random_bytes_rejected() {
    let garbage = vec![0u8; 256];
    let result = validate_tdf_structure(&garbage);
    assert!(result.is_err(), "Random bytes should be rejected");
}

#[test]
fn test_empty_zip_rejected() {
    let mut buf = Vec::new();
    {
        let writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        writer.finish().unwrap();
    }
    let result = validate_tdf_structure(&buf);
    assert!(result.is_err(), "ZIP without manifest should be rejected");
}

fn create_test_tdf() -> Vec<u8> {
    use opentdf::prelude::*;

    let policy = PolicyBuilder::new()
        .id_auto()
        .dissemination(["test@example.com"])
        .build()
        .unwrap();

    Tdf::encrypt(b"test payload data")
        .kas_url("https://kas.example.com")
        .policy(policy)
        .to_bytes()
        .unwrap()
}

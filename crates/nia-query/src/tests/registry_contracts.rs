use super::*;

#[test]
fn declarative_registry_records_single_consumer_storage() {
    let mut registry = QueryRegistry::new();
    registry.register::<TestContext, OwnedNonCloneValueQuery>();

    let descriptors = registry.descriptors();
    assert_eq!(descriptors.len(), 1);
    assert_eq!(
        descriptors[0].storage,
        QueryStoragePolicy::SingleConsumerOwned
    );
}

#[test]
fn declarative_registry_records_an_external_owned_producer() {
    let mut registry = QueryRegistry::new();
    registry.register::<TestContext, PublishedOwnedValueQuery>();

    let descriptors = registry.descriptors();
    assert_eq!(
        descriptors[0].provider,
        QueryProviderPolicy::ExternallyPublished
    );
    assert_eq!(
        descriptors[0].storage,
        QueryStoragePolicy::SingleConsumerOwned
    );
}

#[test]
fn declarative_registry_records_and_enforces_query_contracts() {
    let mut registry = QueryRegistry::new();
    registry.register::<TestContext, Double>();
    let db = QueryDb::new_registered(
        TestContext {
            executions: AtomicUsize::new(0),
        },
        registry,
    );

    assert_eq!(*db.expect_get(Double(21)), 42);
    let descriptors = db.registered_queries();
    assert_eq!(descriptors.len(), 1);
    assert_eq!(descriptors[0].name, "double");
    assert_eq!(descriptors[0].key_type, std::any::type_name::<Double>());
    assert_eq!(descriptors[0].value_type, std::any::type_name::<usize>());
    assert_eq!(descriptors[0].provider, QueryProviderPolicy::KeyExecute);
    assert_eq!(descriptors[0].fingerprint, QueryFingerprintPolicy::None);
    assert_eq!(descriptors[0].storage, QueryStoragePolicy::CacheOwnedArc);

    let missing = std::panic::catch_unwind(|| db.get(NonCloneValueQuery));
    assert!(missing.is_err());
}

#[test]
fn fingerprint_builder_is_deterministic_and_domain_separated() {
    let fingerprint = |domain| {
        let mut builder = QueryFingerprintBuilder::new(domain);
        builder.write_u8(7);
        builder.write_u64(42);
        builder.write_str("nia");
        builder.finish()
    };

    assert_eq!(
        fingerprint(FingerprintDomain::new("nia.query-a.v1")),
        fingerprint(FingerprintDomain::new("nia.query-a.v1"))
    );
    assert_ne!(
        fingerprint(FingerprintDomain::new("nia.query-a.v1")),
        fingerprint(FingerprintDomain::new("nia.query-b.v1"))
    );
    assert_eq!(std::mem::size_of::<QueryFingerprint>(), 16);
}

#[test]
fn fingerprint_byte_stream_matches_one_shot_bytes_and_enforces_length() {
    let domain = FingerprintDomain::new("nia.query.stream-test.v1");
    let mut direct = QueryFingerprintBuilder::new(domain);
    direct.write_bytes(b"streamed payload");

    let mut streamed = QueryFingerprintBuilder::new(domain);
    let mut writer = streamed.bytes_writer(16);
    writer.write_chunk(b"streamed ").expect("first chunk");
    writer.write_chunk(b"payload").expect("second chunk");
    writer.finish().expect("complete stream");
    assert_eq!(streamed.finish(), direct.finish());

    let mut incomplete_builder = QueryFingerprintBuilder::new(domain);
    let incomplete = incomplete_builder.bytes_writer(1);
    assert_eq!(
        incomplete.finish().expect_err("incomplete stream").kind(),
        std::io::ErrorKind::UnexpectedEof
    );

    let mut oversized_builder = QueryFingerprintBuilder::new(domain);
    let mut oversized = oversized_builder.bytes_writer(1);
    assert_eq!(
        oversized
            .write_chunk(b"too long")
            .expect_err("oversized chunk")
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
}

#[test]
fn fingerprint_domains_require_a_versioned_nia_identity() {
    assert_eq!(
        FingerprintDomain::new("nia.query.product.v12").as_str(),
        "nia.query.product.v12"
    );
    for invalid in [
        "query.product.v1",
        "nia.query.product",
        "nia..v1",
        "nia.query..product.v1",
        "nia.-query.product.v1",
        "nia.query-.product.v1",
        "nia.query--product.v1",
        "nia.query_product.v1",
        "nia.query.product.v0",
        "nia.query.product.v01",
    ] {
        assert!(
            std::panic::catch_unwind(|| FingerprintDomain::new(invalid)).is_err(),
            "accepted invalid fingerprint domain {invalid}"
        );
    }
}

#[test]
fn declarative_registry_records_stable_value_fingerprints() {
    let mut registry = QueryRegistry::new();
    registry.register::<TestContext, StableInput>();

    assert_eq!(
        registry.descriptors()[0].fingerprint,
        QueryFingerprintPolicy::StableValue
    );
}

#[test]
#[should_panic(expected = "is already registered")]
fn declarative_registry_rejects_duplicate_key_types() {
    let mut registry = QueryRegistry::new();
    registry.register::<TestContext, Double>();
    registry.register::<TestContext, Double>();
}

#[test]
#[should_panic(expected = "query name `double` is already registered")]
fn declarative_registry_rejects_duplicate_names() {
    let mut registry = QueryRegistry::new();
    registry.register::<TestContext, Double>();
    registry.register::<TestContext, DuplicateDoubleName>();
}

#[test]
fn query_node_ids_are_word_sized_and_database_scoped() {
    assert_eq!(std::mem::size_of::<QueryNodeId>(), 8);
    let first = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });
    let second = QueryDb::new(TestContext {
        executions: AtomicUsize::new(0),
    });

    let first_id = first.slot_for(&Double(1)).node_id;
    let second_id = second.slot_for(&Double(1)).node_id;

    assert_ne!(first_id, second_id);
    assert_eq!(first_id.index, second_id.index);
    assert_ne!(first_id.db_id, second_id.db_id);
}

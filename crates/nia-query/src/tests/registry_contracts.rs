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

    assert_eq!(fingerprint("query-a.v1"), fingerprint("query-a.v1"));
    assert_ne!(fingerprint("query-a.v1"), fingerprint("query-b.v1"));
    assert_eq!(std::mem::size_of::<QueryFingerprint>(), 16);
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

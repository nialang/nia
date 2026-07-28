fn temp_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "nia_signature_cache_{name}_{}_{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    ))
}

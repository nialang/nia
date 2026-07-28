fn sym(text: &str) -> SymbolId {
    SymbolId::from_stable_hash(stable_hash(text))
}

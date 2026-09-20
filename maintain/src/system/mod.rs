//! Host and cgroup resource metadata used to label maintenance evidence.
/// Machine identity and host metadata probing.
pub mod machine;
/// Bounded child-process execution and process-group cleanup.
pub mod process;
/// Procfs and cgroup resource parsing and probing.
pub mod resources;
/// Compiler source and external tool identities attached to performance evidence.
pub mod toolchain;

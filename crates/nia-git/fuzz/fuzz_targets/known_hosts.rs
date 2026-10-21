#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    nia_git_fuzz::known_hosts(data);
});

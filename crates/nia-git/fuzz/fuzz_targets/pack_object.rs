#![no_main]

libfuzzer_sys::fuzz_target!(|data: &[u8]| {
    nia_git_fuzz::pack_object(data);
});

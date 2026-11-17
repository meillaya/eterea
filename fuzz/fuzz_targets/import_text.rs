#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let ingester = eterea_core::Ingester::new();
    let _ = ingester.parse_content("json", text);
    let _ = ingester.parse_content("js", text);
    let _ = ingester.parse_content("csv", text);
});

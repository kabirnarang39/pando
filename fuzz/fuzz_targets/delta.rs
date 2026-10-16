//! Delta payloads written by compaction, applied against an arbitrary base.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let split = data.first().map_or(0, |&b| b as usize).min(data.len().saturating_sub(1));
    let (payload, base) = data.split_at(split.max(1).min(data.len()));
    let _ = pando_core::delta::parse(payload);
    let _ = pando_core::delta::decode_with_base(payload, base);
});

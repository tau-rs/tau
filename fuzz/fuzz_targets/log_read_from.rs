//! `Log::read_from` over arbitrary bytes.
//!
//! A log file is the kernel's source of truth and the replay CLI (M2) will
//! read logs written by other builds. Any bytes must yield a `Log` or a
//! `LogError`; a `Log` that comes out must write itself back out and read in
//! again with the same header and entries.

#![no_main]

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use tau_kernel::abi::LogHeader;
use tau_kernel::log::{Entry, Log};

fn refold(log: &Log) -> Result<(LogHeader, Vec<Entry>), String> {
    let mut buf = Vec::new();
    log.write_to(&mut buf).map_err(|e| e.to_string())?;
    let again = Log::read_from(Cursor::new(buf)).map_err(|e| e.to_string())?;
    Ok((again.header(), again.entries().to_vec()))
}

fuzz_target!(|data: &[u8]| {
    let Ok(log) = Log::read_from(Cursor::new(data)) else {
        return;
    };
    assert_eq!(
        refold(&log),
        Ok((log.header(), log.entries().to_vec())),
        "log did not survive a round trip"
    );
});

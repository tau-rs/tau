//! UUID / ULID / now-timestamp helpers routed through `Clock` and `RandomSource` ports.
//!
//! These are the only places in the kernel that materialize a session
//! ID, run ID, trace ID, or wall-clock timestamp. The kernel never
//! reaches `chrono::Utc::now()` / `ulid::Ulid::new()` /
//! `uuid::Uuid::new_v4()` directly — every call site routes through
//! these helpers, fed by ports.

use alloc::string::String;
use alloc::sync::Arc;

use tau_ports::{Clock, RandomSource};

/// Mint a UUID v4 from the supplied `RandomSource`. Returns the
/// canonical 36-character hyphenated form.
///
/// # Example
///
/// ```
/// use tau_runtime_core::ids::uuid_v4;
/// use tau_ports::DeterministicRandom;
/// use std::sync::Arc;
///
/// let random: Arc<dyn tau_ports::RandomSource> = Arc::new(DeterministicRandom::seeded(42));
/// let id = uuid_v4(&random);
/// let s = id.to_string();
/// assert_eq!(s.len(), 36);
/// // version nibble must be 4 (UUID v4)
/// assert_eq!(&s[14..15], "4");
/// ```
pub fn uuid_v4(random: &Arc<dyn RandomSource>) -> uuid::Uuid {
    let mut bytes = [0u8; 16];
    random.fill(&mut bytes);
    // Variant + version bits per RFC 4122 §4.4.
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    uuid::Uuid::from_bytes(bytes)
}

/// Mint a ULID from the supplied `Clock` + `RandomSource`. Returns the
/// base32-encoded canonical form (26 chars).
///
/// # Example
///
/// ```
/// use tau_runtime_core::ids::ulid;
/// use tau_ports::{MockClock, DeterministicRandom};
/// use std::sync::Arc;
///
/// let clock: Arc<dyn tau_ports::Clock> = Arc::new(MockClock::at(1_000));
/// let random: Arc<dyn tau_ports::RandomSource> = Arc::new(DeterministicRandom::seeded(7));
/// let id = ulid(&clock, &random);
/// assert_eq!(id.len(), 26);
/// ```
pub fn ulid(clock: &Arc<dyn Clock>, random: &Arc<dyn RandomSource>) -> String {
    let ts = clock.now().max(0) as u64;
    let mut rand_bytes = [0u8; 10];
    random.fill(&mut rand_bytes);
    let mut b = [0u8; 16];
    b[..10].copy_from_slice(&rand_bytes);
    let rand_part = u128::from_le_bytes(b);
    // `to_str` (no_std-safe) rather than `Display`/`to_string` (ulid gates
    // its `Display` impl behind its `std` feature, which this no_std kernel
    // does not enable). A canonical ULID is always 26 base32 chars.
    let mut buf = [0u8; 26];
    let s = ::ulid::Ulid::from_parts(ts, rand_part).array_to_str(&mut buf);
    String::from(&*s)
}

/// Wall-clock now as `chrono::DateTime<Utc>` from the supplied `Clock`.
///
/// # Example
///
/// ```
/// use tau_runtime_core::ids::now_utc;
/// use tau_ports::MockClock;
/// use std::sync::Arc;
///
/// let clock: Arc<dyn tau_ports::Clock> = Arc::new(MockClock::new());
/// let dt = now_utc(&clock);
/// // MockClock increments per call; verify post-epoch + non-negative.
/// assert!(dt.timestamp_millis() > 0);
/// ```
pub fn now_utc(clock: &Arc<dyn Clock>) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(clock.now())
        .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(0).unwrap())
}

/// Mint a [`tau_domain::MessageId`] (UUID v7) from the `Clock` + `RandomSource`
/// ports. The no_std kernel uses this instead of `MessageId::new` (which is
/// std-only and reads the ambient clock/RNG) so message ids are reproducible
/// under deterministic ports — the property wasm conformance relies on.
pub fn message_id(clock: &Arc<dyn Clock>, random: &Arc<dyn RandomSource>) -> tau_domain::MessageId {
    let mut rand_bytes = [0u8; 10];
    random.fill(&mut rand_bytes);
    tau_domain::MessageId::from_parts(clock.now().max(0) as u64, rand_bytes)
}

/// Mint a [`tau_domain::AgentInstanceId`] (UUID v7) from the `Clock` +
/// `RandomSource` ports — the no_std counterpart of `AgentInstanceId::new`.
pub fn agent_instance_id(
    clock: &Arc<dyn Clock>,
    random: &Arc<dyn RandomSource>,
) -> tau_domain::AgentInstanceId {
    let mut rand_bytes = [0u8; 10];
    random.fill(&mut rand_bytes);
    tau_domain::AgentInstanceId::from_parts(clock.now().max(0) as u64, rand_bytes)
}

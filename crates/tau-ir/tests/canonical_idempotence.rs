//! Property: `decode(encode(x)) == x` and `encode(decode(encode(x))) == encode(x)`.

use tau_ir::canonical::{from_canonical_bytes, to_canonical_bytes};
use tau_ir::{IrFormatVersion, IrModule, Workflow};
use tau_ports::target::registry;

fn sample_module() -> IrModule {
    IrModule {
        ir_format: IrFormatVersion::current(),
        tau_version: "0.0.0".into(),
        target: registry::list_available()
            .next()
            .expect("at least one available target")
            .triple,
        workflow: Workflow::default(),
        triggers: Vec::new(),
    }
}

#[test]
fn round_trip_through_bytes() {
    let m = sample_module();
    let bytes1 = to_canonical_bytes(&m);
    let m2 = from_canonical_bytes(&bytes1).expect("decode");
    assert_eq!(m, m2);
    let bytes2 = to_canonical_bytes(&m2);
    assert_eq!(bytes1, bytes2, "encoder is idempotent");
}

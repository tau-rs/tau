//! `Kernel::shred` over the port (ADR-0012 §3, §6): the same suite on
//! `Memory` and on `Disk`, because the port is one contract with two
//! implementations.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use common::{echo_id, run, run_with, stores, tokio_spawner, CHILD_RESULT, ROOT_RESULT};
use tau_kernel::abi::{AgentId, BlobRef, Budget, DimKey, Namespace};
use tau_kernel::blob::Memory;
use tau_kernel::driver::echo::EchoDriver;
use tau_kernel::kernel::{Kernel, ShredError};
use tau_kernel::log::Log;
use tau_kernel::reducer::fold;
use tau_kernel::syscall::{program, CancelMode, Match, WaitFor};
use tau_store::Disk;
use tokio::sync::Notify;

#[tokio::test]
async fn a_shred_is_invisible_to_the_fold() {
    // §5 as a test: the same scenario on Memory untouched and on Disk with
    // the child shredded. The two logs are byte-for-byte equal, the live
    // states hash the same, and the file form of the shredded run folds to
    // that hash with no store beside it — which is what `tau replay` does.
    let untouched = run(Box::new(Memory::new()), CHILD_RESULT, ROOT_RESULT).await;
    let dir = tempfile::tempdir().unwrap();
    let shredded = run(
        Box::new(Disk::open(dir.path()).unwrap()),
        CHILD_RESULT,
        ROOT_RESULT,
    )
    .await;
    shredded.kernel.shred(shredded.child).unwrap();
    shredded.assert_readable(shredded.child, false, "shredded");

    let bytes = shredded.sink.contents();
    assert_eq!(
        bytes,
        untouched.sink.contents(),
        "the logs are the same bytes"
    );
    assert_eq!(shredded.kernel.state_hash(), untouched.kernel.state_hash());
    let reread = Log::read_from(bytes.as_slice()).unwrap();
    assert_eq!(
        fold(reread.entries()).unwrap().hash(),
        untouched.kernel.state_hash(),
        "the written log folds to the same hash with every child payload gone"
    );
}

#[tokio::test]
async fn a_shred_is_observable_only_through_read() {
    for (name, blobs, _dir) in stores() {
        let run = run(blobs, CHILD_RESULT, ROOT_RESULT).await;
        let entries = run.kernel.entries();
        let state = run.kernel.state();
        let hash = run.kernel.state_hash();
        run.assert_readable(run.child, true, name);
        run.assert_readable(run.root, true, name);

        run.kernel.shred(run.child).unwrap();

        run.assert_readable(run.child, false, name);
        run.assert_readable(run.root, true, name);
        assert_eq!(
            run.kernel.entries(),
            entries,
            "{name}: the log is untouched"
        );
        assert_eq!(run.kernel.state(), state, "{name}: the state is untouched");
        assert_eq!(run.kernel.state_hash(), hash, "{name}");
    }
}

#[tokio::test]
async fn a_shred_of_a_live_subtree_is_refused() {
    for (name, blobs, _dir) in stores() {
        let kernel = Kernel::boot_with(Log::with_sink(Vec::new()).unwrap(), tokio_spawner, blobs);
        let echo = kernel
            .register_driver(
                echo_id(),
                EchoDriver::new(),
                Budget::from_dims([(DimKey::Tokens, 64)]),
            )
            .unwrap();
        let ns = Namespace::from_caps([echo]);
        let child_ns = ns.clone();
        let started = Arc::new(Notify::new());
        let started_in = Arc::clone(&started);
        let root = kernel
            .spawn_root(
                program(move |root| async move {
                    let child = root
                        .spawn(
                            program(move |child| async move {
                                started_in.notify_one();
                                // Waits for a notice that only a cancel brings.
                                let _ = child.recv(Match::Any).await;
                                child.exit(b"never")
                            }),
                            child_ns,
                            Budget::from_dims([(DimKey::Tokens, 100)]),
                        )
                        .unwrap();
                    let _ = root.wait(WaitFor::Child(child)).await;
                    root.exit(ROOT_RESULT)
                }),
                ns,
                Budget::from_dims([(DimKey::Tokens, 1_000), (DimKey::Depth, 1)]),
            )
            .unwrap();
        started.notified().await;
        let child = kernel
            .state()
            .agents()
            .find_map(|(id, a)| (a.parent == Some(root)).then_some(id))
            .unwrap();

        assert_eq!(
            kernel.shred(child),
            Err(ShredError::Live(child)),
            "{name}: one live agent is enough, and it is named"
        );
        assert!(
            matches!(kernel.shred(root), Err(ShredError::Live(_))),
            "{name}: a live subtree is refused from its root too"
        );

        kernel
            .cancel_from_harness(child, &CancelMode::immediate())
            .unwrap();
        kernel.drained().await.unwrap();
        kernel.shutdown();
        assert_eq!(
            kernel.shred(child),
            Ok(()),
            "{name}: finished, so shreddable"
        );
        assert_eq!(kernel.shred(root), Ok(()), "{name}");
        assert_eq!(
            kernel.read(tau_kernel::blob::digest(ROOT_RESULT)),
            None,
            "{name}: the root's result went with the root"
        );
    }
}

#[tokio::test]
async fn the_root_shreds_its_own_payloads_too() {
    for (name, blobs, _dir) in stores() {
        let run = run(blobs, CHILD_RESULT, ROOT_RESULT).await;
        run.kernel.shred(run.root).unwrap();
        run.assert_readable(run.root, false, name);
        run.assert_readable(run.child, false, name);
    }
}

#[tokio::test]
async fn a_shred_of_an_agent_that_put_nothing_is_a_no_op() {
    for (name, blobs, _dir) in stores() {
        // The child sends nothing and its result is the empty reference:
        // nothing was ever put for it.
        let run = run_with(blobs, b"", ROOT_RESULT, false).await;
        assert!(
            run.refs_of(run.child).is_empty(),
            "{name}: no payload of its own"
        );
        assert_eq!(run.kernel.shred(run.child), Ok(()), "{name}");
        let unknown = AgentId::new(4_000_000);
        assert_eq!(run.kernel.shred(unknown), Ok(()), "{name}: unknown agent");
        run.assert_readable(run.root, true, name);
        assert_eq!(run.kernel.read(run.handed_up), Some(Vec::new()), "{name}");
    }
}

#[tokio::test]
async fn a_second_shred_is_a_no_op() {
    for (name, blobs, _dir) in stores() {
        let run = run(blobs, CHILD_RESULT, ROOT_RESULT).await;
        run.kernel.shred(run.child).unwrap();
        let entries = run.kernel.entries();
        run.kernel.shred(run.child).unwrap();
        run.assert_readable(run.child, false, name);
        run.assert_readable(run.root, true, name);
        assert_eq!(run.kernel.entries(), entries, "{name}");
    }
}

#[tokio::test]
async fn identical_bytes_from_two_owners_survive_the_shred_of_one() {
    for (name, blobs, _dir) in stores() {
        let run = run(blobs, b"the same bytes", b"the same bytes").await;
        let blob = tau_kernel::blob::digest(b"the same bytes");
        assert!(run.refs_of(run.child).contains(&blob), "{name}");
        assert!(run.refs_of(run.root).contains(&blob), "{name}");

        run.kernel.shred(run.child).unwrap();
        assert_eq!(
            run.kernel.read(blob).as_deref(),
            Some(&b"the same bytes"[..]),
            "{name}: the root's copy is still there"
        );
        run.kernel.shred(run.root).unwrap();
        assert_eq!(run.kernel.read(blob), None, "{name}: both owners gone");
    }
}

#[tokio::test]
async fn the_empty_reference_is_never_stored_and_never_shredded() {
    for (name, blobs, _dir) in stores() {
        let run = run(blobs, b"", b"").await;
        assert_eq!(run.kernel.read(BlobRef::EMPTY), Some(Vec::new()), "{name}");
        run.kernel.shred(run.child).unwrap();
        run.kernel.shred(run.root).unwrap();
        assert_eq!(run.kernel.read(BlobRef::EMPTY), Some(Vec::new()), "{name}");
        assert_eq!(run.kernel.read(run.handed_up), Some(Vec::new()), "{name}");
    }
}

#[tokio::test]
async fn a_parent_reading_a_shredded_childs_result_gets_none() {
    // §1's named consequence: the root was handed the child's result by
    // `wait`, and owns nothing of it.
    for (name, blobs, _dir) in stores() {
        let run = run(blobs, CHILD_RESULT, ROOT_RESULT).await;
        assert_eq!(
            run.kernel.read(run.handed_up).as_deref(),
            Some(CHILD_RESULT),
            "{name}"
        );
        run.kernel.shred(run.child).unwrap();
        assert_eq!(run.kernel.read(run.handed_up), None, "{name}");
        run.assert_readable(run.root, true, name);
    }
}

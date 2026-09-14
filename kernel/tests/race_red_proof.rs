//! Scratch: a deliberate data race so the Tier 2 `tsan (scheduler)` job can
//! be shown to go red on a real fault (#38). The workspace forbids `unsafe`;
//! this scratch commit lowers that to `deny` so the file can opt out. Never
//! merge.
#![allow(unsafe_code)]

static mut COUNTER: u64 = 0;

#[test]
fn a_deliberate_data_race_for_the_red_proof() {
    let workers: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(|| {
                for _ in 0..100_000 {
                    // SAFETY: none. That is the point of this file.
                    unsafe {
                        let p = &raw mut COUNTER;
                        *p += 1;
                    }
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
}

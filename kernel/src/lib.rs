//! The tau agent kernel.
//!
//! tau is an *agent kernel*: the minimal, stable substrate on which agent
//! harnesses and pipelines are composed. It is deliberately not an agent
//! framework. Everything an agent can do is one of seven syscalls, and nothing
//! else is expressible:
//!
//! | # | Syscall  | Group |
//! |---|----------|-------|
//! | 1 | `spawn`  | tree  |
//! | 2 | `exit`   | tree  |
//! | 3 | `wait`   | tree  |
//! | 4 | `cancel` | tree  |
//! | 5 | `send`   | flow  |
//! | 6 | `recv`   | flow  |
//! | 7 | `attach` | meta  |
//!
//! The *irreducibility test* is the regression rule against scope creep: any
//! proposed eighth syscall must be shown inexpressible as a program over these
//! seven. If it is expressible, it belongs in `libtau`, not here.
//!
//! # What is frozen
//!
//! [`abi`] is the constitution. The message envelope and the grant types are
//! the "we do not break userspace" surface; everything else in this crate may
//! churn freely. See `docs/adr/0004-abi-freeze.md`.
//!
//! # Status
//!
//! M0, the walking skeleton: the [`log`], the [`reducer`], four of the seven
//! syscalls (`spawn`, `exit`, `send`, `recv`), and an echo [`driver`]. The
//! loop runs end to end and its log refolds to the same state hash. `wait`,
//! `cancel`, the full budget dimensions, and the clock driver are M1; hooks
//! and `attach` are M2.
//!
//! # Shape
//!
//! ```text
//! Program  ──Handle──▶  Kernel  ──Delivery──▶  Driver
//!    ▲                   │  │                     │
//!    └──────Msg──────────┘  └──── Log ◀── reply ──┘
//!                              (the truth)
//! ```
//!
//! Every arrow through the kernel is a log [`Entry`](log::Entry), appended
//! before it takes effect. [`reducer::fold`] over the entries reproduces
//! [`reducer::State`] exactly; the kernel's in-memory state is that fold,
//! kept warm.

pub mod abi;
pub mod blob;
pub mod driver;
pub mod kernel;
pub mod log;
pub mod reducer;
pub mod syscall;

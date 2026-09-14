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
//! M1b: the [`log`], the [`reducer`], six of the seven syscalls (`spawn`,
//! `exit`, `wait`, `cancel`, `send`, `recv`), an echo [`driver`], a virtual
//! clock and a wall clock. The loop runs end to end, a cancelled subtree is
//! frozen, notified, abandoned at the drivers, and aborted at a tick, every
//! reserved budget dimension is enforced — a send reserves the driver's
//! ceiling before delivery, the clock spends wall time as it ticks — and
//! every log refolds to the same state hash. Hooks and `attach` are M2.
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
pub mod bridge;
pub mod driver;
pub mod kernel;
pub mod log;
pub mod reducer;
pub mod syscall;

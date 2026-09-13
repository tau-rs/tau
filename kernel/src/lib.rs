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
//! Pre-M0. This crate currently contains the frozen ABI and nothing else — by
//! design: the pipeline and the ABI guard land before the first line of kernel
//! logic, so that logic arrives as a pull request that already flows through
//! them.

pub mod abi;

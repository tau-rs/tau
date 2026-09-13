//! Budgets: resource grants, carved atomically and returned at exit.
//!
//! # Why a map and not a struct
//!
//! A fixed set of fields (`tokens`, `cost`, `wall_ms`) would make every new
//! device kind an ABI break: an image model reports pixels, a classifier
//! reports `compute_ms`, a sandbox reports CPU seconds. A map of dimensions
//! lets a new driver report in its own native units on the day it ships, with
//! the reserved keys covering the dimensions the kernel itself enforces.
//!
//! # Why the kernel can account without understanding
//!
//! The kernel never parses a payload. It learns what a call cost from the
//! [`Consumption`] a driver attaches to its reply. That is precisely what makes
//! model-agnosticism structural rather than aspirational: swapping Anthropic
//! for a local vLLM changes a driver, and the accounting code does not move.

use core::fmt;
use core::str::FromStr;
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{Name, NameError};

/// A budget dimension: one reserved key, or a driver-defined one.
///
/// Reserved keys are the dimensions the kernel enforces itself. Everything else
/// is a [`DimKey::Custom`], which the kernel carries, sums, and compares
/// without interpreting.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DimKey {
    /// Model tokens, in and out combined.
    Tokens,
    /// Monetary cost in millionths of a US dollar.
    CostMicroUsd,
    /// Wall-clock milliseconds, enforced by the reducer when a clock tick is
    /// applied — never by reading a clock.
    WallMs,
    /// Number of `send` calls.
    Calls,
    /// Depth of the agent tree below the grant holder.
    Depth,
    /// In-process compute milliseconds, as reported by local ML drivers.
    ComputeMs,
    /// A driver-defined dimension.
    Custom(Name),
}

impl DimKey {
    /// The wire form of this key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Tokens => "tokens",
            Self::CostMicroUsd => "cost_microusd",
            Self::WallMs => "wall_ms",
            Self::Calls => "calls",
            Self::Depth => "depth",
            Self::ComputeMs => "compute_ms",
            Self::Custom(name) => name.as_str(),
        }
    }

    /// Every reserved key, in wire form.
    #[must_use]
    pub const fn reserved() -> &'static [&'static str] {
        &[
            "tokens",
            "cost_microusd",
            "wall_ms",
            "calls",
            "depth",
            "compute_ms",
        ]
    }
}

impl fmt::Display for DimKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DimKey {
    type Err = NameError;

    /// Parses a wire-form key.
    ///
    /// A reserved key always parses to its variant, so a driver cannot shadow
    /// `tokens` with a custom dimension of the same name and quietly detach its
    /// spending from the dimension the kernel enforces.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "tokens" => Self::Tokens,
            "cost_microusd" => Self::CostMicroUsd,
            "wall_ms" => Self::WallMs,
            "calls" => Self::Calls,
            "depth" => Self::Depth,
            "compute_ms" => Self::ComputeMs,
            other => Self::Custom(Name::new(other)?),
        })
    }
}

impl Serialize for DimKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for DimKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

/// A resource grant along zero or more dimensions.
///
/// An absent dimension means *no grant*, not unlimited: an agent that was never
/// given `tokens` cannot spend a token. Unlimited is not expressible, on
/// purpose.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Budget {
    dims: BTreeMap<DimKey, u64>,
}

/// Why a budget operation failed.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BudgetError {
    /// The parent holds no grant along a dimension the child asked for.
    ///
    /// Distinct from [`Self::Insufficient`] because it is almost always an
    /// authoring mistake rather than exhaustion.
    NoGrant {
        /// The dimension asked for.
        dim: DimKey,
    },
    /// The parent's remaining grant is smaller than the request.
    Insufficient {
        /// The dimension in question.
        dim: DimKey,
        /// What the parent has left.
        available: u64,
        /// What was asked for.
        requested: u64,
    },
    /// Returning unspent budget would exceed [`u64::MAX`].
    ///
    /// Unreachable if carve and restore are paired correctly, which is exactly
    /// why it is an error rather than a wrapping add: the overflow is the
    /// symptom of a double-restore, and silently wrapping would hide it.
    Overflow {
        /// The dimension in question.
        dim: DimKey,
    },
}

impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoGrant { dim } => write!(f, "no grant along dimension `{dim}`"),
            Self::Insufficient {
                dim,
                available,
                requested,
            } => write!(
                f,
                "insufficient `{dim}`: {available} available, {requested} requested"
            ),
            Self::Overflow { dim } => write!(f, "budget overflow along dimension `{dim}`"),
        }
    }
}

impl std::error::Error for BudgetError {}

impl Budget {
    /// An empty budget: a grant of nothing along every dimension.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Builds a budget from dimension/amount pairs.
    pub fn from_dims<I: IntoIterator<Item = (DimKey, u64)>>(dims: I) -> Self {
        Self {
            dims: dims.into_iter().collect(),
        }
    }

    /// The remaining grant along `dim`, or `None` if there is no grant at all.
    #[must_use]
    pub fn get(&self, dim: &DimKey) -> Option<u64> {
        self.dims.get(dim).copied()
    }

    /// Iterates the dimensions in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = (&DimKey, u64)> + '_ {
        self.dims.iter().map(|(k, v)| (k, *v))
    }

    /// Whether this budget grants nothing along any dimension.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dims.is_empty()
    }

    /// Carves `requested` out of this budget, atomically.
    ///
    /// Either every dimension is deducted or none is: the check runs to
    /// completion before the first mutation. A partial carve would leave a
    /// parent short of budget it never granted, and the shortfall would surface
    /// as an unrelated failure much later.
    ///
    /// # Errors
    ///
    /// [`BudgetError::NoGrant`] if this budget has no grant along a requested
    /// dimension, or [`BudgetError::Insufficient`] if the remaining grant is
    /// too small. In either case `self` is unchanged.
    pub fn carve(&mut self, requested: &Self) -> Result<Self, BudgetError> {
        for (dim, want) in requested.iter() {
            match self.dims.get(dim) {
                None => return Err(BudgetError::NoGrant { dim: dim.clone() }),
                Some(&have) if have < want => {
                    return Err(BudgetError::Insufficient {
                        dim: dim.clone(),
                        available: have,
                        requested: want,
                    })
                }
                Some(_) => {}
            }
        }
        for (dim, want) in requested.iter() {
            if let Some(have) = self.dims.get_mut(dim) {
                *have -= want;
            }
        }
        Ok(requested.clone())
    }

    /// Returns unspent budget to this one, as happens when a child exits.
    ///
    /// # Errors
    ///
    /// [`BudgetError::Overflow`] if the sum would exceed [`u64::MAX`], which
    /// can only happen if the same grant is restored twice.
    pub fn restore(&mut self, unspent: &Self) -> Result<(), BudgetError> {
        for (dim, amount) in unspent.iter() {
            let slot = self.dims.entry(dim.clone()).or_insert(0);
            *slot = slot
                .checked_add(amount)
                .ok_or_else(|| BudgetError::Overflow { dim: dim.clone() })?;
        }
        Ok(())
    }
}

/// What a call actually cost, as reported by the driver that made it.
///
/// Attached to replies. The kernel sums these against the sender's budget
/// without knowing what any dimension means.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Consumption {
    dims: BTreeMap<DimKey, u64>,
}

impl Consumption {
    /// Nothing consumed.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// Builds a consumption report from dimension/amount pairs.
    pub fn from_dims<I: IntoIterator<Item = (DimKey, u64)>>(dims: I) -> Self {
        Self {
            dims: dims.into_iter().collect(),
        }
    }

    /// The amount consumed along `dim`.
    #[must_use]
    pub fn get(&self, dim: &DimKey) -> Option<u64> {
        self.dims.get(dim).copied()
    }

    /// Iterates the dimensions in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = (&DimKey, u64)> + '_ {
        self.dims.iter().map(|(k, v)| (k, *v))
    }

    /// Whether nothing at all was consumed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dims.is_empty()
    }
}

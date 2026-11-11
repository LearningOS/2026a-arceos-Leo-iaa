//! Collection types.
//!
//! Re-exports everything from [`alloc::collections`] and adds a [`HashMap`],
//! which `alloc` does not provide because hashing needs a source of
//! randomness that only a full `std` normally has.

#[doc(no_inline)]
pub use alloc::collections::*;

pub mod hash_map;

#[doc(inline)]
pub use self::hash_map::HashMap;

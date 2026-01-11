//! Authentication and authorization
//!
//! Token types:
//! - `sub-` prefix: Stateless subscription tokens (JWT-like, self-contained)
//! - `role-` prefix: Stateful role tokens (ID lookup, revocable)
//!
//! Grant types:
//! - `read`: Receive events from matching channels
//! - `write`: Push events to matching channels
//! - `create`: Create new channels in that namespace
//! - `all`: Shorthand for read+write+create

mod grants;
mod tokens;

pub use grants::{Grant, GrantSet, GrantType};
pub use tokens::{AdminKey, RoleToken, SubToken, Token, TokenError};

pub mod schema;
pub mod sql_idents;
pub mod sqlite;

pub use sql_idents::{
    CountableTable, FragmentTable, SidecarTable, TRUSTED_REFERENCE_ROLE_PREDICATE,
};
pub use sqlite::Database;

/// The highest schema migration currently required by the canonical read model.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;

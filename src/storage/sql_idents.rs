//! Trusted SQL identifiers for RecallEngine.
//!
//! SQLite parameters can bind *values*, not table or column names. Any
//! identifier interpolated into SQL must therefore come from a closed enum
//! (or an equally closed static constant), never from `&str` call-site input.

/// Conversation-shard tables deactivated by fragment reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentTable {
    Conversations,
    Nodes,
    Messages,
}

impl FragmentTable {
    pub const ALL: [Self; 3] = [Self::Conversations, Self::Nodes, Self::Messages];

    /// Stable SQLite table name. Safe to interpolate into identifier slots only.
    pub const fn sql_name(self) -> &'static str {
        match self {
            Self::Conversations => "conversations",
            Self::Nodes => "nodes",
            Self::Messages => "messages",
        }
    }
}

/// Sidecar tables deactivated by sidecar reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarTable {
    Feedback,
    SharedConversations,
    LibraryFiles,
}

impl SidecarTable {
    pub const ALL: [Self; 3] = [
        Self::Feedback,
        Self::SharedConversations,
        Self::LibraryFiles,
    ];

    /// Stable SQLite table name. Safe to interpolate into identifier slots only.
    pub const fn sql_name(self) -> &'static str {
        match self {
            Self::Feedback => "feedback",
            Self::SharedConversations => "shared_conversations",
            Self::LibraryFiles => "library_files",
        }
    }
}

/// Tables counted by import stats / projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountableTable {
    Conversations,
    Nodes,
    Messages,
    Assets,
    ContentReferences,
    Feedback,
    SharedConversations,
    LibraryFiles,
}

impl CountableTable {
    /// Stable SQLite table name. Safe to interpolate into identifier slots only.
    pub const fn sql_name(self) -> &'static str {
        match self {
            Self::Conversations => "conversations",
            Self::Nodes => "nodes",
            Self::Messages => "messages",
            Self::Assets => "assets",
            Self::ContentReferences => "content_references",
            Self::Feedback => "feedback",
            Self::SharedConversations => "shared_conversations",
            Self::LibraryFiles => "library_files",
        }
    }

    /// Whether `is_active = 1` is meaningful for this table's "active" count.
    pub const fn supports_active_filter(self) -> bool {
        !matches!(self, Self::ContentReferences)
    }
}

/// Trusted static predicate: only active user/assistant messages are citation targets.
pub const TRUSTED_REFERENCE_ROLE_PREDICATE: &str =
    "LOWER(TRIM(COALESCE(m.role, ''))) IN ('user', 'assistant')";

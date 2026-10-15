pub mod chunking;
pub mod embedding;
pub mod hybrid;
pub mod indexer;
pub mod spike;
pub mod store;

pub use chunking::{chunk_content_block, ChunkerOptions, TextChunk};
pub use embedding::{EmbeddingProvider, FastEmbedProvider, MockDeterministicProvider, OfflineMode};
pub use hybrid::{HybridMatch, HybridSearchService, RrfParams};
pub use indexer::{IndexingStats, SemanticIndexer};
pub use spike::{SemanticSpikeRunner, SpikeReport};
pub use store::{
    ensure_sqlite_vec_registered, register_sqlite_vec_on_connection, SqliteBlobStore,
    SqliteVecStore, SyncStatus, VectorMatch, VectorStore,
};

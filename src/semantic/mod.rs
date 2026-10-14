pub mod chunking;
pub mod embedding;
pub mod spike;
pub mod store;

pub use chunking::{chunk_content_block, ChunkerOptions, TextChunk};
pub use embedding::{EmbeddingProvider, FastEmbedProvider, MockDeterministicProvider};
pub use spike::{SemanticSpikeRunner, SpikeReport};
pub use store::{SqliteBlobStore, SqliteVecStore, VectorMatch, VectorStore};

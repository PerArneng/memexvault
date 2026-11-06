//! Fakes and shared suites for tests in this workspace. Tests never use the production wiring.
pub mod blob_contract;
pub mod contract;
mod fakes;
pub mod generate;
pub mod lexical_contract;
mod mem_blobs;
mod mem_lexical;
mod mem_registry;
mod mem_store;
mod mem_vector;
pub mod registry_contract;
pub mod vector_contract;

pub use fakes::{FakeRasterizer, ManualClock, SeededRandom};
pub use mem_blobs::{BlobStep, MemBlobStore};
pub use mem_lexical::MemLexical;
pub use mem_registry::MemRegistry;
pub use mem_store::{CallCounts, MemStore};
pub use mem_vector::{FakeEmbedder, MemVector, fake_vector, passes};

/// The search edges every test server starts with.
pub fn mem_search() -> crate::SearchEdges<MemLexical, MemVector, FakeEmbedder> {
    crate::SearchEdges {
        lexical: MemLexical::new(),
        vector: MemVector::new(),
        embedder: FakeEmbedder::default(),
    }
}

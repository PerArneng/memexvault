//! Pure: traits, the Engine, reducers and policy. No runtime, network or file access. Anything with an edge sits behind a trait here.
mod auth;
mod batch;
mod blob;
mod clock;
mod dump;
mod engine;
pub mod governance;
pub mod jobs;
pub mod ocr;
pub mod raster;
mod reducer;
mod registry;
pub mod rendition;
mod replay;
mod seed;
mod server;
mod settings;
pub mod sources;
mod store;
pub mod testing;
mod upcast;

pub use auth::{Caller, Decision, Operation, authorize};
pub use batch::{BatchLimits, Pending, plan_batch};
pub use blob::{BlobInfo, BlobStore, BlobStoreError, BlobStored, Verified, sweepable};
pub use clock::Clock;
pub use dump::{Dump, canonical_dump, dump_store};
pub use engine::{ApplyError, Engine, NewSource, RecordSourceError};
pub use raster::{ALLOWED_DPI, PageImage, RasterError, Rasterizer, THUMBNAIL_DPI, rect_to_pixels};
pub use reducer::{ChangeSetRecorded, ReduceError, StateRecorded, reduce};
pub use registry::{Registry, RegistryError};
pub use replay::{ReplayError, all_events, replay};
pub use seed::Seed;
pub use server::{
    ClaimRef, CreatedToken, EvidenceLineage, Lineage, NewToken, PIPELINE_VERSION, PageList, Proposal, ProposalOutcome, RENDITION_JOB, RESULT_LIMIT,
    RenditionInput, Server, ServerEdges, ServerError,
};
pub use settings::{Backends, Settings, SettingsError};
pub use sources::{ALLOWED_MEDIA_TYPES, UNTRUSTED, UploadLimits, UploadOutcome, UploadRequest, sniff_media_type};
pub use store::{CommitItem, PAGE_LIMIT, Receipt, Snapshot, Store, StoreError, WriterLease};
pub use upcast::{CURRENT_VERSIONS, UpcastError, upcast};

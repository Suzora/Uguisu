//! The archive engine.
//!
//! The pure half of the archive engine: deterministic path templates with OS
//! sanitization profiles (ADR 0009/0022), component-aware path resolution
//! under the archive root (`docs/SECURITY.md` §3.2), collision handling,
//! the verification algorithm over a `&Path` and archive-policy
//! evaluation. Nothing here touches the database, HTTP or the engine, and
//! nothing here ever deletes user data.
//!
//! Rendering is a pure function of a [`template::Context`]: a path preview
//! creates no directory and reads no file.
//!
//! Lifecycle: `docs/STATE_MACHINES.md` §4.

pub mod collision;
pub mod image;
pub mod import;
pub mod layout;
pub mod manifest;
pub mod path;
pub mod policy;
pub mod sanitize;
pub mod scan;
pub mod sidecar;
pub mod template;
pub mod verify;

pub use collision::{Holder, Occupancy, Placement, place};
pub use image::{ImageError, sniff_image};
pub use import::{Candidate, ImportFormat, SourceFormat, Verdict};
pub use layout::{CONTROL_DIR, is_control_path, is_sidecar_path, sidecar_of};
pub use manifest::{ManifestDiff, ManifestError};
pub use path::{PathError, RelativePath, is_inside, resolve, resolve_checked};
pub use policy::{EffectivePolicy, Position, decide};
pub use sanitize::{MAX_PATH_CHARS, MAX_SEGMENT_BYTES, MAX_SEGMENT_CHARS};
pub use scan::{ScanError, ScanOptions, ScannedFile, walk};
pub use sidecar::{MAX_SIDECAR_BYTES, SidecarError};
pub use template::{Context, Template, TemplateError};
pub use verify::{Expectation, Outcome, verify};

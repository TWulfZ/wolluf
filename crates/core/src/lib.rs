//! Shared domain vocabulary: ids, time, keymode, version keys, error codes, clock (architecture §5.1). No algorithms, no IO.

pub mod anchor;
pub mod clock;
pub mod digest;
pub mod error;
pub mod id;
pub mod keymode;
pub mod time;
pub mod vkey;

pub use anchor::SegmentAnchor;
pub use clock::{Clock, FixedClock};
pub use digest::{AliasId, BlobSha256, ChartMd5, Game, PlayId, ProfileId, ScopeHash};
pub use error::{CoreError, ErrorCode};
pub use id::{AxisId, PatternId, StageId};
pub use keymode::{ColMask, Keymode};
pub use time::{DotNetTicks, FileTime, RateMilli, TimeUs, UnixUs};
pub use vkey::{VersionKey, VersionKeyBuilder};

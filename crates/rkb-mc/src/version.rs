//! Which Minecraft versions this crate understands, and how it decides.
//!
//! A version is never assumed from a folder name such as `1.21`: the save's
//! `DataVersion` is authoritative. Only versions with constants cross-checked
//! against published version data are listed. Other 1.21.x releases must be
//! added with their own descriptor and chunk-format tests before this crate
//! claims to read them.

use std::fmt;

/// How much confidence the crate has in a version descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionStatus {
    /// The version's constants are recorded and its world format is covered by
    /// this crate's tests.
    Verified,
    /// The version is known but has not been individually verified. Opening a
    /// world requires an explicit opt-in.
    Recognized,
}

/// Constants of one Minecraft: Java Edition release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionSpec {
    /// User-facing version, for example `1.21.1`.
    pub id: &'static str,
    /// `WorldVersion.dataVersion()` stored in `level.dat` and chunks.
    pub data_version: i32,
    /// `SharedConstants.getProtocolVersion()`.
    pub protocol: i32,
    /// Resource pack format (`pack.mcmeta`).
    pub resource_pack_format: u32,
    /// Data pack format (`pack.mcmeta`).
    pub data_pack_format: u32,
    pub status: VersionStatus,
}

impl VersionSpec {
    /// Whether this crate may open worlds of this version by default.
    pub const fn is_verified(self) -> bool {
        matches!(self.status, VersionStatus::Verified)
    }
}

/// Versions with constants taken from published Java Edition version data.
///
/// 1.21 and 1.21.1 share protocol 767 and pack formats 34/48; their data
/// versions differ (3953 and 3955). Later 1.21.x releases change pack formats
/// and data versions and are intentionally absent until they are verified.
pub const SUPPORTED: &[VersionSpec] = &[
    VersionSpec {
        id: "1.21.1",
        data_version: 3955,
        protocol: 767,
        resource_pack_format: 34,
        data_pack_format: 48,
        status: VersionStatus::Verified,
    },
    VersionSpec {
        id: "1.21",
        data_version: 3953,
        protocol: 767,
        resource_pack_format: 34,
        data_pack_format: 48,
        status: VersionStatus::Verified,
    },
];

/// Lookup failures for a world or chunk data version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionError {
    /// No descriptor exists for this data version.
    Unknown { data_version: i32 },
    /// A descriptor exists, but it is not verified by this crate.
    Unverified { id: &'static str, data_version: i32 },
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { data_version } => write!(
                f,
                "unsupported Minecraft data version {data_version}; \
                 this build reads 1.21 (3953) and 1.21.1 (3955)"
            ),
            Self::Unverified { id, data_version } => write!(
                f,
                "Minecraft {id} (data version {data_version}) is recognized but \
                 not verified by this build; open it explicitly to experiment"
            ),
        }
    }
}

impl std::error::Error for VersionError {}

/// The version table.
#[derive(Clone, Copy, Debug, Default)]
pub struct Versions;

impl Versions {
    /// All descriptors known to this build, newest first.
    pub const fn all() -> &'static [VersionSpec] {
        SUPPORTED
    }

    /// Finds a descriptor by `DataVersion`.
    pub fn find(data_version: i32) -> Option<&'static VersionSpec> {
        SUPPORTED
            .iter()
            .find(|spec| spec.data_version == data_version)
    }

    /// Resolves a world's data version under the default strict policy.
    pub fn require(data_version: i32) -> Result<&'static VersionSpec, VersionError> {
        Self::require_with(data_version, false)
    }

    /// Resolves a world's data version, optionally accepting a descriptor that
    /// has not been individually verified.
    pub fn require_with(
        data_version: i32,
        allow_unverified: bool,
    ) -> Result<&'static VersionSpec, VersionError> {
        let spec = Self::find(data_version).ok_or(VersionError::Unknown { data_version })?;
        if spec.status == VersionStatus::Recognized && !allow_unverified {
            return Err(VersionError::Unverified {
                id: spec.id,
                data_version,
            });
        }
        Ok(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_versions_are_verified() {
        let one = Versions::find(3953).expect("1.21 is present");
        assert_eq!(one.id, "1.21");
        assert_eq!(one.protocol, 767);
        assert_eq!(one.resource_pack_format, 34);
        assert_eq!(one.data_pack_format, 48);
        assert!(one.is_verified());

        let one_one = Versions::find(3955).expect("1.21.1 is present");
        assert_eq!(one_one.id, "1.21.1");
        assert_eq!(one_one.protocol, 767);
        assert_eq!(one_one.resource_pack_format, 34);
        assert_eq!(one_one.data_pack_format, 48);
        assert!(one_one.is_verified());
    }

    #[test]
    fn unknown_versions_are_rejected_not_guessed() {
        let error = Versions::require(4080).expect_err("1.21.2 is not verified here");
        assert_eq!(error, VersionError::Unknown { data_version: 4080 });
        assert!(error.to_string().contains("3953"));
    }

    #[test]
    fn supported_table_is_unique_and_ordered() {
        for (index, spec) in SUPPORTED.iter().enumerate() {
            assert!(
                !SUPPORTED[..index]
                    .iter()
                    .any(|other| other.data_version == spec.data_version),
                "duplicate data version {}",
                spec.data_version
            );
        }
        assert!(SUPPORTED
            .windows(2)
            .all(|pair| pair[0].data_version > pair[1].data_version));
    }
}

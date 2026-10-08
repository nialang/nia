// SPDX-License-Identifier: GPL-3.0-or-later
//! Build-draft envelope carrying a validated plan and configuration evidence.

use std::fmt;

use nia_compat::formats::CONFIGURE_MANIFEST;

use crate::{BuildPlan, PlanCodecError};

const MAGIC: &[u8; 8] = b"NIADRAF\0";
const MAX_DRAFT_BYTES: usize = 80 * 1024 * 1024;

/// Configuration evidence attached to a runner-produced plan draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildDraft {
    /// Canonical immutable plan produced by the build API.
    pub plan: BuildPlan,
    /// Opaque observation manifest encoded by the host-side observation layer.
    pub observations: Vec<u8>,
}

/// Error while decoding a build draft envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildDraftError {
    /// Payload exceeds the bounded draft budget.
    TooLarge,
    /// Header, lengths, checksum, or trailing bytes are invalid.
    InvalidEnvelope,
    /// The nested plan is malformed or semantically invalid.
    Plan(PlanCodecError),
}

impl fmt::Display for BuildDraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => f.write_str("build draft exceeds the size limit"),
            Self::InvalidEnvelope => f.write_str("build draft envelope is invalid"),
            Self::Plan(error) => write!(f, "build draft plan is invalid: {error}"),
        }
    }
}

impl std::error::Error for BuildDraftError {}

impl BuildDraft {
    /// Encodes a canonical plan and bounded observation payload.
    pub fn encode(&self) -> Result<Vec<u8>, BuildDraftError> {
        let plan = self.plan.encode().map_err(BuildDraftError::Plan)?;
        let plan_len = u32::try_from(plan.len()).map_err(|_| BuildDraftError::TooLarge)?;
        let observations_len =
            u32::try_from(self.observations.len()).map_err(|_| BuildDraftError::TooLarge)?;
        let total = 8usize
            .checked_add(4 + 4 + 32)
            .and_then(|value| value.checked_add(plan.len()))
            .and_then(|value| value.checked_add(self.observations.len()))
            .ok_or(BuildDraftError::TooLarge)?;
        if total > MAX_DRAFT_BYTES {
            return Err(BuildDraftError::TooLarge);
        }
        let mut encoded = Vec::with_capacity(total);
        encoded.extend_from_slice(MAGIC);
        encoded.extend_from_slice(&plan_len.to_le_bytes());
        encoded.extend_from_slice(&observations_len.to_le_bytes());
        encoded.extend_from_slice(blake3::hash(&plan).as_bytes());
        encoded.extend_from_slice(&plan);
        encoded.extend_from_slice(&self.observations);
        Ok(encoded)
    }

    /// Decodes the envelope and revalidates the nested plan through freeze.
    pub fn decode(bytes: &[u8]) -> Result<Self, BuildDraftError> {
        if bytes.len() > MAX_DRAFT_BYTES || bytes.len() < 48 || bytes[..8] != *MAGIC {
            return Err(BuildDraftError::InvalidEnvelope);
        }
        let plan_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let observations_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        let plan_start = 48usize;
        let plan_end = plan_start
            .checked_add(plan_len)
            .ok_or(BuildDraftError::InvalidEnvelope)?;
        let end = plan_end
            .checked_add(observations_len)
            .ok_or(BuildDraftError::InvalidEnvelope)?;
        if end != bytes.len()
            || bytes[16..48] != *blake3::hash(&bytes[plan_start..plan_end]).as_bytes()
        {
            return Err(BuildDraftError::InvalidEnvelope);
        }
        Ok(Self {
            plan: BuildPlan::decode(&bytes[plan_start..plan_end]).map_err(BuildDraftError::Plan)?,
            observations: bytes[plan_end..end].to_vec(),
        })
    }
}

/// Returns the registered observation format identity used by the envelope.
pub const fn observation_format() -> &'static str {
    CONFIGURE_MANIFEST.name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::draft;

    #[test]
    fn draft_round_trips_and_revalidates_plan() {
        let draft = BuildDraft {
            plan: BuildPlan::freeze(draft(false)).unwrap(),
            observations: b"observations".to_vec(),
        };
        let encoded = draft.encode().unwrap();
        assert_eq!(BuildDraft::decode(&encoded).unwrap(), draft);
        let mut damaged = encoded;
        damaged[16] ^= 1;
        assert_eq!(
            BuildDraft::decode(&damaged),
            Err(BuildDraftError::InvalidEnvelope)
        );
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later
use std::path::Path;

use nia_target_config::TargetConfig;

use crate::{
    BUILD_PLAN_SCHEMA_VERSION, BuildError, BuildInvocation, BuildStepSelection, OptimizationMode,
};

pub(crate) const RUNNER_CONFIG_SCHEMA_VERSION: u32 = 1;
pub(crate) const RUNNER_CONFIG_MAGIC: &[u8; 8] = b"NIARUNCF";
pub(crate) const RUNNER_CONFIG_MAGIC_TEXT: &str = "NIARUNCF";
pub(crate) const RUNNER_CONFIG_MAX_BYTES: usize = 1024 * 1024;

pub(crate) fn encode(invocation: &BuildInvocation) -> Result<Vec<u8>, BuildError> {
    let mut payload = Vec::new();
    write_path(&mut payload, "package root", &invocation.package_root)?;
    write_path(&mut payload, "build directory", &invocation.build_dir)?;
    write_path(&mut payload, "cache directory", &invocation.cache_dir)?;
    write_path(
        &mut payload,
        "toolchain executable",
        invocation.toolchain.compiler_executable(),
    )?;
    write_path(
        &mut payload,
        "toolchain resource root",
        invocation.toolchain.resource_root(),
    )?;
    write_target(&mut payload, invocation.toolchain.host_target())?;
    write_target(&mut payload, invocation.toolchain.artifact_target())?;
    write_u32(&mut payload, optimization_tag(invocation.optimization));
    write_u32(&mut payload, BUILD_PLAN_SCHEMA_VERSION);
    write_path(&mut payload, "build-plan draft", &invocation.plan_draft)?;
    match &invocation.step {
        BuildStepSelection::Default => payload.push(0),
        BuildStepSelection::Named(step) => {
            payload.push(1);
            write_text(&mut payload, "requested step", step)?;
        }
    }

    if payload.len() > RUNNER_CONFIG_MAX_BYTES - 24 || payload.len() > u32::MAX as usize {
        return Err(BuildError::RunnerConfigurationTooLarge { len: payload.len() });
    }
    let mut encoded = Vec::with_capacity(24 + payload.len());
    encoded.extend_from_slice(RUNNER_CONFIG_MAGIC);
    write_u32(&mut encoded, RUNNER_CONFIG_SCHEMA_VERSION);
    write_u32(&mut encoded, payload.len() as u32);
    write_u64(&mut encoded, payload_checksum(&payload));
    encoded.extend_from_slice(&payload);
    Ok(encoded)
}

fn write_target(encoded: &mut Vec<u8>, target: &TargetConfig) -> Result<(), BuildError> {
    for (role, value) in [
        ("target architecture", target.arch.as_str()),
        ("target vendor", target.vendor.as_str()),
        ("target operating system", target.os.as_str()),
        ("target environment", target.env.as_str()),
        ("target ABI", target.abi.as_str()),
        ("target endianness", target.endian.as_str()),
    ] {
        write_text(encoded, role, value)?;
    }
    write_u32(encoded, target.pointer_width);
    Ok(())
}

fn write_path(encoded: &mut Vec<u8>, role: &'static str, path: &Path) -> Result<(), BuildError> {
    let text = path.to_str().ok_or_else(|| BuildError::NonUtf8Path {
        role,
        path: path.to_path_buf(),
    })?;
    write_text(encoded, role, text)
}

fn write_text(encoded: &mut Vec<u8>, role: &'static str, value: &str) -> Result<(), BuildError> {
    if value.len() > u32::MAX as usize {
        return Err(BuildError::RunnerConfigurationFieldTooLarge {
            role,
            len: value.len(),
        });
    }
    write_u32(encoded, value.len() as u32);
    encoded.extend_from_slice(value.as_bytes());
    Ok(())
}

fn optimization_tag(optimization: OptimizationMode) -> u32 {
    match optimization {
        OptimizationMode::O0 => 0,
        OptimizationMode::O1 => 1,
        OptimizationMode::O2 => 2,
        OptimizationMode::O3 => 3,
        OptimizationMode::Os => 4,
        OptimizationMode::Oz => 5,
    }
}

fn write_u32(encoded: &mut Vec<u8>, value: u32) {
    encoded.extend_from_slice(&value.to_le_bytes());
}

fn write_u64(encoded: &mut Vec<u8>, value: u64) {
    encoded.extend_from_slice(&value.to_le_bytes());
}

fn payload_checksum(bytes: &[u8]) -> u64 {
    let mut first = 1u32;
    let mut second = 0u32;
    for byte in bytes {
        first = (first + u32::from(*byte)) % 65_521;
        second = (second + first) % 65_521;
    }
    (u64::from(second) << 32) | u64::from(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[derive(Debug, PartialEq, Eq)]
    struct DecodedConfig {
        package_root: String,
        build_dir: String,
        cache_dir: String,
        toolchain_executable: String,
        toolchain_resource_root: String,
        host_target: TargetConfig,
        artifact_target: TargetConfig,
        optimization: u32,
        plan_schema: u32,
        plan_draft: String,
        step: Option<String>,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum DecodeError {
        Magic,
        Version,
        Length,
        Checksum,
        Truncated,
        Utf8,
        Tag,
        Trailing,
    }

    #[test]
    fn round_trip_preserves_typed_runner_configuration() {
        assert_eq!(RUNNER_CONFIG_MAGIC_TEXT.as_bytes(), RUNNER_CONFIG_MAGIC);
        let invocation = invocation(BuildStepSelection::Named("install".to_string()));
        let decoded = decode(&encode(&invocation).expect("encode runner configuration"))
            .expect("decode runner configuration");

        assert_eq!(decoded.package_root, "/workspace/package");
        assert_eq!(decoded.build_dir, "/workspace/package/.nia-build");
        assert_eq!(decoded.cache_dir, "/workspace/package/.nia-cache");
        assert_eq!(decoded.host_target, *invocation.toolchain.host_target());
        assert_eq!(
            decoded.artifact_target,
            *invocation.toolchain.artifact_target()
        );
        assert_eq!(decoded.optimization, 5);
        assert_eq!(decoded.plan_schema, BUILD_PLAN_SCHEMA_VERSION);
        assert_eq!(decoded.step.as_deref(), Some("install"));
    }

    #[test]
    fn default_step_is_an_explicit_protocol_value() {
        let invocation = invocation(BuildStepSelection::Default);
        let decoded = decode(&encode(&invocation).expect("encode default selection"))
            .expect("decode default selection");
        assert_eq!(decoded.step, None);
    }

    #[test]
    fn decoder_rejects_header_payload_and_tag_damage() {
        let invocation = invocation(BuildStepSelection::Named("install".to_string()));
        let baseline = encode(&invocation).expect("encode runner configuration");

        let mut bad_magic = baseline.clone();
        bad_magic[0] ^= 1;
        assert_eq!(decode(&bad_magic), Err(DecodeError::Magic));

        let mut bad_version = baseline.clone();
        bad_version[8..12].copy_from_slice(&(RUNNER_CONFIG_SCHEMA_VERSION + 1).to_le_bytes());
        assert_eq!(decode(&bad_version), Err(DecodeError::Version));

        let mut bad_length = baseline.clone();
        bad_length[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&bad_length), Err(DecodeError::Length));

        let mut bad_checksum = baseline.clone();
        bad_checksum[16] ^= 1;
        assert_eq!(decode(&bad_checksum), Err(DecodeError::Checksum));

        assert_eq!(
            decode(&baseline[..baseline.len() - 1]),
            Err(DecodeError::Length)
        );

        let mut envelope_trailing = baseline.clone();
        envelope_trailing.push(0);
        assert_eq!(decode(&envelope_trailing), Err(DecodeError::Length));

        let mut payload_trailing = baseline.clone();
        payload_trailing.push(0);
        let payload_len = payload_trailing.len() - 24;
        payload_trailing[12..16].copy_from_slice(&(payload_len as u32).to_le_bytes());
        rewrite_checksum(&mut payload_trailing);
        assert_eq!(decode(&payload_trailing), Err(DecodeError::Trailing));

        let mut bad_utf8 = baseline.clone();
        bad_utf8[28] = 0xff;
        rewrite_checksum(&mut bad_utf8);
        assert_eq!(decode(&bad_utf8), Err(DecodeError::Utf8));

        let mut bad_tag = baseline.clone();
        let last_step_tag = locate_step_tag(&baseline);
        bad_tag[last_step_tag] = 2;
        rewrite_checksum(&mut bad_tag);
        assert_eq!(decode(&bad_tag), Err(DecodeError::Tag));
    }

    fn invocation(step: BuildStepSelection) -> BuildInvocation {
        let mut artifact_target = TargetConfig::host();
        artifact_target.arch = "artifact-arch".to_string();
        artifact_target.pointer_width = 32;
        let toolchain = crate::tests::test_toolchain_layout_for(artifact_target);
        BuildInvocation {
            toolchain: Arc::clone(&toolchain),
            package_root: PathBuf::from("/workspace/package"),
            build_script: PathBuf::from("/workspace/package/build.nia"),
            build_dir: PathBuf::from("/workspace/package/.nia-build"),
            cache_dir: PathBuf::from("/workspace/package/.nia-cache"),
            runner_dir: PathBuf::from("/workspace/package/.nia-build/runner"),
            runner_executable: PathBuf::from("/workspace/package/.nia-build/runner/runner"),
            runner_config: PathBuf::from("/workspace/package/.nia-build/.runner.config"),
            plan_draft: PathBuf::from("/workspace/package/.nia-build/.plan.draft"),
            plan_path: PathBuf::from("/workspace/package/.nia-build/build-plan.bin"),
            step,
            timings: nia_driver::TimingMode::Off,
            timing_format: nia_timing::TimingFormat::Text,
            max_parallel_actions: None,
            optimization: OptimizationMode::Oz,
        }
    }

    fn rewrite_checksum(encoded: &mut [u8]) {
        let checksum = payload_checksum(&encoded[24..]);
        encoded[16..24].copy_from_slice(&checksum.to_le_bytes());
    }

    fn locate_step_tag(encoded: &[u8]) -> usize {
        let mut cursor = Cursor::new(&encoded[24..]);
        for _ in 0..5 {
            let _ = cursor.text().unwrap();
        }
        let _ = cursor.target().unwrap();
        let _ = cursor.target().unwrap();
        let _ = cursor.u32().unwrap();
        let _ = cursor.u32().unwrap();
        let _ = cursor.text().unwrap();
        24 + cursor.position
    }

    fn decode(encoded: &[u8]) -> Result<DecodedConfig, DecodeError> {
        if encoded.len() < 24 {
            return Err(DecodeError::Truncated);
        }
        if &encoded[..8] != RUNNER_CONFIG_MAGIC {
            return Err(DecodeError::Magic);
        }
        let version = u32::from_le_bytes(encoded[8..12].try_into().unwrap());
        if version != RUNNER_CONFIG_SCHEMA_VERSION {
            return Err(DecodeError::Version);
        }
        let payload_len = u32::from_le_bytes(encoded[12..16].try_into().unwrap()) as usize;
        if encoded.len() > RUNNER_CONFIG_MAX_BYTES || encoded.len() != 24 + payload_len {
            return Err(DecodeError::Length);
        }
        let checksum = u64::from_le_bytes(encoded[16..24].try_into().unwrap());
        let payload = &encoded[24..];
        if checksum != payload_checksum(payload) {
            return Err(DecodeError::Checksum);
        }
        let mut cursor = Cursor::new(payload);
        let package_root = cursor.text()?;
        let build_dir = cursor.text()?;
        let cache_dir = cursor.text()?;
        let toolchain_executable = cursor.text()?;
        let toolchain_resource_root = cursor.text()?;
        let host_target = cursor.target()?;
        let artifact_target = cursor.target()?;
        let optimization = cursor.u32()?;
        if optimization > 5 {
            return Err(DecodeError::Tag);
        }
        let plan_schema = cursor.u32()?;
        let plan_draft = cursor.text()?;
        let step = match cursor.byte()? {
            0 => None,
            1 => Some(cursor.text()?),
            _ => return Err(DecodeError::Tag),
        };
        if cursor.position != payload.len() {
            return Err(DecodeError::Trailing);
        }
        Ok(DecodedConfig {
            package_root,
            build_dir,
            cache_dir,
            toolchain_executable,
            toolchain_resource_root,
            host_target,
            artifact_target,
            optimization,
            plan_schema,
            plan_draft,
            step,
        })
    }

    struct Cursor<'a> {
        bytes: &'a [u8],
        position: usize,
    }

    impl<'a> Cursor<'a> {
        fn new(bytes: &'a [u8]) -> Self {
            Self { bytes, position: 0 }
        }

        fn take(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
            if len > self.bytes.len().saturating_sub(self.position) {
                return Err(DecodeError::Truncated);
            }
            let start = self.position;
            self.position += len;
            Ok(&self.bytes[start..self.position])
        }

        fn byte(&mut self) -> Result<u8, DecodeError> {
            Ok(self.take(1)?[0])
        }

        fn u32(&mut self) -> Result<u32, DecodeError> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
        }

        fn text(&mut self) -> Result<String, DecodeError> {
            let len = self.u32()? as usize;
            std::str::from_utf8(self.take(len)?)
                .map(str::to_owned)
                .map_err(|_| DecodeError::Utf8)
        }

        fn target(&mut self) -> Result<TargetConfig, DecodeError> {
            Ok(TargetConfig {
                arch: self.text()?,
                vendor: self.text()?,
                os: self.text()?,
                env: self.text()?,
                abi: self.text()?,
                endian: self.text()?,
                pointer_width: self.u32()?,
            })
        }
    }
}

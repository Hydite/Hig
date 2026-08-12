use crate::{
    ArchiveFormat, BatchOptions, ChunkOptions, Compression, EncryptionMode, KdfProfile,
    ManifestFormat, PipelineOptions, ProjectMode, SerializablePackOptions, SolidMode, SpeedMode,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopPackRequest {
    pub input_dir: PathBuf,
    pub output_file: PathBuf,
    pub password: Option<String>,
    pub use_session: bool,
    pub encryption: EncryptionMode,
    pub speed: SpeedMode,
    pub cache_dir: Option<PathBuf>,
    pub threads: Option<usize>,
    pub level: Option<i32>,
    pub use_cache: bool,
    pub format: ArchiveFormat,
    pub manifest_format: ManifestFormat,
    pub batch: BatchOptions,
    pub chunk: ChunkOptions,
    pub kdf_profile: Option<KdfProfile>,
    pub trust_metadata: bool,
    pub project_mode: ProjectMode,
    pub solid: SolidMode,
}

impl DesktopPackRequest {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.input_dir.as_os_str().is_empty(),
            "input directory is required"
        );
        anyhow::ensure!(
            !self.output_file.as_os_str().is_empty(),
            "output archive is required"
        );
        if let Some(threads) = self.threads {
            anyhow::ensure!(
                (1..=1024).contains(&threads),
                "threads must be between 1 and 1024"
            );
        }
        if let Some(level) = self.level {
            anyhow::ensure!(
                (-7..=22).contains(&level),
                "zstd level must be between -7 and 22"
            );
        }
        if self.batch.enabled {
            anyhow::ensure!(
                self.batch.small_file_threshold > 0,
                "small-file threshold must be positive"
            );
            anyhow::ensure!(
                self.batch.max_batch_raw_bytes >= self.batch.small_file_threshold,
                "maximum batch bytes must be at least the small-file threshold"
            );
        }
        if self.chunk.enabled {
            anyhow::ensure!(
                (64 * 1024..=64 * 1024 * 1024).contains(&self.chunk.chunk_size),
                "chunk size must be between 64 KiB and 64 MiB"
            );
            anyhow::ensure!(
                self.chunk.chunk_file_threshold >= self.chunk.chunk_size,
                "chunk file threshold must be at least the chunk size"
            );
        }
        anyhow::ensure!(
            self.kdf_profile != Some(KdfProfile::FastBench),
            "fast-bench KDF is only available in diagnostics"
        );
        match self.encryption {
            EncryptionMode::None => anyhow::ensure!(
                self.password.is_none() && !self.use_session,
                "unencrypted archives cannot use a password or secure session"
            ),
            EncryptionMode::Password => anyhow::ensure!(
                self.password.is_some() || self.use_session,
                "password encryption requires a password or unlocked session"
            ),
        }
        anyhow::ensure!(
            !(self.password.is_some() && self.use_session),
            "choose either a password or an unlocked session"
        );
        Ok(())
    }

    pub fn resolved_kdf_profile(&self) -> KdfProfile {
        self.kdf_profile.unwrap_or(match self.speed {
            SpeedMode::Balanced => KdfProfile::Secure,
            SpeedMode::Fastest => KdfProfile::Interactive,
        })
    }

    pub fn serializable_options(&self) -> anyhow::Result<SerializablePackOptions> {
        self.validate()?;
        let pipeline = PipelineOptions {
            project_mode: self.project_mode,
            ..PipelineOptions::default()
        };
        Ok(SerializablePackOptions {
            input_dir: self.input_dir.clone(),
            output_file: self.output_file.clone(),
            encryption: self.encryption,
            threads: self.threads,
            compression: Compression::Zstd,
            level: self.level,
            use_cache: self.use_cache,
            trust_metadata: self.trust_metadata,
            format: self.format,
            batch: self.batch,
            chunk: self.chunk,
            speed: self.speed,
            kdf_profile: self.resolved_kdf_profile(),
            sealed_cache: self.speed == SpeedMode::Fastest,
            manifest_format: self.manifest_format,
            use_session: self.use_session,
            session_required: self.use_session,
            solid: self.solid,
            pipeline,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopUnpackRequest {
    pub archive_file: PathBuf,
    pub output_dir: PathBuf,
    pub password: Option<String>,
    pub overwrite: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> DesktopPackRequest {
        DesktopPackRequest {
            input_dir: PathBuf::from("input"),
            output_file: PathBuf::from("output.hig"),
            password: Some("secret".to_string()),
            use_session: false,
            encryption: EncryptionMode::Password,
            speed: SpeedMode::Balanced,
            cache_dir: None,
            threads: None,
            level: None,
            use_cache: true,
            format: ArchiveFormat::HigV2,
            manifest_format: ManifestFormat::Compact,
            batch: BatchOptions::default(),
            chunk: ChunkOptions::default(),
            kdf_profile: None,
            trust_metadata: false,
            project_mode: ProjectMode::Auto,
            solid: SolidMode::Auto,
        }
    }

    #[test]
    fn desktop_defaults_match_secure_cli_defaults() {
        let options = request().serializable_options().unwrap();
        assert_eq!(options.format, ArchiveFormat::HigV2);
        assert_eq!(options.manifest_format, ManifestFormat::Compact);
        assert_eq!(options.kdf_profile, KdfProfile::Secure);
        assert!(!options.trust_metadata);
        assert!(options.batch.enabled);
        assert!(options.chunk.enabled);
    }

    #[test]
    fn desktop_request_rejects_secret_conflicts_and_fast_bench() {
        let mut value = request();
        value.use_session = true;
        assert!(value.validate().is_err());
        value.password = None;
        value.kdf_profile = Some(KdfProfile::FastBench);
        assert!(value.validate().is_err());
    }

    #[test]
    fn desktop_request_rejects_invalid_thresholds() {
        let mut value = request();
        value.chunk.chunk_size = 0;
        assert!(value.validate().is_err());
        value.chunk = ChunkOptions::default();
        value.batch.max_batch_raw_bytes = 1;
        assert!(value.validate().is_err());
    }
}

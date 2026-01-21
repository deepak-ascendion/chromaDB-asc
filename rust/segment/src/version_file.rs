//! Version file management utilities for handling collection version files.
//!
//! This module provides a centralized way to download, upload, and manage
//! collection version files stored in S3/storage. Version files track the
//! evolution of segments within a collection.

use chroma_storage::{GetOptions, PutOptions, Storage, StorageError};
use chroma_types::chroma_proto::CollectionVersionFile;
use chroma_types::CollectionUuid;
use prost::Message;
use thiserror::Error;

/// Types of version file operations that determine the file naming convention.
#[derive(Debug, Clone, PartialEq)]
pub enum VersionFileType {
    /// Compaction operation - file name ends with _flush
    Compaction,
    /// Garbage collection operation - file name ends with _gc_mark
    GarbageCollection,
}

impl VersionFileType {
    /// Get the file suffix for the version file type
    pub fn suffix(&self) -> &'static str {
        match self {
            VersionFileType::Compaction => "flush",
            VersionFileType::GarbageCollection => "gc_mark",
        }
    }
}

/// Manager for version file operations including download, upload, and validation.
#[derive(Clone, Debug)]
pub struct VersionFileManager {
    storage: Storage,
}

/// Errors that can occur during version file operations.
#[derive(Error, Debug)]
pub enum VersionFileError {
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("Protobuf decode error: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("Invalid collection ID: {0}")]
    InvalidUuid(#[from] uuid::Error),
    #[error("Missing collection info in version file")]
    MissingCollectionInfo,
    #[error("Invalid version file path: {0}")]
    InvalidPath(String),
    #[error("Version file validation failed: {0}")]
    ValidationFailed(String),
}

impl VersionFileManager {
    /// Create a new VersionFileManager with the given storage backend.
    pub fn new(storage: Storage) -> Self {
        Self { storage }
    }

    /// Download and decode a version file from storage.
    ///
    /// # Arguments
    /// * `version_file_path` - The full path to the version file in storage (without bucket name)
    ///
    /// # Returns
    /// A decoded `CollectionVersionFile`
    pub async fn fetch_version_file(
        &self,
        version_file_path: &str,
    ) -> Result<CollectionVersionFile, VersionFileError> {
        if version_file_path.is_empty() {
            return Err(VersionFileError::InvalidPath(
                "Version file path cannot be empty".to_string(),
            ));
        }

        let content = self
            .storage
            .get(version_file_path, GetOptions::default())
            .await
            .map_err(|e| {
                tracing::error!(
                    error = ?e,
                    path = %version_file_path,
                    "Failed to fetch version file"
                );
                e
            })?;

        tracing::info!(
            path = %version_file_path,
            size = content.len(),
            "Successfully fetched version file"
        );

        let version_file = CollectionVersionFile::decode(content.as_slice())?;

        // Validate the version file
        self.validate_version_file(&version_file)?;

        Ok(version_file)
    }

    /// Encode and upload a version file to storage.
    ///
    /// # Arguments
    /// * `version_file` - The version file to upload
    /// * `collection` - The collection metadata for generating the path
    /// * `file_type` - The type of version file operation (COMPACTION or GARBAGE_COLLECTION)
    /// * `new_version_id` - The new version identifier
    ///
    /// # Returns
    /// The path where the version file was stored
    pub async fn upload_version_file(
        &self,
        version_file: &CollectionVersionFile,
        collection: &chroma_types::Collection,
        file_type: VersionFileType,
        new_version_id: &str,
    ) -> Result<String, VersionFileError> {
        // Generate the version file path from collection metadata
        let version_file_path = self.generate_version_file_path(
            &collection.tenant,
            &collection.database,
            &collection.collection_id,
            new_version_id,
            file_type,
        );

        // Encode the version file
        let content = version_file.encode_to_vec();
        let content_size = content.len();

        // Upload to storage
        self.storage
            .put_bytes(&version_file_path, content, PutOptions::default())
            .await
            .map_err(|e| {
                tracing::error!(
                    error = ?e,
                    path = %version_file_path,
                    size = content_size,
                    "Failed to upload version file"
                );
                e
            })?;

        tracing::info!(
            path = %version_file_path,
            size = content_size,
            "Successfully uploaded version file"
        );

        Ok(version_file_path)
    }

    /// Generate a standard version file path based on collection metadata.
    ///
    /// # Arguments
    /// * `tenant_id` - The tenant ID
    /// * `database_id` - The database ID
    /// * `collection_id` - The collection UUID
    /// * `version_id` - The version identifier (typically a UUID or timestamp)
    /// * `file_type` - The type of version file operation (COMPACTION or GARBAGE_COLLECTION)
    ///
    /// # Returns
    /// A formatted path matching Go implementation with appropriate suffix:
    /// - For COMPACTION: "tenant/{tenant_id}/database/{database_id}/collection/{collection_id}/versionfiles/{version_id}_flush"
    /// - For GARBAGE_COLLECTION: "tenant/{tenant_id}/database/{database_id}/collection/{collection_id}/versionfiles/{version_id}_gc_mark"
    fn generate_version_file_path(
        &self,
        tenant_id: &str,
        database_id: &str,
        collection_id: &CollectionUuid,
        version_id: &str,
        file_type: VersionFileType,
    ) -> String {
        format!(
            "tenant/{}/database/{}/collection/{}/versionfiles/{}_{}",
            tenant_id,
            database_id,
            collection_id,
            version_id,
            file_type.suffix()
        )
    }

    /// Validate that a version file contains the required fields and is well-formed.
    ///
    /// # Arguments
    /// * `version_file` - The version file to validate
    pub fn validate_version_file(
        &self,
        version_file: &CollectionVersionFile,
    ) -> Result<(), VersionFileError> {
        // Check that collection info exists
        if version_file.collection_info_immutable.is_none() {
            return Err(VersionFileError::MissingCollectionInfo);
        }

        let collection_info = version_file.collection_info_immutable.as_ref().unwrap();

        // Validate collection ID
        if collection_info.collection_id.is_empty() {
            return Err(VersionFileError::ValidationFailed(
                "Collection ID cannot be empty".to_string(),
            ));
        }

        // Validate tenant ID
        if collection_info.tenant_id.is_empty() {
            return Err(VersionFileError::ValidationFailed(
                "Tenant ID cannot be empty".to_string(),
            ));
        }

        // Validate database ID
        if collection_info.database_id.is_empty() {
            return Err(VersionFileError::ValidationFailed(
                "Database ID cannot be empty".to_string(),
            ));
        }

        // Check that version history exists
        if version_file.version_history.is_none() {
            return Err(VersionFileError::ValidationFailed(
                "Version history cannot be empty".to_string(),
            ));
        }

        // Validate collection name
        if collection_info.collection_name.is_empty() {
            return Err(VersionFileError::ValidationFailed(
                "Collection name cannot be empty".to_string(),
            ));
        }

        Ok(())
    }

    /// Get the collection ID from a version file.
    ///
    /// # Arguments
    /// * `version_file` - The version file to extract collection ID from
    ///
    /// # Returns
    /// The collection UUID
    pub fn extract_collection_id(
        &self,
        version_file: &CollectionVersionFile,
    ) -> Result<CollectionUuid, VersionFileError> {
        let collection_id_str = &version_file
            .collection_info_immutable
            .as_ref()
            .ok_or(VersionFileError::MissingCollectionInfo)?
            .collection_id;

        let collection_id = collection_id_str
            .parse()
            .map_err(VersionFileError::InvalidUuid)?;

        Ok(collection_id)
    }

    /// Get the tenant ID from a version file.
    ///
    /// # Arguments
    /// * `version_file` - The version file to extract tenant ID from
    ///
    /// # Returns
    /// The tenant ID string
    pub fn extract_tenant_id(
        &self,
        version_file: &CollectionVersionFile,
    ) -> Result<String, VersionFileError> {
        version_file
            .collection_info_immutable
            .as_ref()
            .ok_or(VersionFileError::MissingCollectionInfo)
            .map(|info| info.tenant_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chroma_config::Configurable;
    use chroma_storage::config::{LocalStorageConfig, StorageConfig};
    use chroma_types::chroma_proto::{
        CollectionInfoImmutable, CollectionVersionHistory, CollectionVersionInfo,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    async fn create_test_storage() -> (Storage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = StorageConfig::Local(LocalStorageConfig {
            root: temp_dir.path().to_string_lossy().to_string(),
        });
        let storage = Storage::try_from_config(&config, &chroma_config::registry::Registry::new())
            .await
            .unwrap();
        (storage, temp_dir)
    }

    fn create_test_version_file() -> CollectionVersionFile {
        let collection_id = Uuid::new_v4();
        let tenant_id = Uuid::new_v4().to_string();
        let database_id = Uuid::new_v4().to_string();

        CollectionVersionFile {
            collection_info_immutable: Some(CollectionInfoImmutable {
                tenant_id,
                database_id,
                database_name: "test_db".to_string(),
                is_deleted: false,
                dimension: 128,
                collection_id: collection_id.to_string(),
                collection_name: "test_collection".to_string(),
                collection_creation_secs: 1640995200, // 2022-01-01
            }),
            version_history: Some(CollectionVersionHistory { versions: vec![] }),
        }
    }

    fn create_test_collection() -> chroma_types::Collection {
        let collection_id = CollectionUuid(Uuid::new_v4());
        let tenant_id = Uuid::new_v4().to_string();
        let database_id = Uuid::new_v4().to_string();
        let database_uuid = chroma_types::DatabaseUuid(Uuid::new_v4());

        chroma_types::Collection {
            collection_id,
            name: "test_collection".to_string(),
            config: chroma_types::InternalCollectionConfiguration::default_hnsw(),
            schema: None,
            metadata: Some(std::collections::HashMap::new()),
            dimension: Some(128),
            tenant: tenant_id,
            database: database_id,
            log_position: 0,
            version: 1,
            total_records_post_compaction: 0,
            size_bytes_post_compaction: 0,
            last_compaction_time_secs: 0,
            version_file_path: None,
            root_collection_id: None,
            lineage_file_path: None,
            updated_at: std::time::SystemTime::now(),
            database_id: database_uuid,
            compaction_failure_count: 0,
        }
    }

    #[tokio::test]
    async fn test_upload_and_fetch_version_file() {
        let (storage, _temp_dir) = create_test_storage().await;
        let manager = VersionFileManager::new(storage);
        let version_file = create_test_version_file();
        let collection = create_test_collection();
        let version_id = "000001_test_version";

        // Upload the version file
        let uploaded_path = manager
            .upload_version_file(
                &version_file,
                &collection,
                VersionFileType::Compaction,
                version_id,
            )
            .await
            .unwrap();

        // Expected path should follow the new format
        let expected_path = format!(
            "tenant/{}/database/{}/collection/{}/versionfiles/{}_{}",
            collection.tenant,
            collection.database,
            collection.collection_id,
            version_id,
            VersionFileType::Compaction.suffix()
        );
        assert_eq!(uploaded_path, expected_path);

        // Fetch the version file
        let fetched_file = manager.fetch_version_file(&uploaded_path).await.unwrap();

        // Verify the content
        assert_eq!(
            fetched_file
                .collection_info_immutable
                .as_ref()
                .unwrap()
                .collection_id,
            version_file
                .collection_info_immutable
                .as_ref()
                .unwrap()
                .collection_id
        );
    }

    #[tokio::test]
    async fn test_fetch_nonexistent_version_file() {
        let (storage, _temp_dir) = create_test_storage().await;
        let manager = VersionFileManager::new(storage);

        let result = manager.fetch_version_file("nonexistent/file.binpb").await;
        assert!(matches!(result, Err(VersionFileError::Storage(_))));
    }

    #[tokio::test]
    async fn test_validate_version_file() {
        let (storage, _temp_dir) = create_test_storage().await;
        let manager = VersionFileManager::new(storage);

        // Valid version file should pass validation
        let valid_file = create_test_version_file();
        assert!(manager.validate_version_file(&valid_file).is_ok());

        // Invalid version file (missing collection info) should fail
        let mut invalid_file = create_test_version_file();
        invalid_file.collection_info_immutable = None;
        assert!(matches!(
            manager.validate_version_file(&invalid_file),
            Err(VersionFileError::MissingCollectionInfo)
        ));
    }

    #[tokio::test]
    async fn test_extract_collection_id() {
        let (storage, _temp_dir) = create_test_storage().await;
        let manager = VersionFileManager::new(storage);
        let version_file = create_test_version_file();

        let expected_id = version_file
            .collection_info_immutable
            .as_ref()
            .unwrap()
            .collection_id
            .parse::<Uuid>()
            .unwrap();

        let extracted_id = manager.extract_collection_id(&version_file).unwrap();
        assert_eq!(extracted_id, CollectionUuid(expected_id));
    }

    #[tokio::test]
    async fn test_empty_path_validation() {
        let (storage, _temp_dir) = create_test_storage().await;
        let manager = VersionFileManager::new(storage);

        let fetch_result = manager.fetch_version_file("").await;
        assert!(matches!(
            fetch_result,
            Err(VersionFileError::InvalidPath(_))
        ));
    }

    #[tokio::test]
    async fn test_upload_modify_and_reupload() {
        let (storage, _temp_dir) = create_test_storage().await;
        let manager = VersionFileManager::new(storage);
        let collection = create_test_collection();

        // Create initial version file
        let mut version_file = create_test_version_file();

        // Upload initial version
        let initial_version_id = "000001_test_version";
        let upload_result = manager
            .upload_version_file(
                &version_file,
                &collection,
                VersionFileType::Compaction,
                initial_version_id,
            )
            .await;
        assert!(upload_result.is_ok());
        let initial_path = upload_result.unwrap();

        // Download and validate
        let downloaded = manager.fetch_version_file(&initial_path).await.unwrap();
        assert_eq!(
            downloaded
                .collection_info_immutable
                .as_ref()
                .unwrap()
                .collection_id,
            version_file
                .collection_info_immutable
                .as_ref()
                .unwrap()
                .collection_id
        );
        assert_eq!(
            downloaded.version_history.as_ref().unwrap().versions.len(),
            version_file
                .version_history
                .as_ref()
                .unwrap()
                .versions
                .len()
        );

        // Modify the version file
        let original_version_count = version_file
            .version_history
            .as_ref()
            .unwrap()
            .versions
            .len();
        let new_version = CollectionVersionInfo {
            version: 999,
            created_at_secs: 1234567890,
            marked_for_deletion: false,
            ..Default::default()
        };
        version_file
            .version_history
            .as_mut()
            .unwrap()
            .versions
            .push(new_version);

        // Upload modified version with different version ID
        let modified_version_id = "000002_test_version";
        let reupload_result = manager
            .upload_version_file(
                &version_file,
                &collection,
                VersionFileType::Compaction,
                modified_version_id,
            )
            .await;
        assert!(reupload_result.is_ok());
        let modified_path = reupload_result.unwrap();

        // Validate that the modified version ID is used in the path
        let expected_modified_path = format!(
            "tenant/{}/database/{}/collection/{}/versionfiles/{}_{}",
            collection.tenant,
            collection.database,
            collection.collection_id,
            modified_version_id,
            VersionFileType::Compaction.suffix()
        );
        assert_eq!(modified_path, expected_modified_path);

        // Download modified version and validate changes
        let modified_downloaded = manager.fetch_version_file(&modified_path).await.unwrap();
        assert_eq!(
            modified_downloaded
                .collection_info_immutable
                .as_ref()
                .unwrap()
                .collection_id,
            version_file
                .collection_info_immutable
                .as_ref()
                .unwrap()
                .collection_id
        );
        assert_eq!(
            modified_downloaded
                .version_history
                .as_ref()
                .unwrap()
                .versions
                .len(),
            original_version_count + 1
        );
        assert_eq!(
            modified_downloaded
                .version_history
                .as_ref()
                .unwrap()
                .versions
                .last()
                .unwrap()
                .version,
            999
        );

        // Verify original file is unchanged
        let original_downloaded = manager.fetch_version_file(&initial_path).await.unwrap();
        assert_eq!(
            original_downloaded
                .version_history
                .as_ref()
                .unwrap()
                .versions
                .len(),
            original_version_count
        );
    }
}

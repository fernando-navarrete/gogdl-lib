//! Compile-time guard for the public enums that are `#[non_exhaustive]`.
//!
//! Outside the crate a `match` on these needs a wildcard arm, so `tests/public_types.rs` can no
//! longer notice a new variant. These matches have no wildcard: adding a variant stops this module
//! compiling until the change is looked at (documented, tested, and called out in `CHANGELOG.md`).
//! Nothing here runs.

use crate::{
    AuthError, ClientError, DepotError, DownloadError, DownloadEvent, DownloadStageEvent,
    FileAllocationEvent, FileSizeVerificationEvent, FileSystemError, GamesError, GogDlError,
    ProtonDownloadEvent, ProtonError, SavesDownloadEvent, SavesError, SavesUploadEvent,
    SecureLinksError, VerificationEvent,
};

#[allow(dead_code)]
#[allow(deprecated)]
fn download_event(e: &DownloadEvent) {
    match e {
        DownloadEvent::Preparing
        | DownloadEvent::Prepared
        | DownloadEvent::Started { .. }
        | DownloadEvent::Downloading
        | DownloadEvent::Progress(..)
        | DownloadEvent::ProgressRegression(..) => {}
    }
}

#[allow(dead_code)]
fn download_stage_event(e: &DownloadStageEvent) {
    match e {
        DownloadStageEvent::FileSizeVerificationStage(..)
        | DownloadStageEvent::FileAllocationStage(..)
        | DownloadStageEvent::VerificationStage(..)
        | DownloadStageEvent::FileAllocationError(..)
        | DownloadStageEvent::DownloadStage(..) => {}
    }
}

#[allow(dead_code)]
fn verification_event(e: &VerificationEvent) {
    match e {
        VerificationEvent::CouldNotResolvePath(..)
        | VerificationEvent::FileNotFound(..)
        | VerificationEvent::ChecksumMismatch(..)
        | VerificationEvent::Verified(..) => {}
    }
}

#[allow(dead_code)]
fn file_allocation_event(e: &FileAllocationEvent) {
    match e {
        FileAllocationEvent::FileWithNoChunks(..)
        | FileAllocationEvent::CouldNotResolvePath(..)
        | FileAllocationEvent::FileAllocationSuccess(..) => {}
    }
}

#[allow(dead_code)]
fn file_size_verification_event(e: &FileSizeVerificationEvent) {
    match e {
        FileSizeVerificationEvent::FileWithNoChunks(..)
        | FileSizeVerificationEvent::CouldNotResolvePath(..)
        | FileSizeVerificationEvent::FileNotFound(..)
        | FileSizeVerificationEvent::FileSizeVerificationFailed(..)
        | FileSizeVerificationEvent::FileSizeMismatch(..)
        | FileSizeVerificationEvent::FileSizeVerificationSuccess(..) => {}
    }
}

#[allow(dead_code)]
fn proton_download_event(e: &ProtonDownloadEvent) {
    match e {
        ProtonDownloadEvent::Downloading { .. }
        | ProtonDownloadEvent::Progress(..)
        | ProtonDownloadEvent::Extracted(..) => {}
    }
}

#[allow(dead_code)]
fn saves_download_event(e: &SavesDownloadEvent) {
    match e {
        SavesDownloadEvent::Preparing { .. }
        | SavesDownloadEvent::FileStarted { .. }
        | SavesDownloadEvent::Progress(..)
        | SavesDownloadEvent::FileFinished { .. } => {}
    }
}

#[allow(dead_code)]
fn saves_upload_event(e: &SavesUploadEvent) {
    match e {
        SavesUploadEvent::Preparing { .. }
        | SavesUploadEvent::FileStarted { .. }
        | SavesUploadEvent::Progress(..)
        | SavesUploadEvent::FileFinished { .. } => {}
    }
}

#[allow(dead_code)]
fn file_system_error(e: &FileSystemError) {
    match e {
        FileSystemError::PathResolverCreationError(..)
        | FileSystemError::FileMetadataError(..)
        | FileSystemError::FileCreationError(..)
        | FileSystemError::PathResolutionError(..)
        | FileSystemError::FileOpenError(..)
        | FileSystemError::FileRemovalError(..)
        | FileSystemError::NoDiskMatchingPath(..) => {}
    }
}

#[allow(dead_code)]
fn download_error(e: &DownloadError) {
    match e {
        DownloadError::UrlParseError(..)
        | DownloadError::NetworkError(..)
        | DownloadError::Http { .. }
        | DownloadError::DecodeError(..)
        | DownloadError::DeflateError(..)
        | DownloadError::GamesError(..)
        | DownloadError::DepotError(..)
        | DownloadError::BuildNotFound
        | DownloadError::FileAllocationError
        | DownloadError::SecureLinksError(..)
        | DownloadError::FileSystemError(..)
        | DownloadError::ClientError(..)
        | DownloadError::ChunkIntegrityCheckFailed(..)
        | DownloadError::CouldNotResolveFreeSpace { .. }
        | DownloadError::NotEnoughFreeSpace { .. }
        | DownloadError::ChunkHashMismatch { .. } => {}
    }
}

#[allow(dead_code)]
fn proton_error(e: &ProtonError) {
    match e {
        ProtonError::ClientError(..)
        | ProtonError::NoSuitableAsset(..)
        | ProtonError::Io(..)
        | ProtonError::ExtractionError(..)
        | ProtonError::FileSystemError(..)
        | ProtonError::CouldNotResolveFreeSpace { .. }
        | ProtonError::NotEnoughFreeSpace { .. } => {}
    }
}

#[allow(dead_code)]
fn saves_error(e: &SavesError) {
    match e {
        SavesError::ClientError(..)
        | SavesError::GamesError(..)
        | SavesError::DepotError(..)
        | SavesError::BuildNotFound
        | SavesError::CloudStorageNotSupported
        | SavesError::Io(..)
        | SavesError::FileSystemError(..)
        | SavesError::HashMismatch { .. }
        | SavesError::InvalidHeader(..)
        | SavesError::WineUserDirNotFound(..)
        | SavesError::UnknownSaveLocationVariable(..)
        | SavesError::InvalidSaveLocation(..)
        | SavesError::InvalidSaveFileName(..) => {}
    }
}

#[allow(dead_code)]
fn gogdl_error(e: &GogDlError) {
    match e {
        GogDlError::GameError(..)
        | GogDlError::DepotError(..)
        | GogDlError::SecureLinksError(..)
        | GogDlError::DownloadError(..)
        | GogDlError::ClientError(..)
        | GogDlError::ProtonError(..) => {}
    }
}

#[allow(dead_code)]
fn client_error(e: &ClientError) {
    match e {
        ClientError::UrlParseError(..)
        | ClientError::NetworkError(..)
        | ClientError::HttpError { .. }
        | ClientError::DeserializationError(..)
        | ClientError::DecodeError(..)
        | ClientError::ChunkStreamCallbackError(..)
        | ClientError::MaxRetriesReached
        | ClientError::AuthError(..) => {}
    }
}

#[allow(dead_code)]
fn auth_error(e: &AuthError) {
    match e {
        AuthError::NotAuthenticated
        | AuthError::TokenExpired
        | AuthError::AuthDecodeError(..)
        | AuthError::AuthEncodeError(..)
        | AuthError::ClientError { .. } => {}
    }
}

#[allow(dead_code)]
fn games_error(e: &GamesError) {
    match e {
        GamesError::UrlParseError(..)
        | GamesError::NetworkError(..)
        | GamesError::Http { .. }
        | GamesError::DecodeError(..)
        | GamesError::DeflateError(..)
        | GamesError::ProductNotAGame
        | GamesError::ClientError(..) => {}
    }
}

#[allow(dead_code)]
fn depot_error(e: &DepotError) {
    match e {
        DepotError::UrlParseError(..)
        | DepotError::NetworkError(..)
        | DepotError::Http { .. }
        | DepotError::DecodeError(..)
        | DepotError::DeflateError(..)
        | DepotError::ClientError(..) => {}
    }
}

#[allow(dead_code)]
fn secure_links_error(e: &SecureLinksError) {
    match e {
        SecureLinksError::UrlParseError(..)
        | SecureLinksError::NetworkError(..)
        | SecureLinksError::Http { .. }
        | SecureLinksError::DecodeError(..)
        | SecureLinksError::DeflateError(..)
        | SecureLinksError::GamesError(..)
        | SecureLinksError::ProductNotOwned(..)
        | SecureLinksError::NoSecureLink
        | SecureLinksError::ClientError(..)
        | SecureLinksError::IncorrectGameId(..) => {}
    }
}

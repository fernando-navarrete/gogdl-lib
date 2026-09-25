//! Builds every type the bridge's model adapters need from outside the crate,
//! so this file sees only the public API. The structs come from captured
//! responses through `serde_json`; the event enums through their variants.
//!
//! Each enum helper matches exhaustively on purpose: a new variant is a
//! public API change, and this stops compiling until it is looked at.

use gogdl_lib::{
    DownloadEvent, DownloadStageEvent, FileAllocationEvent, FileSizeVerificationEvent, GameBuild,
    GameBuilds, ProductDetails, ProtonDownloadEvent, ProtonGeRelease, ProtonGeReleasesPage,
    SavesDownloadEvent, SavesUploadEvent, VerificationEvent,
};
use std::path::PathBuf;

#[test]
fn game_builds_deserialize_from_a_captured_listing() {
    let builds: GameBuilds =
        serde_json::from_str(include_str!("fixtures/game_builds.json")).unwrap();
    assert_eq!(builds.count, 2);
    assert_eq!(builds.items.len(), 2);
    // `game_title` is `#[serde(skip)]`, so it is always empty.
    assert_eq!(builds.game_title, "");
    let newest = &builds.items[0];
    assert_eq!(newest.build_id, "58989493373906337");
    assert_eq!(newest.version_name, "2.31a");
    assert_eq!(
        newest.date_published.to_rfc3339(),
        "2025-09-16T13:04:39+00:00"
    );
    assert!(
        newest
            .link
            .starts_with("https://downloadable-manifests-collector.gog.com/")
    );
}

#[test]
fn a_game_build_deserializes_on_its_own() {
    let build: GameBuild = serde_json::from_str(
        r#"{"build_id": "1", "version_name": "1.0", "date_published": "2025-01-02T03:04:05+0000",
            "link": "https://example.invalid/m"}"#,
    )
    .unwrap();
    assert_eq!(build.build_id, "1");
    assert_eq!(build.version_name, "1.0");
}

#[test]
fn product_details_deserialize_and_leave_the_title_to_the_caller() {
    let mut game: ProductDetails =
        serde_json::from_str(include_str!("fixtures/product_details_game.json")).unwrap();
    assert_eq!(game.get_product_type(), "GAME");
    assert_eq!(game.get_product_id(), 1423049311);
    // `title` is `#[serde(skip)]`: only `get_product_details` fills it.
    assert_eq!(game.title, "");
    game.title = "Cyberpunk 2077".into();
    assert_eq!(game.title, "Cyberpunk 2077");

    let dlc: ProductDetails =
        serde_json::from_str(include_str!("fixtures/product_details_dlc.json")).unwrap();
    assert_eq!(dlc.get_product_type(), "DLC");
    assert_eq!(dlc.get_product_id(), 1288586309);
}

#[test]
fn proton_releases_deserialize_from_captured_responses() {
    let release: ProtonGeRelease =
        serde_json::from_str(include_str!("fixtures/proton_ge_release.json")).unwrap();
    assert_eq!(release.tag_name, "GE-Proton11-7");
    assert_eq!(
        release.get_suitable_asset().unwrap().name,
        "GE-Proton11-7-x86_64.tar.gz"
    );

    let page: ProtonGeReleasesPage =
        serde_json::from_str(include_str!("fixtures/proton_ge_releases_page.json")).unwrap();
    assert_eq!(page.releases().len(), 2);
}

#[test]
fn download_events_can_be_built_and_matched() {
    fn label(event: &DownloadEvent) -> &'static str {
        match event {
            DownloadEvent::Preparing => "preparing",
            DownloadEvent::Prepared => "prepared",
            DownloadEvent::Downloading => "downloading",
            DownloadEvent::Progress(_) => "progress",
            DownloadEvent::ProgressRegression(_) => "regression",
        }
    }
    let events = [
        DownloadEvent::Preparing,
        DownloadEvent::Prepared,
        DownloadEvent::Downloading,
        DownloadEvent::Progress(1),
        DownloadEvent::ProgressRegression(1),
    ];
    let labels: Vec<_> = events.iter().map(label).collect();
    assert_eq!(
        labels,
        [
            "preparing",
            "prepared",
            "downloading",
            "progress",
            "regression"
        ]
    );
}

#[test]
fn stage_events_can_wrap_every_inner_event() {
    fn label(event: &DownloadStageEvent) -> &'static str {
        match event {
            DownloadStageEvent::FileSizeVerificationStage(inner) => match inner {
                FileSizeVerificationEvent::FileWithNoChunks(..) => "size/no-chunks",
                FileSizeVerificationEvent::CouldNotResolvePath(..) => "size/no-path",
                FileSizeVerificationEvent::FileNotFound(..) => "size/not-found",
                FileSizeVerificationEvent::FileSizeVerificationFailed(..) => "size/failed",
                FileSizeVerificationEvent::FileSizeMismatch(..) => "size/mismatch",
                FileSizeVerificationEvent::FileSizeVerificationSuccess(..) => "size/ok",
            },
            DownloadStageEvent::FileAllocationStage(inner) => match inner {
                FileAllocationEvent::FileWithNoChunks(..) => "alloc/no-chunks",
                FileAllocationEvent::CouldNotResolvePath(..) => "alloc/no-path",
                FileAllocationEvent::FileAllocationSuccess(..) => "alloc/ok",
            },
            DownloadStageEvent::VerificationStage(inner) => match inner {
                VerificationEvent::CouldNotResolvePath(..) => "verify/no-path",
                VerificationEvent::FileNotFound(..) => "verify/not-found",
                VerificationEvent::ChecksumMismatch(..) => "verify/mismatch",
                VerificationEvent::Verified(..) => "verify/ok",
            },
            DownloadStageEvent::FileAllocationError() => "alloc-error",
            DownloadStageEvent::DownloadStage(_) => "download",
        }
    }
    let p = || "a/b".to_string();
    let events = [
        DownloadStageEvent::FileSizeVerificationStage(FileSizeVerificationEvent::FileWithNoChunks(
            p(),
            0,
        )),
        DownloadStageEvent::FileSizeVerificationStage(
            FileSizeVerificationEvent::CouldNotResolvePath(p(), 1),
        ),
        DownloadStageEvent::FileSizeVerificationStage(FileSizeVerificationEvent::FileNotFound(
            p(),
            1,
        )),
        DownloadStageEvent::FileSizeVerificationStage(
            FileSizeVerificationEvent::FileSizeVerificationFailed(p(), 1),
        ),
        DownloadStageEvent::FileSizeVerificationStage(FileSizeVerificationEvent::FileSizeMismatch(
            p(),
            1,
        )),
        DownloadStageEvent::FileSizeVerificationStage(
            FileSizeVerificationEvent::FileSizeVerificationSuccess(p(), 1),
        ),
        DownloadStageEvent::FileAllocationStage(FileAllocationEvent::FileWithNoChunks(p(), 0)),
        DownloadStageEvent::FileAllocationStage(FileAllocationEvent::CouldNotResolvePath(p(), 1)),
        DownloadStageEvent::FileAllocationStage(FileAllocationEvent::FileAllocationSuccess(p(), 1)),
        DownloadStageEvent::VerificationStage(VerificationEvent::CouldNotResolvePath(p(), 1)),
        DownloadStageEvent::VerificationStage(VerificationEvent::FileNotFound(p(), 1)),
        DownloadStageEvent::VerificationStage(VerificationEvent::ChecksumMismatch(p(), 1)),
        DownloadStageEvent::VerificationStage(VerificationEvent::Verified(p(), 1)),
        DownloadStageEvent::FileAllocationError(),
        DownloadStageEvent::DownloadStage(DownloadEvent::Progress(1)),
    ];
    let labels: Vec<_> = events.iter().map(label).collect();
    assert_eq!(labels.len(), 15);
    assert_eq!(labels[5], "size/ok");
    assert_eq!(labels[12], "verify/ok");
    assert_eq!(labels[14], "download");
}

#[test]
fn proton_download_events_can_be_built_and_matched() {
    let events = [
        ProtonDownloadEvent::Downloading { total_bytes: 10 },
        ProtonDownloadEvent::Progress(4),
        ProtonDownloadEvent::Extracted("bin/proton".into()),
    ];
    for event in &events {
        match event {
            ProtonDownloadEvent::Downloading { total_bytes } => assert_eq!(*total_bytes, 10),
            ProtonDownloadEvent::Progress(n) => assert_eq!(*n, 4),
            ProtonDownloadEvent::Extracted(path) => assert_eq!(path, "bin/proton"),
        }
    }
}

#[test]
fn saves_download_events_can_be_built_and_matched() {
    let events = [
        SavesDownloadEvent::Preparing {
            total_files: 1,
            total_bytes: 9,
        },
        SavesDownloadEvent::FileStarted {
            name: "slot".into(),
            destination: PathBuf::from("/tmp/slot"),
            total_bytes: 9,
        },
        SavesDownloadEvent::Progress(9),
        SavesDownloadEvent::FileFinished {
            name: "slot".into(),
        },
    ];
    let labels: Vec<_> = events
        .iter()
        .map(|e| match e {
            SavesDownloadEvent::Preparing { .. } => "preparing",
            SavesDownloadEvent::FileStarted { .. } => "started",
            SavesDownloadEvent::Progress(_) => "progress",
            SavesDownloadEvent::FileFinished { .. } => "finished",
        })
        .collect();
    assert_eq!(labels, ["preparing", "started", "progress", "finished"]);
}

#[test]
fn saves_upload_events_can_be_built_and_matched() {
    let events = [
        SavesUploadEvent::Preparing { total_files: 1 },
        SavesUploadEvent::FileStarted {
            name: "slot".into(),
            source: PathBuf::from("/tmp/slot"),
            total_bytes: 9,
        },
        SavesUploadEvent::Progress(9),
        SavesUploadEvent::FileFinished {
            name: "slot".into(),
        },
    ];
    let labels: Vec<_> = events
        .iter()
        .map(|e| match e {
            SavesUploadEvent::Preparing { .. } => "preparing",
            SavesUploadEvent::FileStarted { .. } => "started",
            SavesUploadEvent::Progress(_) => "progress",
            SavesUploadEvent::FileFinished { .. } => "finished",
        })
        .collect();
    assert_eq!(labels, ["preparing", "started", "progress", "finished"]);
}

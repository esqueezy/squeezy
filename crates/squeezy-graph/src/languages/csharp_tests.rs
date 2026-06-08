use squeezy_core::{ContentHash, FileId, Freshness, LanguageKind};
use squeezy_workspace::FileRecord;

use super::*;

fn csharp_file_record(relative_path: &str) -> FileRecord {
    FileRecord {
        id: FileId::new(relative_path),
        path: std::path::PathBuf::from(relative_path),
        relative_path: relative_path.to_string(),
        hash: ContentHash::new(""),
        size_bytes: 0,
        modified_unix_millis: 0,
        language: LanguageKind::CSharp,
        freshness: Freshness::Fresh,
    }
}

#[test]
fn dotnet_metadata_provider_canonical_casing() {
    assert_eq!(
        dotnet_project_metadata_provider(&csharp_file_record("MyApp/MyApp.csproj")),
        Some("csproj")
    );
    assert_eq!(
        dotnet_project_metadata_provider(&csharp_file_record("MyApp.sln")),
        Some("sln")
    );
    assert_eq!(
        dotnet_project_metadata_provider(&csharp_file_record("Directory.Build.props")),
        Some("directory-build-props")
    );
    assert_eq!(
        dotnet_project_metadata_provider(&csharp_file_record("global.json")),
        Some("global-json")
    );
}

#[test]
fn dotnet_metadata_provider_case_insensitive_windows_spellings() {
    // Windows MSBuild conventions allow any casing; the provider must
    // recognise these without requiring an exact match.
    assert_eq!(
        dotnet_project_metadata_provider(&csharp_file_record("src/APP.CSPROJ")),
        Some("csproj"),
    );
    assert_eq!(
        dotnet_project_metadata_provider(&csharp_file_record("Solution.SLN")),
        Some("sln"),
    );
    assert_eq!(
        dotnet_project_metadata_provider(&csharp_file_record("directory.build.props")),
        Some("directory-build-props"),
    );
    assert_eq!(
        dotnet_project_metadata_provider(&csharp_file_record("DIRECTORY.BUILD.TARGETS")),
        Some("directory-build-targets"),
    );
    assert_eq!(
        dotnet_project_metadata_provider(&csharp_file_record("GLOBAL.JSON")),
        Some("global-json"),
    );
    assert_eq!(
        dotnet_project_metadata_provider(&csharp_file_record("packages.LOCK.JSON")),
        Some("packages-lock"),
    );
    assert_eq!(
        dotnet_project_metadata_provider(&csharp_file_record("MyApp.SLNX")),
        Some("slnx"),
    );
    // Non-metadata files must still return None.
    assert!(dotnet_project_metadata_provider(&csharp_file_record("src/Program.cs")).is_none());
}

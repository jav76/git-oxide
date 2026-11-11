use serde::{Deserialize, Serialize};

/// Number of days in a standard calendar week for the 52-week commit heatmap.
pub const DAYS_PER_WEEK: usize = 7;

/// Number of weeks displayed in the commit activity heatmap.
pub const HEATMAP_WEEKS: usize = 52;

/// RGBA color value used for syntax highlighting tokens and language badges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbaColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl RgbaColor {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
}

/// A contiguous slice of a single source line sharing an identical foreground RGBA color.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeTokenSpan {
    pub text: String,
    pub color: RgbaColor,
}

/// Classification of a line inside a Unified or Side-by-Side diff view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    HunkHeader,
    EmptyPlaceholder,
}

impl DiffLineKind {
    pub const fn as_i32(self) -> i32 {
        match self {
            Self::Context => 0,
            Self::Added => 1,
            Self::Removed => 2,
            Self::HunkHeader => 3,
            Self::EmptyPlaceholder => 4,
        }
    }
}

/// A single virtualized diff row holding both Unified and Side-by-Side (Split) representations.
#[derive(Debug, Clone)]
pub struct DiffLineRow {
    pub old_line_no: Option<u32>,
    pub new_line_no: Option<u32>,
    pub kind: DiffLineKind,
    pub unified_spans: Vec<CodeTokenSpan>,
    pub left_line_no: Option<u32>,
    pub left_kind: DiffLineKind,
    pub left_spans: Vec<CodeTokenSpan>,
    pub right_line_no: Option<u32>,
    pub right_kind: DiffLineKind,
    pub right_spans: Vec<CodeTokenSpan>,
}

/// Kind of Git reference selectable as a `Ref Target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefKind {
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
}

/// Selectable Git reference (`HEAD`, branch, or tag) resolved to a commit OID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefItem {
    pub display_name: String,
    pub full_ref_name: String,
    pub target_oid_hex: String,
    pub kind: RefKind,
}

/// Summary metadata and churn counts for a single commit in the commit graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitSummary {
    pub oid_hex: String,
    pub short_oid: String,
    pub summary: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub timestamp_secs: i64,
    pub date_iso: String,
    pub parent_count: u32,
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
    pub churn_indexed: bool,
}

/// Per-file churn recorded inside a cached commit churn entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChurnEntry {
    pub path: String,
    pub insertions: u32,
    pub deletions: u32,
}

/// Immutable per-commit churn statistics persisted in the `redb` `OID Cache`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitChurnStats {
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
    pub touched_paths: Vec<FileChurnEntry>,
}

/// Immutable per-blob line counts persisted in the `redb` `OID Cache`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageBlobStats {
    pub code: u32,
    pub comments: u32,
    pub blanks: u32,
}

/// Aggregated language statistics across all files in the `Ref Target` tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageStat {
    pub language: String,
    pub files: u32,
    pub code: u64,
    pub comments: u64,
    pub blanks: u64,
    pub bytes: u64,
    pub percentage: f32,
    pub color: RgbaColor,
}

/// Aggregated commit and line churn metrics for a repository contributor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContributorStat {
    pub name: String,
    pub email: String,
    pub commits: u32,
    pub insertions: u64,
    pub deletions: u64,
    pub first_commit_date: String,
    pub last_commit_date: String,
}

/// A `File Hotspot` ranked by cumulative commit touches and line churn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHotspot {
    pub path: String,
    pub language: String,
    pub touches: u32,
    pub insertions: u64,
    pub deletions: u64,
    pub total_churn: u64,
}

/// A single day cell in the 52-week commit activity heatmap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatmapDay {
    pub date_iso: String,
    pub commit_count: u32,
    pub intensity_level: u8,
}

/// One 7-day column in the 52-week commit activity heatmap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatmapWeek {
    pub week_label: String,
    pub days: [HeatmapDay; DAYS_PER_WEEK],
}

/// Time-bucketed commit velocity and code churn for timeline plotting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChurnTimeBucket {
    pub period_label: String,
    pub start_timestamp: i64,
    pub commits: u32,
    pub insertions: u64,
    pub deletions: u64,
    pub net_lines: i64,
}

/// File modification type within a single commit diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChangedFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

impl ChangedFileStatus {
    pub const fn badge_str(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
        }
    }
}

/// Summary of a single changed file in a selected commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangedFileStat {
    pub path: String,
    pub old_path: Option<String>,
    pub status: ChangedFileStatus,
    pub insertions: u32,
    pub deletions: u32,
}

/// A file entry inside the repository tree browser at a given `Ref Target`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeEntryItem {
    pub path: String,
    pub file_name: String,
    pub depth: u32,
    pub language: String,
    pub size_bytes: u64,
    pub blob_oid_hex: String,
}

/// A single line in the `Files & Blame` view pairing blame metadata with `Coalesced Token Spans`.
#[derive(Debug, Clone)]
pub struct BlameLineRow {
    pub line_no: u32,
    pub commit_oid_hex: String,
    pub short_oid: String,
    pub author_name: String,
    pub date_iso: String,
    pub summary: String,
    pub is_hunk_start: bool,
    pub spans: Vec<CodeTokenSpan>,
}

/// Result of a fast `Phase 1 Snapshot` scan.
#[derive(Debug, Clone)]
pub struct Phase1Snapshot {
    pub repo_path: String,
    pub repo_name: String,
    pub active_ref_name: String,
    pub active_ref_oid_hex: String,
    pub refs: Vec<RefItem>,
    pub commits: Vec<CommitSummary>,
    pub languages: Vec<LanguageStat>,
    pub contributors: Vec<ContributorStat>,
    pub heatmap_weeks: Vec<HeatmapWeek>,
    pub hotspots: Vec<FileHotspot>,
    pub weekly_churn: Vec<ChurnTimeBucket>,
    pub monthly_churn: Vec<ChurnTimeBucket>,
    pub total_commits: u32,
    pub indexed_churn_commits: u32,
}

/// Incremental update emitted by the background `Phase 2 Churn Index` worker.
#[derive(Debug, Clone)]
pub struct ChurnBatchProgress {
    pub indexed_commits: u32,
    pub total_commits: u32,
    pub updated_commits: Vec<(String, u32, u32, u32)>,
    pub contributors: Vec<ContributorStat>,
    pub hotspots: Vec<FileHotspot>,
    pub weekly_churn: Vec<ChurnTimeBucket>,
    pub monthly_churn: Vec<ChurnTimeBucket>,
    pub is_complete: bool,
}

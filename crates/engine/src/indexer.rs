use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use gix::ObjectId;
use rayon::prelude::*;
use tokei::{Config as TokeiConfig, LanguageType};

use crate::cache::{OID_BYTE_LEN, OidCache};
use crate::diff::{compute_commit_churn_entries, parse_oid_hex};
use crate::error::EngineError;
use crate::models::{
    ChurnBatchProgress, ChurnTimeBucket, CommitChurnStats, CommitSummary, ContributorStat, DAYS_PER_WEEK,
    FileChurnEntry, FileHotspot, HEATMAP_WEEKS, HeatmapDay, HeatmapWeek, LanguageBlobStats, LanguageStat,
    Phase1Snapshot, RefItem, RefKind, TreeEntryItem,
};
use crate::tree_blame::{
    classify_language_name, format_date_iso, language_badge_color, list_tree_entries, shorten_oid_str,
};

/// Number of commits processed per `Phase 2 Churn Index` batch before persisting to `redb`.
pub const CHURN_BATCH_SIZE: usize = 250;

/// Maximum number of `File Hotspot` entries returned to the UI.
pub const MAX_HOTSPOTS: usize = 100;

/// Seconds in one standard UTC day.
pub const SECONDS_PER_DAY: i64 = 86_400;

/// Converts a `gix::ObjectId` into a 20-byte array suitable for `redb` table keys.
pub fn oid_to_bytes(oid: ObjectId) -> [u8; OID_BYTE_LEN] {
    let slice: &[u8] = oid.as_bytes();
    let mut bytes: [u8; OID_BYTE_LEN] = [0u8; OID_BYTE_LEN];
    let copy_len: usize = slice.len().min(OID_BYTE_LEN);
    bytes[..copy_len].copy_from_slice(&slice[..copy_len]);
    bytes
}

/// Executes the fast `Phase 1 Snapshot` scan for a repository and `Ref Target`.
pub fn run_phase1(
    repo_path: &Path,
    target_ref: Option<&str>,
    cache: &OidCache,
    cancel_flag: &AtomicBool,
) -> Result<Phase1Snapshot, EngineError> {
    let start_time: Instant = Instant::now();
    let repo: gix::Repository = gix::discover(repo_path).map_err(|error| EngineError::RepositoryOpen {
        path: repo_path.display().to_string(),
        message: error.to_string(),
    })?;

    let workdir_or_git: &Path = repo.work_dir().unwrap_or_else(|| repo.path());
    let repo_display_path: String = workdir_or_git.display().to_string();
    let repo_name: String =
        workdir_or_git.file_name().and_then(|name| name.to_str()).unwrap_or("repository").to_owned();

    let refs: Vec<RefItem> = collect_repository_refs(&repo);
    let (active_ref_name, active_ref_oid) = resolve_active_ref(&repo, target_ref, &refs)?;
    let active_ref_oid_hex: String = active_ref_oid.to_hex().to_string();

    if cancel_flag.load(Ordering::Relaxed) {
        return Err(EngineError::Cancelled);
    }

    let mut commits: Vec<CommitSummary> = walk_commit_graph(&repo, active_ref_oid, cancel_flag)?;
    let commit_oid_keys: Vec<[u8; OID_BYTE_LEN]> =
        commits.iter().filter_map(|commit| parse_oid_hex(&commit.oid_hex).ok().map(oid_to_bytes)).collect();

    let cached_churn_map: HashMap<[u8; OID_BYTE_LEN], CommitChurnStats> =
        cache.get_commit_churn_batch(&commit_oid_keys)?;

    let mut indexed_churn_commits: u32 = 0;
    let mut churn_by_oid_hex: HashMap<String, CommitChurnStats> = HashMap::with_capacity(cached_churn_map.len());

    for commit in &mut commits {
        if let Ok(oid) = parse_oid_hex(&commit.oid_hex) {
            let key: [u8; OID_BYTE_LEN] = oid_to_bytes(oid);
            if let Some(churn) = cached_churn_map.get(&key) {
                commit.files_changed = churn.files_changed;
                commit.insertions = churn.insertions;
                commit.deletions = churn.deletions;
                commit.churn_indexed = true;
                indexed_churn_commits += 1;
                churn_by_oid_hex.insert(commit.oid_hex.clone(), churn.clone());
            }
        }
    }

    if cancel_flag.load(Ordering::Relaxed) {
        return Err(EngineError::Cancelled);
    }

    let languages: Vec<LanguageStat> = compute_ref_languages(&repo, &active_ref_oid_hex, cache, cancel_flag)?;
    let contributors: Vec<ContributorStat> = build_contributor_stats(&commits);
    let heatmap_weeks: Vec<HeatmapWeek> = build_heatmap_weeks(&commits);
    let hotspots: Vec<FileHotspot> = build_file_hotspots(&churn_by_oid_hex);
    let (weekly_churn, monthly_churn) = build_churn_buckets(&commits);
    let total_commits: u32 = commits.len() as u32;

    let elapsed_ms: u128 = start_time.elapsed().as_millis();
    log::info!(
        "completed Phase 1 snapshot for {repo_display_path} at {active_ref_name} ({total_commits} commits, {indexed_churn_commits} cached in {elapsed_ms} ms)"
    );

    Ok(Phase1Snapshot {
        repo_path: repo_display_path,
        repo_name,
        active_ref_name,
        active_ref_oid_hex,
        refs,
        commits,
        languages,
        contributors,
        heatmap_weeks,
        hotspots,
        weekly_churn,
        monthly_churn,
        total_commits,
        indexed_churn_commits,
    })
}

/// Runs the background `Phase 2 Churn Index` over all commits missing from the `OID Cache`.
pub fn run_phase2_churn<F>(
    repo_path: &Path,
    snapshot: &Phase1Snapshot,
    cache: &OidCache,
    cancel_flag: &AtomicBool,
    mut on_progress: F,
) -> Result<(), EngineError>
where
    F: FnMut(ChurnBatchProgress),
{
    let mut working_commits: Vec<CommitSummary> = snapshot.commits.clone();
    let all_oid_keys: Vec<[u8; OID_BYTE_LEN]> =
        working_commits.iter().filter_map(|commit| parse_oid_hex(&commit.oid_hex).ok().map(oid_to_bytes)).collect();
    let existing_cache: HashMap<[u8; OID_BYTE_LEN], CommitChurnStats> = cache.get_commit_churn_batch(&all_oid_keys)?;

    let mut churn_by_oid_hex: HashMap<String, CommitChurnStats> = HashMap::with_capacity(working_commits.len());
    let mut uncached_indices: Vec<(usize, ObjectId)> = Vec::new();
    let mut indexed_count: u32 = 0;

    for (idx, commit) in working_commits.iter_mut().enumerate() {
        let Ok(oid) = parse_oid_hex(&commit.oid_hex) else {
            continue;
        };
        let key: [u8; OID_BYTE_LEN] = oid_to_bytes(oid);
        if let Some(stats) = existing_cache.get(&key) {
            commit.files_changed = stats.files_changed;
            commit.insertions = stats.insertions;
            commit.deletions = stats.deletions;
            commit.churn_indexed = true;
            churn_by_oid_hex.insert(commit.oid_hex.clone(), stats.clone());
            indexed_count += 1;
        } else {
            uncached_indices.push((idx, oid));
        }
    }

    let total_commits: u32 = working_commits.len() as u32;
    if uncached_indices.is_empty() {
        let (weekly_churn, monthly_churn) = build_churn_buckets(&working_commits);
        on_progress(ChurnBatchProgress {
            indexed_commits: total_commits,
            total_commits,
            updated_commits: Vec::new(),
            contributors: build_contributor_stats(&working_commits),
            hotspots: build_file_hotspots(&churn_by_oid_hex),
            weekly_churn,
            monthly_churn,
            is_complete: true,
        });
        return Ok(());
    }

    let shared_repo: gix::ThreadSafeRepository = gix::discover(repo_path)
        .map_err(|error| EngineError::RepositoryOpen {
            path: repo_path.display().to_string(),
            message: error.to_string(),
        })?
        .into_sync();

    for chunk in uncached_indices.chunks(CHURN_BATCH_SIZE) {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(EngineError::Cancelled);
        }

        let computed_chunk: Vec<(usize, ObjectId, CommitChurnStats)> = chunk
            .par_iter()
            .filter_map(|(commit_idx, commit_oid)| {
                if cancel_flag.load(Ordering::Relaxed) {
                    return None;
                }
                let local_repo: gix::Repository = shared_repo.to_thread_local();
                let touched_paths: Vec<FileChurnEntry> = compute_commit_churn_entries(&local_repo, *commit_oid).ok()?;
                let files_changed: u32 = touched_paths.len() as u32;
                let insertions: u32 = touched_paths.iter().map(|entry| entry.insertions).sum();
                let deletions: u32 = touched_paths.iter().map(|entry| entry.deletions).sum();
                Some((
                    *commit_idx,
                    *commit_oid,
                    CommitChurnStats { files_changed, insertions, deletions, touched_paths },
                ))
            })
            .collect();

        if cancel_flag.load(Ordering::Relaxed) {
            return Err(EngineError::Cancelled);
        }

        let cache_entries: Vec<([u8; OID_BYTE_LEN], CommitChurnStats)> =
            computed_chunk.iter().map(|(_, oid, stats)| (oid_to_bytes(*oid), stats.clone())).collect();
        cache.insert_commit_churn_batch(&cache_entries)?;

        let mut updated_commits: Vec<(String, u32, u32, u32)> = Vec::with_capacity(computed_chunk.len());
        for (commit_idx, _, stats) in computed_chunk {
            if let Some(commit) = working_commits.get_mut(commit_idx) {
                commit.files_changed = stats.files_changed;
                commit.insertions = stats.insertions;
                commit.deletions = stats.deletions;
                commit.churn_indexed = true;
                indexed_count += 1;
                updated_commits.push((commit.oid_hex.clone(), stats.files_changed, stats.insertions, stats.deletions));
                churn_by_oid_hex.insert(commit.oid_hex.clone(), stats);
            }
        }

        let is_complete: bool = indexed_count >= total_commits;
        let (weekly_churn, monthly_churn) = build_churn_buckets(&working_commits);
        on_progress(ChurnBatchProgress {
            indexed_commits: indexed_count,
            total_commits,
            updated_commits,
            contributors: build_contributor_stats(&working_commits),
            hotspots: build_file_hotspots(&churn_by_oid_hex),
            weekly_churn,
            monthly_churn,
            is_complete,
        });
    }

    log::info!("completed Phase 2 churn index for {} ({indexed_count}/{total_commits} commits)", snapshot.repo_path);
    Ok(())
}

fn collect_repository_refs(repo: &gix::Repository) -> Vec<RefItem> {
    let mut items: Vec<RefItem> = Vec::new();

    if let Ok(mut head) = repo.head()
        && let Ok(Some(oid)) = head.try_peel_to_id_in_place()
    {
        let head_label: String = head
            .referent_name()
            .map_or_else(|| "HEAD (detached)".to_owned(), |name| format!("HEAD ({})", name.shorten()));
        items.push(RefItem {
            display_name: head_label,
            full_ref_name: "HEAD".to_owned(),
            target_oid_hex: oid.detach().to_hex().to_string(),
            kind: RefKind::Head,
        });
    }

    if let Ok(platform) = repo.references()
        && let Ok(all_refs) = platform.all()
    {
        for reference_result in all_refs.flatten() {
            let mut reference = reference_result;
            let full_name: String = reference.name().as_bstr().to_string();
            let short_name: String = reference.name().shorten().to_string();
            let kind: RefKind = if full_name.starts_with("refs/heads/") {
                RefKind::LocalBranch
            } else if full_name.starts_with("refs/remotes/") {
                RefKind::RemoteBranch
            } else if full_name.starts_with("refs/tags/") {
                RefKind::Tag
            } else {
                continue;
            };

            if let Ok(id) = reference.peel_to_id_in_place() {
                let target_oid: ObjectId = id.detach();
                if let Ok(obj) = repo.find_object(target_oid)
                    && obj.try_into_commit().is_ok()
                {
                    items.push(RefItem {
                        display_name: short_name,
                        full_ref_name: full_name,
                        target_oid_hex: target_oid.to_hex().to_string(),
                        kind,
                    });
                }
            }
        }
    }

    items
}

fn resolve_active_ref(
    repo: &gix::Repository,
    target_ref: Option<&str>,
    refs: &[RefItem],
) -> Result<(String, ObjectId), EngineError> {
    if let Some(requested) = target_ref.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(found) = refs.iter().find(|item| {
            item.full_ref_name == requested || item.display_name == requested || item.target_oid_hex == requested
        }) {
            let oid: ObjectId = parse_oid_hex(&found.target_oid_hex)?;
            return Ok((found.display_name.clone(), oid));
        }

        if let Ok(oid) = parse_oid_hex(requested) {
            return Ok((shorten_oid_str(requested), oid));
        }
    }

    if let Some(first_ref) = refs.first() {
        let oid: ObjectId = parse_oid_hex(&first_ref.target_oid_hex)?;
        return Ok((first_ref.display_name.clone(), oid));
    }

    let oid: ObjectId = repo
        .head_id()
        .map_err(|error| EngineError::GitOperation(format!("repository has no commits at HEAD: {error}")))?
        .detach();
    Ok(("HEAD".to_owned(), oid))
}

fn walk_commit_graph(
    repo: &gix::Repository,
    start_oid: ObjectId,
    cancel_flag: &AtomicBool,
) -> Result<Vec<CommitSummary>, EngineError> {
    let walk = repo
        .rev_walk([start_oid])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()
        .map_err(|error| EngineError::GitOperation(format!("failed to start commit graph walk: {error}")))?;

    let mut commits: Vec<CommitSummary> = Vec::new();
    for item_result in walk {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(EngineError::Cancelled);
        }
        let info =
            item_result.map_err(|error| EngineError::GitOperation(format!("commit graph walk error: {error}")))?;
        let commit_oid: ObjectId = info.id;
        let oid_hex: String = commit_oid.to_hex().to_string();
        let short_oid: String = shorten_oid_str(&oid_hex);
        let parent_count: u32 = info.parent_ids.len() as u32;

        let object = repo
            .find_object(commit_oid)
            .map_err(|error| EngineError::GitOperation(format!("failed to read commit {commit_oid}: {error}")))?;
        let commit = object
            .try_into_commit()
            .map_err(|error| EngineError::GitOperation(format!("object {commit_oid} is not a commit: {error}")))?;
        let commit_ref = commit
            .decode()
            .map_err(|error| EngineError::GitOperation(format!("failed to decode commit {commit_oid}: {error}")))?;

        let author_name: String = String::from_utf8_lossy(commit_ref.author.name).into_owned();
        let author_email: String = String::from_utf8_lossy(commit_ref.author.email).into_owned();
        let timestamp_secs: i64 = info.commit_time.unwrap_or(commit_ref.committer.time.seconds);
        let date_iso: String = format_date_iso(timestamp_secs);
        let message: String = String::from_utf8_lossy(commit_ref.message).into_owned();
        let summary: String = message.lines().next().unwrap_or("").trim().to_owned();

        commits.push(CommitSummary {
            oid_hex,
            short_oid,
            summary,
            message,
            author_name,
            author_email,
            timestamp_secs,
            date_iso,
            parent_count,
            files_changed: 0,
            insertions: 0,
            deletions: 0,
            churn_indexed: false,
        });
    }

    Ok(commits)
}

fn compute_ref_languages(
    repo: &gix::Repository,
    commit_oid_hex: &str,
    cache: &OidCache,
    cancel_flag: &AtomicBool,
) -> Result<Vec<LanguageStat>, EngineError> {
    let entries: Vec<TreeEntryItem> = list_tree_entries(repo, commit_oid_hex)?;
    let blob_keys: Vec<[u8; OID_BYTE_LEN]> =
        entries.iter().filter_map(|item| parse_oid_hex(&item.blob_oid_hex).ok().map(oid_to_bytes)).collect();

    let mut cached_blobs: HashMap<[u8; OID_BYTE_LEN], LanguageBlobStats> = cache.get_blob_lang_batch(&blob_keys)?;

    let mut uncached_items: Vec<([u8; OID_BYTE_LEN], LanguageType, Vec<u8>)> = Vec::new();
    let tokei_config: TokeiConfig = TokeiConfig::default();

    for entry in &entries {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(EngineError::Cancelled);
        }
        let Some(lang_type) = LanguageType::from_path(Path::new(&entry.path), &tokei_config) else {
            continue;
        };
        let Ok(oid) = parse_oid_hex(&entry.blob_oid_hex) else {
            continue;
        };
        let key: [u8; OID_BYTE_LEN] = oid_to_bytes(oid);
        if cached_blobs.contains_key(&key) {
            continue;
        }
        if let Ok(object) = repo.find_object(oid) {
            uncached_items.push((key, lang_type, object.data.clone()));
        }
    }

    if !uncached_items.is_empty() {
        let newly_parsed: Vec<([u8; OID_BYTE_LEN], LanguageBlobStats)> = uncached_items
            .par_iter()
            .map(|(key, lang_type, data)| {
                let stats = lang_type.parse_from_slice(data.as_slice(), &tokei_config);
                (
                    *key,
                    LanguageBlobStats {
                        code: stats.code as u32,
                        comments: stats.comments as u32,
                        blanks: stats.blanks as u32,
                    },
                )
            })
            .collect();

        cache.insert_blob_lang_batch(&newly_parsed)?;
        for (key, stats) in newly_parsed {
            cached_blobs.insert(key, stats);
        }
    }

    let mut by_language: BTreeMap<String, (u32, u64, u64, u64, u64)> = BTreeMap::new();
    for entry in &entries {
        if entry.language == "Other" {
            continue;
        }
        let Ok(oid) = parse_oid_hex(&entry.blob_oid_hex) else {
            continue;
        };
        let key: [u8; OID_BYTE_LEN] = oid_to_bytes(oid);
        let blob_stats: LanguageBlobStats =
            cached_blobs.get(&key).copied().unwrap_or(LanguageBlobStats { code: 0, comments: 0, blanks: 0 });

        let agg = by_language.entry(entry.language.clone()).or_insert((0, 0, 0, 0, 0));
        agg.0 += 1;
        agg.1 += u64::from(blob_stats.code);
        agg.2 += u64::from(blob_stats.comments);
        agg.3 += u64::from(blob_stats.blanks);
        agg.4 += entry.size_bytes;
    }

    let total_code: u64 = by_language.values().map(|v| v.1).sum::<u64>().max(1);
    let mut stats: Vec<LanguageStat> = by_language
        .into_iter()
        .map(|(language, (files, code, comments, blanks, bytes))| {
            let percentage: f32 = ((code as f64 / total_code as f64) * 100.0) as f32;
            let color = language_badge_color(&language);
            LanguageStat { language, files, code, comments, blanks, bytes, percentage, color }
        })
        .collect();

    stats.sort_by(|a, b| b.code.cmp(&a.code).then_with(|| b.files.cmp(&a.files)));
    Ok(stats)
}

/// Aggregates contributor statistics sorted descending by commit count.
pub fn build_contributor_stats(commits: &[CommitSummary]) -> Vec<ContributorStat> {
    let mut map: HashMap<String, ContributorStat> = HashMap::new();

    for commit in commits {
        let key: String = if commit.author_email.is_empty() {
            commit.author_name.clone()
        } else {
            commit.author_email.to_ascii_lowercase()
        };

        let entry = map.entry(key).or_insert_with(|| ContributorStat {
            name: commit.author_name.clone(),
            email: commit.author_email.clone(),
            commits: 0,
            insertions: 0,
            deletions: 0,
            first_commit_date: commit.date_iso.clone(),
            last_commit_date: commit.date_iso.clone(),
        });

        entry.commits += 1;
        entry.insertions += u64::from(commit.insertions);
        entry.deletions += u64::from(commit.deletions);
        if commit.date_iso < entry.first_commit_date {
            entry.first_commit_date.clone_from(&commit.date_iso);
        }
        if commit.date_iso > entry.last_commit_date {
            entry.last_commit_date.clone_from(&commit.date_iso);
        }
    }

    let mut list: Vec<ContributorStat> = map.into_values().collect();
    list.sort_by(|a, b| b.commits.cmp(&a.commits).then_with(|| b.insertions.cmp(&a.insertions)));
    list
}

/// Builds the 52-week x 7-day commit activity heatmap anchored to the latest commit date.
pub fn build_heatmap_weeks(commits: &[CommitSummary]) -> Vec<HeatmapWeek> {
    let latest_ts: i64 = commits.first().map_or_else(|| Utc::now().timestamp(), |commit| commit.timestamp_secs);
    let anchor_date: NaiveDate =
        Utc.timestamp_opt(latest_ts, 0).single().map_or_else(|| Utc::now().date_naive(), |dt| dt.date_naive());

    let mut counts_by_date: HashMap<String, u32> = HashMap::new();
    for commit in commits {
        *counts_by_date.entry(commit.date_iso.clone()).or_insert(0) += 1;
    }

    let max_daily: u32 = counts_by_date.values().copied().max().unwrap_or(1).max(1);
    let weekday_offset: i64 = i64::from(anchor_date.weekday().num_days_from_sunday());
    let end_of_latest_week: NaiveDate =
        anchor_date + chrono::Duration::days((DAYS_PER_WEEK as i64 - 1) - weekday_offset);
    let total_days: i64 = (HEATMAP_WEEKS * DAYS_PER_WEEK) as i64;
    let start_date: NaiveDate = end_of_latest_week - chrono::Duration::days(total_days - 1);

    let mut weeks: Vec<HeatmapWeek> = Vec::with_capacity(HEATMAP_WEEKS);
    for week_idx in 0..HEATMAP_WEEKS {
        let week_start: NaiveDate = start_date + chrono::Duration::days((week_idx * DAYS_PER_WEEK) as i64);
        let week_label: String = week_start.format("%b %d").to_string();
        let days: [HeatmapDay; DAYS_PER_WEEK] = std::array::from_fn(|day_idx| {
            let day_date: NaiveDate = week_start + chrono::Duration::days(day_idx as i64);
            let date_iso: String = day_date.format("%Y-%m-%d").to_string();
            let count: u32 = counts_by_date.get(&date_iso).copied().unwrap_or(0);
            let intensity_level: u8 = if count == 0 {
                0
            } else {
                let ratio: f32 = count as f32 / max_daily as f32;
                if ratio <= 0.25 {
                    1
                } else if ratio <= 0.50 {
                    2
                } else if ratio <= 0.75 {
                    3
                } else {
                    4
                }
            };
            HeatmapDay { date_iso, commit_count: count, intensity_level }
        });
        weeks.push(HeatmapWeek { week_label, days });
    }

    weeks
}

/// Ranks `File Hotspot` entries across all indexed commits by touch count and total line churn.
pub fn build_file_hotspots<S: std::hash::BuildHasher>(
    churn_by_oid: &HashMap<String, CommitChurnStats, S>,
) -> Vec<FileHotspot> {
    let mut file_map: HashMap<String, (u32, u64, u64)> = HashMap::new();
    for stats in churn_by_oid.values() {
        for entry in &stats.touched_paths {
            let record = file_map.entry(entry.path.clone()).or_insert((0, 0, 0));
            record.0 += 1;
            record.1 += u64::from(entry.insertions);
            record.2 += u64::from(entry.deletions);
        }
    }

    let mut hotspots: Vec<FileHotspot> = file_map
        .into_iter()
        .map(|(path, (touches, insertions, deletions))| {
            let language: String = classify_language_name(&path);
            let total_churn: u64 = insertions + deletions;
            FileHotspot { path, language, touches, insertions, deletions, total_churn }
        })
        .collect();

    hotspots.sort_by(|a, b| b.touches.cmp(&a.touches).then_with(|| b.total_churn.cmp(&a.total_churn)));
    hotspots.truncate(MAX_HOTSPOTS);
    hotspots
}

/// Builds weekly and monthly time-bucketed code churn and commit velocity series in chronological order.
pub fn build_churn_buckets(commits: &[CommitSummary]) -> (Vec<ChurnTimeBucket>, Vec<ChurnTimeBucket>) {
    let mut weekly_map: BTreeMap<(i32, u32), ChurnTimeBucket> = BTreeMap::new();
    let mut monthly_map: BTreeMap<(i32, u32), ChurnTimeBucket> = BTreeMap::new();

    for commit in commits {
        let dt = Utc.timestamp_opt(commit.timestamp_secs, 0).single().unwrap_or_else(Utc::now);
        let iso_week = dt.iso_week();
        let week_key: (i32, u32) = (iso_week.year(), iso_week.week());
        let week_entry = weekly_map.entry(week_key).or_insert_with(|| ChurnTimeBucket {
            period_label: format!("{}-W{:02}", iso_week.year(), iso_week.week()),
            start_timestamp: commit.timestamp_secs,
            commits: 0,
            insertions: 0,
            deletions: 0,
            net_lines: 0,
        });
        week_entry.commits += 1;
        week_entry.insertions += u64::from(commit.insertions);
        week_entry.deletions += u64::from(commit.deletions);
        week_entry.net_lines += i64::from(commit.insertions) - i64::from(commit.deletions);
        if commit.timestamp_secs < week_entry.start_timestamp {
            week_entry.start_timestamp = commit.timestamp_secs;
        }

        let month_key: (i32, u32) = (dt.year(), dt.month());
        let month_entry = monthly_map.entry(month_key).or_insert_with(|| ChurnTimeBucket {
            period_label: format!("{:04}-{:02}", dt.year(), dt.month()),
            start_timestamp: commit.timestamp_secs,
            commits: 0,
            insertions: 0,
            deletions: 0,
            net_lines: 0,
        });
        month_entry.commits += 1;
        month_entry.insertions += u64::from(commit.insertions);
        month_entry.deletions += u64::from(commit.deletions);
        month_entry.net_lines += i64::from(commit.insertions) - i64::from(commit.deletions);
        if commit.timestamp_secs < month_entry.start_timestamp {
            month_entry.start_timestamp = commit.timestamp_secs;
        }
    }

    let weekly: Vec<ChurnTimeBucket> = weekly_map.into_values().collect();
    let monthly: Vec<ChurnTimeBucket> = monthly_map.into_values().collect();
    (weekly, monthly)
}

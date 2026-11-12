use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use gix::ObjectId;
use rayon::prelude::*;
use tokei::LanguageType;

use crate::cache::{OID_BYTE_LEN, OidCache};
use crate::diff::{compute_commit_churn_entries, parse_oid_hex};
use crate::error::EngineError;
use crate::models::{
    ChurnBatchProgress, ChurnTimeBucket, CommitChurnStats, CommitSummary, ContributorStat, DAYS_PER_WEEK,
    FileChurnEntry, FileHotspot, HEATMAP_WEEKS, HeatmapDay, HeatmapWeek, LanguageBlobStats, LanguageStat,
    Phase1Snapshot, RefItem, RefKind, TreeEntryItem,
};
use crate::tree_blame::{
    TOKEI_CONFIG, classify_language_name, format_date_iso, language_badge_color, list_tree_entries_with_options,
    shorten_oid_str,
};

/// Number of commits processed per `Phase 2 Churn Index` batch before persisting to `redb`.
pub const CHURN_BATCH_SIZE: usize = 2_000;

/// Number of temporally adjacent commits processed per `rayon` worker sub-chunk to maximize pack delta cache hits.
pub const PARALLEL_SUBCHUNK_SIZE: usize = 64;

/// Size in bytes (32 MiB) of each thread-local `gix` object cache.
pub const WORKER_OBJECT_CACHE_BYTES: usize = 32 * 1024 * 1024;

/// Maximum number of `File Hotspot` entries returned to the UI.
pub const MAX_HOTSPOTS: usize = 100;

/// Seconds in one standard UTC day.
pub const SECONDS_PER_DAY: i64 = 86_400;

/// Incremental accumulator for contributor, hotspot, and time-bucket analytics across Phase 1 and Phase 2.
struct IncrementalAnalytics {
    contributors: HashMap<String, ContributorStat>,
    hotspots: HashMap<String, (u32, u64, u64)>,
    weekly: BTreeMap<(i32, u32), ChurnTimeBucket>,
    monthly: BTreeMap<(i32, u32), ChurnTimeBucket>,
}

impl IncrementalAnalytics {
    fn from_initial_commits(commits: &[CommitSummary]) -> Self {
        let mut contributors: HashMap<String, ContributorStat> = HashMap::new();
        let mut weekly: BTreeMap<(i32, u32), ChurnTimeBucket> = BTreeMap::new();
        let mut monthly: BTreeMap<(i32, u32), ChurnTimeBucket> = BTreeMap::new();

        for commit in commits {
            let key: String = author_key(commit);
            let entry = contributors.entry(key).or_insert_with(|| ContributorStat {
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

            let (week_key, week_label, month_key, month_label) = time_bucket_keys(commit.timestamp_secs);
            let week_entry = weekly.entry(week_key).or_insert_with(|| ChurnTimeBucket {
                period_label: week_label,
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

            let month_entry = monthly.entry(month_key).or_insert_with(|| ChurnTimeBucket {
                period_label: month_label,
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

        Self { contributors, hotspots: HashMap::new(), weekly, monthly }
    }

    fn record_touched_paths(&mut self, touched_paths: &[FileChurnEntry]) {
        for entry in touched_paths {
            let record = self.hotspots.entry(entry.path.clone()).or_insert((0, 0, 0));
            record.0 += 1;
            record.1 += u64::from(entry.insertions);
            record.2 += u64::from(entry.deletions);
        }
    }

    fn record_commit_churn_delta(&mut self, commit: &CommitSummary, stats: &CommitChurnStats) {
        if stats.insertions > 0 || stats.deletions > 0 {
            let key: String = author_key(commit);
            if let Some(entry) = self.contributors.get_mut(&key) {
                entry.insertions += u64::from(stats.insertions);
                entry.deletions += u64::from(stats.deletions);
            }

            let (week_key, _, month_key, _) = time_bucket_keys(commit.timestamp_secs);
            let net_delta: i64 = i64::from(stats.insertions) - i64::from(stats.deletions);
            if let Some(week_entry) = self.weekly.get_mut(&week_key) {
                week_entry.insertions += u64::from(stats.insertions);
                week_entry.deletions += u64::from(stats.deletions);
                week_entry.net_lines += net_delta;
            }
            if let Some(month_entry) = self.monthly.get_mut(&month_key) {
                month_entry.insertions += u64::from(stats.insertions);
                month_entry.deletions += u64::from(stats.deletions);
                month_entry.net_lines += net_delta;
            }
        }

        self.record_touched_paths(&stats.touched_paths);
    }

    fn sorted_contributors(&self) -> Vec<ContributorStat> {
        let mut list: Vec<ContributorStat> = self.contributors.values().cloned().collect();
        list.sort_unstable_by(|a, b| b.commits.cmp(&a.commits).then_with(|| b.insertions.cmp(&a.insertions)));
        list
    }

    fn top_hotspots(&self) -> Vec<FileHotspot> {
        let mut raw: Vec<(&String, &(u32, u64, u64))> = self.hotspots.iter().collect();
        raw.sort_unstable_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| (b.1.1 + b.1.2).cmp(&(a.1.1 + a.1.2))));
        raw.truncate(MAX_HOTSPOTS);

        raw.into_iter()
            .map(|(path, (touches, insertions, deletions))| FileHotspot {
                path: path.clone(),
                language: classify_language_name(path),
                touches: *touches,
                insertions: *insertions,
                deletions: *deletions,
                total_churn: *insertions + *deletions,
            })
            .collect()
    }

    fn churn_series(&self) -> (Vec<ChurnTimeBucket>, Vec<ChurnTimeBucket>) {
        (self.weekly.values().cloned().collect(), self.monthly.values().cloned().collect())
    }
}

fn author_key(commit: &CommitSummary) -> String {
    if commit.author_email.is_empty() { commit.author_name.clone() } else { commit.author_email.to_ascii_lowercase() }
}

fn time_bucket_keys(timestamp_secs: i64) -> ((i32, u32), String, (i32, u32), String) {
    let dt = Utc.timestamp_opt(timestamp_secs, 0).single().unwrap_or_else(Utc::now);
    let iso_week = dt.iso_week();
    let week_key: (i32, u32) = (iso_week.year(), iso_week.week());
    let week_label: String = format!("{}-W{:02}", iso_week.year(), iso_week.week());
    let month_key: (i32, u32) = (dt.year(), dt.month());
    let month_label: String = format!("{:04}-{:02}", dt.year(), dt.month());
    (week_key, week_label, month_key, month_label)
}

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
    let mut repo: gix::Repository = gix::discover(repo_path).map_err(|error| EngineError::RepositoryOpen {
        path: repo_path.display().to_string(),
        message: error.to_string(),
    })?;
    repo.object_cache_size_if_unset(WORKER_OBJECT_CACHE_BYTES);

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

    let shared_repo: gix::ThreadSafeRepository = repo.clone().into_sync();
    let (walk_res, lang_res) = rayon::join(
        || {
            let mut local_repo: gix::Repository = shared_repo.to_thread_local();
            local_repo.object_cache_size_if_unset(WORKER_OBJECT_CACHE_BYTES);
            walk_commit_graph(&local_repo, &shared_repo, active_ref_oid, cancel_flag)
        },
        || {
            let mut local_repo: gix::Repository = shared_repo.to_thread_local();
            local_repo.object_cache_size_if_unset(WORKER_OBJECT_CACHE_BYTES);
            compute_ref_languages(repo_path, &local_repo, &active_ref_oid_hex, cache, cancel_flag)
        },
    );

    let (mut commits, commit_oid_keys) = walk_res?;
    let languages: Vec<LanguageStat> = lang_res?;

    let mut indexed_churn_commits: u32 = 0;
    let mut hotspot_accum: HashMap<String, (u32, u64, u64)> = HashMap::new();

    cache.scan_commit_churn_batch(&commit_oid_keys, |idx, churn| {
        if let Some(commit) = commits.get_mut(idx) {
            commit.files_changed = churn.files_changed;
            commit.insertions = churn.insertions;
            commit.deletions = churn.deletions;
            commit.churn_indexed = true;
            indexed_churn_commits += 1;
            for entry in &churn.touched_paths {
                let record = hotspot_accum.entry(entry.path.clone()).or_insert((0, 0, 0));
                record.0 += 1;
                record.1 += u64::from(entry.insertions);
                record.2 += u64::from(entry.deletions);
            }
        }
    })?;

    if cancel_flag.load(Ordering::Relaxed) {
        return Err(EngineError::Cancelled);
    }

    let mut analytics: IncrementalAnalytics = IncrementalAnalytics::from_initial_commits(&commits);
    analytics.hotspots = hotspot_accum;

    let contributors: Vec<ContributorStat> = analytics.sorted_contributors();
    let heatmap_weeks: Vec<HeatmapWeek> = build_heatmap_weeks(&commits);
    let hotspots: Vec<FileHotspot> = analytics.top_hotspots();
    let (weekly_churn, monthly_churn) = analytics.churn_series();
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
    for commit in &mut working_commits {
        commit.files_changed = 0;
        commit.insertions = 0;
        commit.deletions = 0;
        commit.churn_indexed = false;
    }

    let mut analytics: IncrementalAnalytics = IncrementalAnalytics::from_initial_commits(&working_commits);
    let all_oid_keys: Vec<[u8; OID_BYTE_LEN]> = working_commits
        .iter()
        .map(|commit| parse_oid_hex(&commit.oid_hex).map_or([0u8; OID_BYTE_LEN], oid_to_bytes))
        .collect();

    let mut indexed_count: u32 = 0;
    cache.scan_commit_churn_batch(&all_oid_keys, |idx, stats| {
        if let Some(commit) = working_commits.get_mut(idx) {
            commit.files_changed = stats.files_changed;
            commit.insertions = stats.insertions;
            commit.deletions = stats.deletions;
            commit.churn_indexed = true;
            indexed_count += 1;
            analytics.record_commit_churn_delta(commit, &stats);
        }
    })?;

    let mut uncached_indices: Vec<(usize, ObjectId, u32)> = Vec::new();
    for (idx, commit) in working_commits.iter().enumerate() {
        if !commit.churn_indexed
            && let Ok(oid) = parse_oid_hex(&commit.oid_hex)
        {
            uncached_indices.push((idx, oid, commit.parent_count));
        }
    }

    let total_commits: u32 = working_commits.len() as u32;
    if uncached_indices.is_empty() {
        let (weekly_churn, monthly_churn) = analytics.churn_series();
        on_progress(ChurnBatchProgress {
            indexed_commits: total_commits,
            total_commits,
            updated_commits: Vec::new(),
            contributors: analytics.sorted_contributors(),
            hotspots: analytics.top_hotspots(),
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
            .par_chunks(PARALLEL_SUBCHUNK_SIZE)
            .flat_map_iter(|subchunk| {
                let mut results: Vec<(usize, ObjectId, CommitChurnStats)> = Vec::new();
                if cancel_flag.load(Ordering::Relaxed) {
                    return results;
                }
                let mut local_repo: gix::Repository = shared_repo.to_thread_local();
                local_repo.object_cache_size_if_unset(WORKER_OBJECT_CACHE_BYTES);

                for (commit_idx, commit_oid, parent_count) in subchunk {
                    if cancel_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    if *parent_count > 1 {
                        results.push((
                            *commit_idx,
                            *commit_oid,
                            CommitChurnStats {
                                files_changed: 0,
                                insertions: 0,
                                deletions: 0,
                                touched_paths: Vec::new(),
                            },
                        ));
                        continue;
                    }
                    if let Ok(touched_paths) = compute_commit_churn_entries(&local_repo, *commit_oid) {
                        let files_changed: u32 = touched_paths.len() as u32;
                        let insertions: u32 = touched_paths.iter().map(|entry| entry.insertions).sum();
                        let deletions: u32 = touched_paths.iter().map(|entry| entry.deletions).sum();
                        results.push((
                            *commit_idx,
                            *commit_oid,
                            CommitChurnStats { files_changed, insertions, deletions, touched_paths },
                        ));
                    }
                }
                results
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
                analytics.record_commit_churn_delta(commit, &stats);
            }
        }

        let is_complete: bool = indexed_count >= total_commits;
        let (weekly_churn, monthly_churn) = analytics.churn_series();
        on_progress(ChurnBatchProgress {
            indexed_commits: indexed_count,
            total_commits,
            updated_commits,
            contributors: analytics.sorted_contributors(),
            hotspots: analytics.top_hotspots(),
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
                let is_commit: bool = match kind {
                    RefKind::Head | RefKind::LocalBranch | RefKind::RemoteBranch => true,
                    RefKind::Tag => repo.find_object(target_oid).is_ok_and(|obj| obj.try_into_commit().is_ok()),
                };
                if is_commit {
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
    shared_repo: &gix::ThreadSafeRepository,
    start_oid: ObjectId,
    cancel_flag: &AtomicBool,
) -> Result<(Vec<CommitSummary>, Vec<[u8; OID_BYTE_LEN]>), EngineError> {
    let walk = repo
        .rev_walk([start_oid])
        .sorting(gix::revision::walk::Sorting::ByCommitTime(
            gix::traverse::commit::simple::CommitTimeOrder::NewestFirst,
        ))
        .all()
        .map_err(|error| EngineError::GitOperation(format!("failed to start commit graph walk: {error}")))?;

    let mut raw_nodes: Vec<(ObjectId, Option<i64>, u32)> = Vec::new();
    for item_result in walk {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(EngineError::Cancelled);
        }
        let info =
            item_result.map_err(|error| EngineError::GitOperation(format!("commit graph walk error: {error}")))?;
        raw_nodes.push((info.id, info.commit_time, info.parent_ids.len() as u32));
    }

    let oid_keys: Vec<[u8; OID_BYTE_LEN]> = raw_nodes.iter().map(|(oid, _, _)| oid_to_bytes(*oid)).collect();

    let commits: Vec<CommitSummary> = raw_nodes
        .par_chunks(1_024)
        .flat_map_iter(|chunk| {
            let mut local_repo: gix::Repository = shared_repo.to_thread_local();
            local_repo.object_cache_size_if_unset(WORKER_OBJECT_CACHE_BYTES);
            let mut batch: Vec<CommitSummary> = Vec::with_capacity(chunk.len());

            for &(commit_oid, cached_time, parent_count) in chunk {
                if cancel_flag.load(Ordering::Relaxed) {
                    break;
                }
                let oid_hex: String = commit_oid.to_hex().to_string();
                let short_oid: String = shorten_oid_str(&oid_hex);

                let (author_name, author_email, timestamp_secs, summary) = if let Ok(object) =
                    local_repo.find_object(commit_oid)
                    && let Ok(commit) = object.try_into_commit()
                    && let Ok(commit_ref) = commit.decode()
                {
                    let name: String = String::from_utf8_lossy(commit_ref.author.name).into_owned();
                    let email: String = String::from_utf8_lossy(commit_ref.author.email).into_owned();
                    let ts: i64 = cached_time.unwrap_or(commit_ref.committer.time.seconds);
                    let first_line_bytes: &[u8] = commit_ref.message.split(|&b| b == b'\n').next().unwrap_or(&[]);
                    let sum: String = String::from_utf8_lossy(first_line_bytes).trim().to_owned();
                    (name, email, ts, sum)
                } else {
                    ("Unknown".to_owned(), String::new(), cached_time.unwrap_or(0), String::new())
                };

                let date_iso: String = format_date_iso(timestamp_secs);
                batch.push(CommitSummary {
                    oid_hex,
                    short_oid,
                    summary,
                    message: String::new(),
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
            batch
        })
        .collect();

    if cancel_flag.load(Ordering::Relaxed) {
        return Err(EngineError::Cancelled);
    }

    Ok((commits, oid_keys))
}

fn compute_ref_languages(
    repo_path: &Path,
    repo: &gix::Repository,
    commit_oid_hex: &str,
    cache: &OidCache,
    cancel_flag: &AtomicBool,
) -> Result<Vec<LanguageStat>, EngineError> {
    let entries: Vec<TreeEntryItem> = list_tree_entries_with_options(repo, commit_oid_hex, false)?;
    let blob_keys: Vec<[u8; OID_BYTE_LEN]> =
        entries.iter().filter_map(|item| parse_oid_hex(&item.blob_oid_hex).ok().map(oid_to_bytes)).collect();

    let mut cached_blobs: HashMap<[u8; OID_BYTE_LEN], LanguageBlobStats> = cache.get_blob_lang_batch(&blob_keys)?;

    let mut uncached_items: Vec<([u8; OID_BYTE_LEN], ObjectId, LanguageType)> = Vec::new();
    let mut seen_uncached: HashSet<[u8; OID_BYTE_LEN]> = HashSet::new();

    for entry in &entries {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(EngineError::Cancelled);
        }
        let Some(lang_type) = LanguageType::from_path(Path::new(&entry.path), &TOKEI_CONFIG) else {
            continue;
        };
        let Ok(oid) = parse_oid_hex(&entry.blob_oid_hex) else {
            continue;
        };
        let key: [u8; OID_BYTE_LEN] = oid_to_bytes(oid);
        if cached_blobs.contains_key(&key) || !seen_uncached.insert(key) {
            continue;
        }
        uncached_items.push((key, oid, lang_type));
    }

    if !uncached_items.is_empty() {
        let shared_repo: gix::ThreadSafeRepository = gix::discover(repo_path)
            .map_err(|error| EngineError::RepositoryOpen {
                path: repo_path.display().to_string(),
                message: error.to_string(),
            })?
            .into_sync();

        let newly_parsed: Vec<([u8; OID_BYTE_LEN], LanguageBlobStats)> = uncached_items
            .par_chunks(128)
            .flat_map_iter(|chunk| {
                let mut results: Vec<([u8; OID_BYTE_LEN], LanguageBlobStats)> = Vec::with_capacity(chunk.len());
                if cancel_flag.load(Ordering::Relaxed) {
                    return results;
                }
                let mut local_repo: gix::Repository = shared_repo.to_thread_local();
                local_repo.object_cache_size_if_unset(WORKER_OBJECT_CACHE_BYTES);

                for (key, oid, lang_type) in chunk {
                    if let Ok(object) = local_repo.find_object(*oid) {
                        let stats = lang_type.parse_from_slice(&object.data, &TOKEI_CONFIG);
                        results.push((
                            *key,
                            LanguageBlobStats {
                                code: stats.code as u32,
                                comments: stats.comments as u32,
                                blanks: stats.blanks as u32,
                            },
                        ));
                    }
                }
                results
            })
            .collect();

        if cancel_flag.load(Ordering::Relaxed) {
            return Err(EngineError::Cancelled);
        }

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
    IncrementalAnalytics::from_initial_commits(commits).sorted_contributors()
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
    let mut analytics: IncrementalAnalytics = IncrementalAnalytics::from_initial_commits(&[]);
    for stats in churn_by_oid.values() {
        analytics.record_touched_paths(&stats.touched_paths);
    }
    analytics.top_hotspots()
}

/// Builds weekly and monthly time-bucketed code churn and commit velocity series in chronological order.
pub fn build_churn_buckets(commits: &[CommitSummary]) -> (Vec<ChurnTimeBucket>, Vec<ChurnTimeBucket>) {
    IncrementalAnalytics::from_initial_commits(commits).churn_series()
}

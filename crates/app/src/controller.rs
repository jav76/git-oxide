use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use git_oxide_engine::cache::OidCache;
use git_oxide_engine::indexer::{run_phase1, run_phase2_churn};
use git_oxide_engine::models::{
    BlameLineRow, ChangedFileStat, ChurnBatchProgress, ChurnTimeBucket, CodeTokenSpan, CommitSummary, ContributorStat,
    DiffLineKind, DiffLineRow, FileHotspot, HeatmapWeek, LanguageStat, Phase1Snapshot, TreeEntryItem,
};
use git_oxide_engine::{
    load_commit_changed_files, load_commit_file_diff, load_ref_file_blame, load_ref_file_source, load_ref_tree_entries,
};
use slint::{Color, ComponentHandle, Model, ModelRc, SharedString, VecModel, Weak};

use crate::charts::{DEFAULT_CHART_HEIGHT, DEFAULT_CHART_WIDTH, render_churn_chart_image};
use crate::{
    AppWindow, UiBlameRow, UiChangedFileRow, UiCommitRow, UiContributorRow, UiDiffRow, UiHeatmapDay, UiHeatmapWeek,
    UiHotspotRow, UiLanguageRow, UiTokenSpan, UiTreeEntryRow,
};

const BYTES_PER_KIB: u64 = 1_024;
const MAX_VISIBLE_COMMITS: usize = 2_500;
const MAX_VISIBLE_TREE_ENTRIES: usize = 5_000;
const MIN_UI_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Formats an integer with ASCII comma thousand separators (e.g., `33373576` -> `"33,373,576"`).
fn format_u64(value: u64) -> String {
    let raw: String = value.to_string();
    let bytes: &[u8] = raw.as_bytes();
    let len: usize = bytes.len();
    let mut out: String = String::with_capacity(len + len / 3);
    for (idx, &ch) in bytes.iter().enumerate() {
        if idx > 0 && (len - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(char::from(ch));
    }
    out
}

/// Shared controller state synchronized between the Slint UI thread and background workers.
pub struct ControllerState {
    pub repo_path: PathBuf,
    pub active_ref_name: String,
    pub active_ref_oid_hex: String,
    pub cache: OidCache,
    pub cancel_flag: Arc<AtomicBool>,
    pub all_commits: Vec<CommitSummary>,
    pub commit_idx_by_oid: HashMap<String, usize>,
    pub all_tree_entries: Vec<TreeEntryItem>,
    pub weekly_churn: Vec<ChurnTimeBucket>,
    pub monthly_churn: Vec<ChurnTimeBucket>,
    pub churn_monthly_mode: bool,
    pub author_filter: Option<String>,
    pub date_filter: Option<String>,
    pub search_query: String,
    pub file_filter_query: String,
    pub selected_commit_oid: String,
    pub selected_tree_file: String,
    pub show_blame_gutter: bool,
    pub current_diff_rows: Vec<DiffLineRow>,
    pub is_split_diff: bool,
}

fn lock_state(state: &Arc<Mutex<ControllerState>>) -> std::sync::MutexGuard<'_, ControllerState> {
    state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Wires all `AppWindow` callbacks to background `git-oxide-engine` workers.
///
/// Must be called on the Slint UI thread.
pub fn attach_controller(window: &AppWindow, cache: OidCache, initial_repo_path: PathBuf, initial_ref: Option<String>) {
    let state: Arc<Mutex<ControllerState>> = Arc::new(Mutex::new(ControllerState {
        repo_path: initial_repo_path.clone(),
        active_ref_name: String::new(),
        active_ref_oid_hex: String::new(),
        cache,
        cancel_flag: Arc::new(AtomicBool::new(false)),
        all_commits: Vec::new(),
        commit_idx_by_oid: HashMap::new(),
        all_tree_entries: Vec::new(),
        weekly_churn: Vec::new(),
        monthly_churn: Vec::new(),
        churn_monthly_mode: false,
        author_filter: None,
        date_filter: None,
        search_query: String::new(),
        file_filter_query: String::new(),
        selected_commit_oid: String::new(),
        selected_tree_file: String::new(),
        show_blame_gutter: true,
        current_diff_rows: Vec::new(),
        is_split_diff: false,
    }));

    // 1. Open Repository Dialog
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_open_repo_dialog(move || {
            let current_dir: PathBuf = lock_state(&state_arc).repo_path.clone();
            if let Some(picked_dir) = rfd::FileDialog::new().set_directory(&current_dir).pick_folder() {
                start_repository_load(&window_weak, &state_arc, picked_dir, None);
            }
        });
    }

    // 2. Select Branch / Tag Ref
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_select_ref(move |ref_name: SharedString| {
            let repo_path: PathBuf = lock_state(&state_arc).repo_path.clone();
            start_repository_load(&window_weak, &state_arc, repo_path, Some(ref_name.to_string()));
        });
    }

    // 3. Clear Author / Date Filter
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_clear_filter(move || {
            {
                let mut st = lock_state(&state_arc);
                st.author_filter = None;
                st.date_filter = None;
            }
            if let Some(win) = window_weak.upgrade() {
                win.set_active_filter_text(SharedString::default());
                let _ = refresh_filtered_commits(&win, &state_arc);
            }
        });
    }

    // 4. Switch Activity Rail View
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_select_view(move |view_idx: i32| {
            if let Some(win) = window_weak.upgrade() {
                win.set_active_view(view_idx);
            }
            if view_idx == 2 && lock_state(&state_arc).all_tree_entries.is_empty() {
                spawn_tree_entries_load(&window_weak, &state_arc, None);
            }
        });
    }

    // 5. Filter Commits by Heatmap Date (Cross-View Drill-Down)
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_filter_by_date(move |date_iso: SharedString| {
            let date_str: String = date_iso.to_string();
            {
                let mut st = lock_state(&state_arc);
                st.date_filter = Some(date_str.clone());
                st.author_filter = None;
            }
            if let Some(win) = window_weak.upgrade() {
                win.set_active_filter_text(SharedString::from(format!("Date: {date_str}")));
                win.set_active_view(1);
                let first_oid: Option<String> = refresh_filtered_commits(&win, &state_arc);
                if let Some(oid) = first_oid {
                    trigger_commit_selection(&window_weak, &state_arc, &oid);
                }
            }
        });
    }

    // 6. Filter Commits by Contributor (Cross-View Drill-Down)
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_filter_by_author(move |author: SharedString| {
            let author_str: String = author.to_string();
            {
                let mut st = lock_state(&state_arc);
                st.author_filter = Some(author_str.clone());
                st.date_filter = None;
            }
            if let Some(win) = window_weak.upgrade() {
                win.set_active_filter_text(SharedString::from(format!("Author: {author_str}")));
                win.set_active_view(1);
                let first_oid: Option<String> = refresh_filtered_commits(&win, &state_arc);
                if let Some(oid) = first_oid {
                    trigger_commit_selection(&window_weak, &state_arc, &oid);
                }
            }
        });
    }

    // 7. Inspect File in Blame View (Cross-View Drill-Down from Hotspots or Diff Toolbar)
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_inspect_file_blame(move |file_path: SharedString| {
            let path_str: String = file_path.to_string();
            if path_str.is_empty() {
                return;
            }
            if let Some(win) = window_weak.upgrade() {
                win.set_active_view(2);
            }
            if lock_state(&state_arc).all_tree_entries.is_empty() {
                spawn_tree_entries_load(&window_weak, &state_arc, Some(path_str));
            } else {
                trigger_tree_file_selection(&window_weak, &state_arc, &path_str);
            }
        });
    }

    // 8. Toggle Churn Timeline Granularity (Weekly vs Monthly)
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_select_churn_granularity(move |monthly: bool| {
            lock_state(&state_arc).churn_monthly_mode = monthly;
            if let Some(win) = window_weak.upgrade() {
                update_churn_chart_on_ui(&win, &lock_state(&state_arc));
            }
        });
    }

    // 9. Search / Filter Commit List
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_filter_commits(move |query: SharedString| {
            lock_state(&state_arc).search_query = query.to_string();
            if let Some(win) = window_weak.upgrade() {
                let _ = refresh_filtered_commits(&win, &state_arc);
            }
        });
    }

    // 10. Select Commit in Commit List
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_select_commit(move |oid_hex: SharedString| {
            trigger_commit_selection(&window_weak, &state_arc, oid_hex.as_str());
        });
    }

    // 11. Select Changed File in Commit Diff
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_select_changed_file(move |file_path: SharedString| {
            let commit_oid: String = lock_state(&state_arc).selected_commit_oid.clone();
            if !commit_oid.is_empty() {
                spawn_file_diff_load(&window_weak, &state_arc, &commit_oid, file_path.as_str());
            }
        });
    }

    // 12. Toggle Unified vs Side-by-Side Diff Mode
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_toggle_diff_mode(move |split_mode: bool| {
            let model: ModelRc<UiDiffRow> = {
                let mut st = lock_state(&state_arc);
                st.is_split_diff = split_mode;
                to_ui_diff_rows_model(&st.current_diff_rows, split_mode)
            };
            if let Some(win) = window_weak.upgrade() {
                win.set_is_split_diff(split_mode);
                win.set_diff_rows(model);
            }
        });
    }

    // 13. Filter Repository Tree Files
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_filter_tree_files(move |query: SharedString| {
            lock_state(&state_arc).file_filter_query = query.to_string();
            if let Some(win) = window_weak.upgrade() {
                refresh_filtered_tree_entries(&win, &lock_state(&state_arc));
            }
        });
    }

    // 14. Select Repository File in Files & Blame View
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_select_tree_file(move |file_path: SharedString| {
            trigger_tree_file_selection(&window_weak, &state_arc, file_path.as_str());
        });
    }

    // 15. Toggle Blame Gutter vs Source Only Mode
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_toggle_blame_mode(move |show_blame: bool| {
            let current_file: String = {
                let mut st = lock_state(&state_arc);
                st.show_blame_gutter = show_blame;
                st.selected_tree_file.clone()
            };
            if !current_file.is_empty() {
                trigger_tree_file_selection(&window_weak, &state_arc, &current_file);
            }
        });
    }

    // 16. Jump from Blame Line SHA to Commit Diff
    {
        let window_weak: Weak<AppWindow> = window.as_weak();
        let state_arc: Arc<Mutex<ControllerState>> = Arc::clone(&state);
        window.on_jump_to_commit(move |commit_oid: SharedString| {
            let target_oid: String = commit_oid.to_string();
            let short_prefix: String = target_oid.chars().take(10).collect();
            {
                let mut st = lock_state(&state_arc);
                st.author_filter = None;
                st.date_filter = None;
                // If commit is beyond the top MAX_VISIBLE_COMMITS, set search query to its SHA prefix so it is visible
                let beyond_window: bool =
                    st.commit_idx_by_oid.get(&target_oid).is_some_and(|&idx| idx >= MAX_VISIBLE_COMMITS);
                if beyond_window {
                    st.search_query.clone_from(&short_prefix);
                } else {
                    st.search_query.clear();
                }
            }
            if let Some(win) = window_weak.upgrade() {
                let query_str: String = lock_state(&state_arc).search_query.clone();
                win.set_active_filter_text(SharedString::default());
                win.set_commit_search_query(SharedString::from(query_str));
                win.set_active_view(1);
                let _ = refresh_filtered_commits(&win, &state_arc);
                trigger_commit_selection(&window_weak, &state_arc, &target_oid);
            }
        });
    }

    // Kick off non-blocking initial repository load
    let window_weak: Weak<AppWindow> = window.as_weak();
    start_repository_load(&window_weak, &state, initial_repo_path, initial_ref);
}

fn start_repository_load(
    window_weak: &Weak<AppWindow>,
    state_arc: &Arc<Mutex<ControllerState>>,
    repo_path: PathBuf,
    target_ref: Option<String>,
) {
    let cancel_flag: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let cache: OidCache = {
        let mut st = lock_state(state_arc);
        st.cancel_flag.store(true, Ordering::Relaxed);
        st.cancel_flag = Arc::clone(&cancel_flag);
        st.repo_path.clone_from(&repo_path);
        st.author_filter = None;
        st.date_filter = None;
        st.search_query.clear();
        st.file_filter_query.clear();
        st.all_tree_entries.clear();
        st.cache.clone()
    };

    if let Some(win) = window_weak.upgrade() {
        win.set_repo_path(SharedString::from(repo_path.display().to_string()));
        win.set_indexing_status_text(SharedString::from("Scanning commit graph..."));
        win.set_is_indexing(true);
        win.set_active_filter_text(SharedString::default());
    }

    let win_weak_bg: Weak<AppWindow> = window_weak.clone();
    let state_bg: Arc<Mutex<ControllerState>> = Arc::clone(state_arc);
    let cancel_bg: Arc<AtomicBool> = Arc::clone(&cancel_flag);
    let repo_path_bg: PathBuf = repo_path;

    let _ = thread::Builder::new().name("git-oxide-phase1".to_owned()).spawn(move || {
        match run_phase1(&repo_path_bg, target_ref.as_deref(), &cache, &cancel_bg) {
            Ok(snapshot) => {
                if cancel_bg.load(Ordering::Relaxed) {
                    return;
                }
                {
                    let mut st = lock_state(&state_bg);
                    st.active_ref_name.clone_from(&snapshot.active_ref_name);
                    st.active_ref_oid_hex.clone_from(&snapshot.active_ref_oid_hex);
                    st.commit_idx_by_oid =
                        snapshot.commits.iter().enumerate().map(|(idx, c)| (c.oid_hex.clone(), idx)).collect();
                    st.all_commits.clone_from(&snapshot.commits);
                    st.weekly_churn.clone_from(&snapshot.weekly_churn);
                    st.monthly_churn.clone_from(&snapshot.monthly_churn);
                }

                let snapshot_for_ui: Phase1Snapshot = Phase1Snapshot {
                    repo_name: snapshot.repo_name.clone(),
                    repo_path: snapshot.repo_path.clone(),
                    active_ref_name: snapshot.active_ref_name.clone(),
                    active_ref_oid_hex: snapshot.active_ref_oid_hex.clone(),
                    refs: snapshot.refs.clone(),
                    total_commits: snapshot.total_commits,
                    indexed_churn_commits: snapshot.indexed_churn_commits,
                    commits: Vec::new(),
                    languages: snapshot.languages.clone(),
                    contributors: snapshot.contributors.clone(),
                    hotspots: snapshot.hotspots.clone(),
                    heatmap_weeks: snapshot.heatmap_weeks.clone(),
                    weekly_churn: snapshot.weekly_churn.clone(),
                    monthly_churn: snapshot.monthly_churn.clone(),
                };
                let win_for_p1: Weak<AppWindow> = win_weak_bg.clone();
                let state_for_p1: Arc<Mutex<ControllerState>> = Arc::clone(&state_bg);

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = win_for_p1.upgrade() {
                        apply_phase1_snapshot_to_window(&win, &snapshot_for_ui);
                        let first_oid: Option<String> = refresh_filtered_commits(&win, &state_for_p1);
                        if let Some(oid) = first_oid {
                            trigger_commit_selection(&win_for_p1, &state_for_p1, &oid);
                        }
                        if win.get_active_view() == 2 {
                            spawn_tree_entries_load(&win_for_p1, &state_for_p1, None);
                        }
                    }
                });

                // Run Phase 2 background churn worker on this worker thread
                let win_for_p2: Weak<AppWindow> = win_weak_bg.clone();
                let state_for_p2: Arc<Mutex<ControllerState>> = Arc::clone(&state_bg);
                let mut last_ui_push: Option<Instant> = None;
                let mut pending_commit_updates: Vec<(String, u32, u32, u32)> = Vec::new();

                let _ = run_phase2_churn(
                    &repo_path_bg,
                    &snapshot,
                    &cache,
                    &cancel_bg,
                    move |mut progress: ChurnBatchProgress| {
                        {
                            let mut st = lock_state(&state_for_p2);
                            st.weekly_churn.clone_from(&progress.weekly_churn);
                            st.monthly_churn.clone_from(&progress.monthly_churn);
                            for (oid, files, ins, del) in &progress.updated_commits {
                                if let Some(&idx) = st.commit_idx_by_oid.get(oid.as_str())
                                    && let Some(commit) = st.all_commits.get_mut(idx)
                                {
                                    commit.files_changed = *files;
                                    commit.insertions = *ins;
                                    commit.deletions = *del;
                                    commit.churn_indexed = true;
                                }
                            }
                        }

                        pending_commit_updates.append(&mut progress.updated_commits);

                        let now: Instant = Instant::now();
                        let should_push_ui: bool = progress.is_complete
                            || last_ui_push.is_none_or(|prev| now.duration_since(prev) >= MIN_UI_PROGRESS_INTERVAL);

                        if should_push_ui {
                            last_ui_push = Some(now);
                            progress.updated_commits = std::mem::take(&mut pending_commit_updates);
                            let win_step: Weak<AppWindow> = win_for_p2.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(win) = win_step.upgrade() {
                                    apply_phase2_progress_to_window(&win, &progress);
                                }
                            });
                        }
                    },
                );
            }
            Err(error) => {
                log::warn!("failed to run Phase 1 snapshot: {error}");
                let err_msg: String = format!("Error: {error}");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = win_weak_bg.upgrade() {
                        win.set_indexing_status_text(SharedString::from(err_msg));
                        win.set_is_indexing(false);
                    }
                });
            }
        }
    });
}

fn apply_phase1_snapshot_to_window(win: &AppWindow, snapshot: &Phase1Snapshot) {
    win.set_repo_name(SharedString::from(snapshot.repo_name.as_str()));
    win.set_repo_path(SharedString::from(snapshot.repo_path.as_str()));

    let ref_names: Vec<SharedString> =
        snapshot.refs.iter().map(|r| SharedString::from(r.display_name.as_str())).collect();
    win.set_ref_names(ModelRc::new(VecModel::from(ref_names)));
    win.set_active_ref(SharedString::from(snapshot.active_ref_name.as_str()));

    win.set_total_commits_text(SharedString::from(format_u64(u64::from(snapshot.total_commits))));
    win.set_total_contributors_text(SharedString::from(format_u64(snapshot.contributors.len() as u64)));

    let total_files: u64 = snapshot.languages.iter().map(|l| u64::from(l.files)).sum();
    let total_code: u64 = snapshot.languages.iter().map(|l| l.code).sum();
    win.set_total_files_text(SharedString::from(format_u64(total_files)));
    win.set_total_code_lines_text(SharedString::from(format_u64(total_code)));

    win.set_languages(to_ui_language_model(&snapshot.languages));
    win.set_heatmap_weeks(to_ui_heatmap_model(&snapshot.heatmap_weeks));
    apply_heatmap_summary_to_window(win, &snapshot.heatmap_weeks);
    win.set_contributors(to_ui_contributor_model(&snapshot.contributors));
    win.set_hotspots(to_ui_hotspot_model(&snapshot.hotspots));

    let buckets: &[ChurnTimeBucket] =
        if win.get_churn_monthly_mode() { &snapshot.monthly_churn } else { &snapshot.weekly_churn };
    win.set_churn_chart_image(render_churn_chart_image(buckets, DEFAULT_CHART_WIDTH, DEFAULT_CHART_HEIGHT));

    if snapshot.indexed_churn_commits >= snapshot.total_commits {
        win.set_indexing_status_text(SharedString::from(format!(
            "Indexed {}/{} commits (cached)",
            format_u64(u64::from(snapshot.indexed_churn_commits)),
            format_u64(u64::from(snapshot.total_commits))
        )));
        win.set_is_indexing(false);
    } else {
        win.set_indexing_status_text(SharedString::from(format!(
            "Indexing churn: {}/{}",
            format_u64(u64::from(snapshot.indexed_churn_commits)),
            format_u64(u64::from(snapshot.total_commits))
        )));
        win.set_is_indexing(true);
    }
}

fn apply_heatmap_summary_to_window(win: &AppWindow, weeks: &[HeatmapWeek]) {
    let mut total_52w_commits: u64 = 0;
    let mut active_days: u32 = 0;
    let mut total_days: u32 = 0;
    let mut peak_date: &str = "";
    let mut peak_commits: u32 = 0;

    for week in weeks {
        for day in &week.days {
            total_days += 1;
            total_52w_commits += u64::from(day.commit_count);
            if day.commit_count > 0 {
                active_days += 1;
            }
            if day.commit_count > peak_commits {
                peak_commits = day.commit_count;
                peak_date = day.date_iso.as_str();
            }
        }
    }

    win.set_heatmap_total_commits_text(SharedString::from(format_u64(total_52w_commits)));
    win.set_heatmap_active_days_text(SharedString::from(format!("{active_days} / {total_days} days")));
    if peak_commits > 0 {
        win.set_heatmap_peak_day_text(SharedString::from(format!(
            "{peak_date} ({})",
            format_u64(u64::from(peak_commits))
        )));
    } else {
        win.set_heatmap_peak_day_text(SharedString::from("None"));
    }
}

fn apply_phase2_progress_to_window(win: &AppWindow, progress: &ChurnBatchProgress) {
    win.set_contributors(to_ui_contributor_model(&progress.contributors));
    win.set_hotspots(to_ui_hotspot_model(&progress.hotspots));

    let buckets: &[ChurnTimeBucket] =
        if win.get_churn_monthly_mode() { &progress.monthly_churn } else { &progress.weekly_churn };
    win.set_churn_chart_image(render_churn_chart_image(buckets, DEFAULT_CHART_WIDTH, DEFAULT_CHART_HEIGHT));

    if !progress.updated_commits.is_empty() {
        let updates: HashMap<&str, (u32, u32, u32)> = progress
            .updated_commits
            .iter()
            .map(|(oid, files, ins, del)| (oid.as_str(), (*files, *ins, *del)))
            .collect();
        let current_model = win.get_commits();
        let row_count: usize = current_model.row_count();
        for idx in 0..row_count {
            if let Some(mut row) = current_model.row_data(idx)
                && let Some((files, ins, del)) = updates.get(row.oid_hex.as_str())
            {
                row.files_changed = *files as i32;
                row.insertions = *ins as i32;
                row.deletions = *del as i32;
                row.churn_indexed = true;
                current_model.set_row_data(idx, row);
            }
        }
    }

    if progress.is_complete {
        win.set_indexing_status_text(SharedString::from(format!(
            "Indexed {}/{} commits",
            format_u64(u64::from(progress.indexed_commits)),
            format_u64(u64::from(progress.total_commits))
        )));
        win.set_is_indexing(false);
    } else {
        win.set_indexing_status_text(SharedString::from(format!(
            "Indexing churn: {}/{}",
            format_u64(u64::from(progress.indexed_commits)),
            format_u64(u64::from(progress.total_commits))
        )));
        win.set_is_indexing(true);
    }
}

fn update_churn_chart_on_ui(win: &AppWindow, state: &ControllerState) {
    let buckets: &[ChurnTimeBucket] = if state.churn_monthly_mode { &state.monthly_churn } else { &state.weekly_churn };
    win.set_churn_chart_image(render_churn_chart_image(buckets, DEFAULT_CHART_WIDTH, DEFAULT_CHART_HEIGHT));
}

fn refresh_filtered_commits(win: &AppWindow, state_arc: &Arc<Mutex<ControllerState>>) -> Option<String> {
    let st = lock_state(state_arc);
    let query_lower: String = st.search_query.trim().to_ascii_lowercase();

    let filtered: Vec<UiCommitRow> = st
        .all_commits
        .iter()
        .filter(|commit| {
            if let Some(author) = &st.author_filter
                && &commit.author_name != author
            {
                return false;
            }
            if let Some(date) = &st.date_filter
                && &commit.date_iso != date
            {
                return false;
            }
            if !query_lower.is_empty() {
                let matches_summary: bool = commit.summary.to_ascii_lowercase().contains(&query_lower);
                let matches_author: bool = commit.author_name.to_ascii_lowercase().contains(&query_lower);
                let matches_oid: bool = commit.oid_hex.starts_with(&query_lower);
                if !matches_summary && !matches_author && !matches_oid {
                    return false;
                }
            }
            true
        })
        .take(MAX_VISIBLE_COMMITS)
        .map(to_ui_commit_row)
        .collect();

    let first_oid: Option<String> = filtered.first().map(|r| r.oid_hex.to_string());
    win.set_commits(ModelRc::new(VecModel::from(filtered)));
    first_oid
}

fn trigger_commit_selection(
    window_weak: &Weak<AppWindow>,
    state_arc: &Arc<Mutex<ControllerState>>,
    commit_oid_hex: &str,
) {
    let (repo_path, commit_meta) = {
        let mut st = lock_state(state_arc);
        commit_oid_hex.clone_into(&mut st.selected_commit_oid);
        let meta = st
            .commit_idx_by_oid
            .get(commit_oid_hex)
            .and_then(|&idx| st.all_commits.get(idx))
            .map(|c| (c.summary.clone(), c.author_name.clone(), c.date_iso.clone()));
        (st.repo_path.clone(), meta)
    };

    if let Some(win) = window_weak.upgrade() {
        win.set_selected_commit_oid(SharedString::from(commit_oid_hex));
        if let Some((summary, author, date)) = commit_meta {
            win.set_selected_commit_summary(SharedString::from(summary));
            win.set_selected_commit_author(SharedString::from(author));
            win.set_selected_commit_date(SharedString::from(date));
        }
    }

    let win_weak_bg: Weak<AppWindow> = window_weak.clone();
    let state_bg: Arc<Mutex<ControllerState>> = Arc::clone(state_arc);
    let oid_owned: String = commit_oid_hex.to_owned();
    let repo_for_diff: PathBuf = repo_path.clone();

    let _ = thread::Builder::new().name("git-oxide-changed-files".to_owned()).spawn(move || {
        let Ok(files) = load_commit_changed_files(&repo_path, &oid_owned) else {
            return;
        };
        let first_file_path: Option<String> = files.first().map(|f| f.path.clone());
        let initial_diff: Vec<DiffLineRow> = match &first_file_path {
            Some(path) => load_commit_file_diff(&repo_for_diff, &oid_owned, path).unwrap_or_default(),
            None => Vec::new(),
        };
        let is_split: bool = {
            let mut st = lock_state(&state_bg);
            st.current_diff_rows.clone_from(&initial_diff);
            st.is_split_diff
        };

        let _ = slint::invoke_from_event_loop(move || {
            if let Some(win) = win_weak_bg.upgrade() {
                win.set_changed_files(to_ui_changed_files_model(&files));
                win.set_selected_diff_file(SharedString::from(first_file_path.unwrap_or_default()));
                win.set_diff_rows(to_ui_diff_rows_model(&initial_diff, is_split));
            }
        });
    });
}

fn spawn_file_diff_load(
    window_weak: &Weak<AppWindow>,
    state_arc: &Arc<Mutex<ControllerState>>,
    commit_oid_hex: &str,
    file_path: &str,
) {
    let repo_path: PathBuf = lock_state(state_arc).repo_path.clone();
    let oid_owned: String = commit_oid_hex.to_owned();
    let path_owned: String = file_path.to_owned();
    let win_weak_bg: Weak<AppWindow> = window_weak.clone();
    let state_bg: Arc<Mutex<ControllerState>> = Arc::clone(state_arc);

    if let Some(win) = window_weak.upgrade() {
        win.set_selected_diff_file(SharedString::from(file_path));
    }

    let _ = thread::Builder::new().name("git-oxide-diff".to_owned()).spawn(move || {
        if let Ok(rows) = load_commit_file_diff(&repo_path, &oid_owned, &path_owned) {
            let is_split: bool = {
                let mut st = lock_state(&state_bg);
                st.current_diff_rows.clone_from(&rows);
                st.is_split_diff
            };
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = win_weak_bg.upgrade() {
                    win.set_diff_rows(to_ui_diff_rows_model(&rows, is_split));
                }
            });
        }
    });
}

fn spawn_tree_entries_load(
    window_weak: &Weak<AppWindow>,
    state_arc: &Arc<Mutex<ControllerState>>,
    select_path_after: Option<String>,
) {
    let (repo_path, ref_oid) = {
        let st = lock_state(state_arc);
        (st.repo_path.clone(), st.active_ref_oid_hex.clone())
    };
    if ref_oid.is_empty() {
        return;
    }

    let win_weak_bg: Weak<AppWindow> = window_weak.clone();
    let state_bg: Arc<Mutex<ControllerState>> = Arc::clone(state_arc);

    let _ = thread::Builder::new().name("git-oxide-tree".to_owned()).spawn(move || {
        if let Ok(entries) = load_ref_tree_entries(&repo_path, &ref_oid) {
            let target_file: Option<String> = select_path_after.or_else(|| entries.first().map(|e| e.path.clone()));
            {
                let mut st = lock_state(&state_bg);
                st.all_tree_entries = entries;
            }
            let win_ui: Weak<AppWindow> = win_weak_bg.clone();
            let state_ui: Arc<Mutex<ControllerState>> = Arc::clone(&state_bg);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = win_ui.upgrade() {
                    refresh_filtered_tree_entries(&win, &lock_state(&state_ui));
                }
                if let Some(path) = target_file {
                    trigger_tree_file_selection(&win_ui, &state_ui, &path);
                }
            });
        }
    });
}

fn refresh_filtered_tree_entries(win: &AppWindow, state: &ControllerState) {
    let query_lower: String = state.file_filter_query.trim().to_ascii_lowercase();
    let rows: Vec<UiTreeEntryRow> = state
        .all_tree_entries
        .iter()
        .filter(|entry| query_lower.is_empty() || entry.path.to_ascii_lowercase().contains(&query_lower))
        .take(MAX_VISIBLE_TREE_ENTRIES)
        .map(|entry| {
            let kib: u64 = (entry.size_bytes / BYTES_PER_KIB).max(1);
            UiTreeEntryRow {
                path: SharedString::from(entry.path.as_str()),
                file_name: SharedString::from(entry.file_name.as_str()),
                depth: entry.depth as i32,
                language: SharedString::from(entry.language.as_str()),
                size_text: SharedString::from(format!("{} KiB", format_u64(kib))),
            }
        })
        .collect();
    win.set_tree_entries(ModelRc::new(VecModel::from(rows)));
}

fn trigger_tree_file_selection(
    window_weak: &Weak<AppWindow>,
    state_arc: &Arc<Mutex<ControllerState>>,
    file_path: &str,
) {
    let (repo_path, ref_oid, show_blame) = {
        let mut st = lock_state(state_arc);
        file_path.clone_into(&mut st.selected_tree_file);
        (st.repo_path.clone(), st.active_ref_oid_hex.clone(), st.show_blame_gutter)
    };
    if ref_oid.is_empty() {
        return;
    }

    if let Some(win) = window_weak.upgrade() {
        win.set_selected_tree_file(SharedString::from(file_path));
    }

    let win_weak_bg: Weak<AppWindow> = window_weak.clone();
    let path_owned: String = file_path.to_owned();

    let _ = thread::Builder::new().name("git-oxide-blame".to_owned()).spawn(move || {
        let result: Result<Vec<BlameLineRow>, _> = if show_blame {
            load_ref_file_blame(&repo_path, &ref_oid, &path_owned)
        } else {
            load_ref_file_source(&repo_path, &ref_oid, &path_owned)
        };

        if let Ok(rows) = result {
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(win) = win_weak_bg.upgrade() {
                    win.set_blame_rows(to_ui_blame_rows_model(&rows));
                }
            });
        }
    });
}

fn to_ui_commit_row(commit: &CommitSummary) -> UiCommitRow {
    UiCommitRow {
        oid_hex: SharedString::from(commit.oid_hex.as_str()),
        short_oid: SharedString::from(commit.short_oid.as_str()),
        summary: SharedString::from(commit.summary.as_str()),
        author_name: SharedString::from(commit.author_name.as_str()),
        date_iso: SharedString::from(commit.date_iso.as_str()),
        insertions: commit.insertions as i32,
        deletions: commit.deletions as i32,
        files_changed: commit.files_changed as i32,
        churn_indexed: commit.churn_indexed,
    }
}

fn to_ui_changed_files_model(files: &[ChangedFileStat]) -> ModelRc<UiChangedFileRow> {
    let rows: Vec<UiChangedFileRow> = files
        .iter()
        .map(|f| {
            let display_path: String = match &f.old_path {
                Some(old) => format!("{old} -> {}", f.path),
                None => f.path.clone(),
            };
            UiChangedFileRow {
                path: SharedString::from(f.path.as_str()),
                display_path: SharedString::from(display_path),
                status: SharedString::from(f.status.badge_str()),
                insertions: f.insertions as i32,
                deletions: f.deletions as i32,
            }
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn to_ui_spans(spans: &[CodeTokenSpan]) -> ModelRc<UiTokenSpan> {
    let ui_spans: Vec<UiTokenSpan> = spans
        .iter()
        .map(|span| UiTokenSpan {
            text: SharedString::from(span.text.as_str()),
            color: Color::from_argb_u8(span.color.a, span.color.r, span.color.g, span.color.b),
        })
        .collect();
    ModelRc::new(VecModel::from(ui_spans))
}

fn to_ui_diff_rows_model(rows: &[DiffLineRow], is_split_diff: bool) -> ModelRc<UiDiffRow> {
    let ui_rows: Vec<UiDiffRow> = rows
        .iter()
        .filter(|row| {
            if is_split_diff {
                !(row.left_kind == DiffLineKind::EmptyPlaceholder && row.right_kind == DiffLineKind::EmptyPlaceholder)
            } else {
                true
            }
        })
        .map(|row| UiDiffRow {
            old_line: row.old_line_no.map_or_else(SharedString::default, |n| SharedString::from(n.to_string())),
            new_line: row.new_line_no.map_or_else(SharedString::default, |n| SharedString::from(n.to_string())),
            kind: row.kind.as_i32(),
            unified_spans: to_ui_spans(&row.unified_spans),
            left_line: row.left_line_no.map_or_else(SharedString::default, |n| SharedString::from(n.to_string())),
            left_kind: row.left_kind.as_i32(),
            left_spans: to_ui_spans(&row.left_spans),
            right_line: row.right_line_no.map_or_else(SharedString::default, |n| SharedString::from(n.to_string())),
            right_kind: row.right_kind.as_i32(),
            right_spans: to_ui_spans(&row.right_spans),
        })
        .collect();
    ModelRc::new(VecModel::from(ui_rows))
}

fn to_ui_blame_rows_model(rows: &[BlameLineRow]) -> ModelRc<UiBlameRow> {
    let ui_rows: Vec<UiBlameRow> = rows
        .iter()
        .map(|row| UiBlameRow {
            line_no: SharedString::from(row.line_no.to_string()),
            commit_oid_hex: SharedString::from(row.commit_oid_hex.as_str()),
            short_oid: SharedString::from(row.short_oid.as_str()),
            author_name: SharedString::from(row.author_name.as_str()),
            date_iso: SharedString::from(row.date_iso.as_str()),
            is_hunk_start: row.is_hunk_start,
            spans: to_ui_spans(&row.spans),
        })
        .collect();
    ModelRc::new(VecModel::from(ui_rows))
}

fn to_ui_language_model(languages: &[LanguageStat]) -> ModelRc<UiLanguageRow> {
    let rows: Vec<UiLanguageRow> = languages
        .iter()
        .map(|lang| UiLanguageRow {
            language: SharedString::from(lang.language.as_str()),
            files: lang.files as i32,
            code: lang.code as i32,
            comments: lang.comments as i32,
            blanks: lang.blanks as i32,
            files_text: SharedString::from(format_u64(u64::from(lang.files))),
            code_text: SharedString::from(format_u64(lang.code)),
            comments_text: SharedString::from(format_u64(lang.comments)),
            blanks_text: SharedString::from(format_u64(lang.blanks)),
            percentage_text: SharedString::from(format!("{:.1}%", lang.percentage)),
            share_fraction: (lang.percentage / 100.0).clamp(0.008, 1.0),
            ratio: lang.percentage.max(0.04),
            color: Color::from_argb_u8(lang.color.a, lang.color.r, lang.color.g, lang.color.b),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn to_ui_contributor_model(contributors: &[ContributorStat]) -> ModelRc<UiContributorRow> {
    let rows: Vec<UiContributorRow> = contributors
        .iter()
        .map(|c| UiContributorRow {
            name: SharedString::from(c.name.as_str()),
            email: SharedString::from(c.email.as_str()),
            commits: c.commits as i32,
            insertions: c.insertions as i32,
            deletions: c.deletions as i32,
            commits_text: SharedString::from(format_u64(u64::from(c.commits))),
            insertions_text: SharedString::from(format_u64(c.insertions)),
            deletions_text: SharedString::from(format_u64(c.deletions)),
            active_range: SharedString::from(format!("{} -> {}", c.first_commit_date, c.last_commit_date)),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn to_ui_hotspot_model(hotspots: &[FileHotspot]) -> ModelRc<UiHotspotRow> {
    let rows: Vec<UiHotspotRow> = hotspots
        .iter()
        .map(|h| UiHotspotRow {
            path: SharedString::from(h.path.as_str()),
            language: SharedString::from(h.language.as_str()),
            touches: h.touches as i32,
            insertions: h.insertions as i32,
            deletions: h.deletions as i32,
            total_churn: h.total_churn as i32,
            touches_text: SharedString::from(format_u64(u64::from(h.touches))),
            insertions_text: SharedString::from(format_u64(h.insertions)),
            deletions_text: SharedString::from(format_u64(h.deletions)),
            total_churn_text: SharedString::from(format_u64(h.total_churn)),
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn to_ui_heatmap_model(weeks: &[HeatmapWeek]) -> ModelRc<UiHeatmapWeek> {
    let ui_weeks: Vec<UiHeatmapWeek> = weeks
        .iter()
        .map(|w| {
            let days: Vec<UiHeatmapDay> = w
                .days
                .iter()
                .map(|d| UiHeatmapDay {
                    date_iso: SharedString::from(d.date_iso.as_str()),
                    commit_count: d.commit_count as i32,
                    intensity: i32::from(d.intensity_level),
                })
                .collect();
            UiHeatmapWeek {
                week_label: SharedString::from(w.week_label.as_str()),
                days: ModelRc::new(VecModel::from(days)),
            }
        })
        .collect();
    ModelRc::new(VecModel::from(ui_weeks))
}

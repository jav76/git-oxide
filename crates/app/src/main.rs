#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod charts;
mod controller;

use std::env;
use std::path::PathBuf;

use anyhow::{Context, Result};
use git_oxide_engine::cache::OidCache;
use slint::ComponentHandle;

slint::include_modules!();

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .filter_module("tokei", log::LevelFilter::Error)
        .init();

    let initial_repo_path: PathBuf =
        env::args().nth(1).map(PathBuf::from).or_else(|| env::current_dir().ok()).unwrap_or_else(|| PathBuf::from("."));
    let initial_ref: Option<String> = env::args().nth(2);

    let cache: OidCache = OidCache::open_default().context("failed to initialize redb oid cache")?;
    let window: AppWindow = AppWindow::new().context("failed to initialize slint application window")?;

    controller::attach_controller(&window, cache, initial_repo_path, initial_ref);
    window.run().context("slint event loop terminated with error")
}

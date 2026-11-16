//! Shared repository discovery, linked worktree detection, and transient object read retries.

use std::borrow::Cow;
use std::error::Error as StdError;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::error::EngineError;

/// Delay before retrying an operation that failed to read the object database.
///
/// On Windows, `git gc` or `git maintenance` running in another worktree can briefly lock or replace pack files.
pub const OBJECT_READ_RETRY_DELAY: Duration = Duration::from_millis(250);

const MAIN_GIT_DIR_NAME: &str = ".git";

/// Discovers and opens the repository that contains `repo_path`, including linked worktrees.
pub fn open_repository(repo_path: &Path) -> Result<gix::Repository, EngineError> {
    gix::discover(repo_path).map_err(|error| EngineError::RepositoryOpen {
        path: repo_path.display().to_string(),
        message: error_chain(&error),
    })
}

/// Returns the main worktree path (or bare repository path) when `repository` is a linked worktree.
pub fn linked_worktree_parent(repository: &gix::Repository) -> Option<PathBuf> {
    let gix::repository::Kind::WorkTree { is_linked: true } = repository.kind() else {
        return None;
    };
    // gix joins `commondir` verbatim, yielding `<main>/.git/worktrees/<name>/../..`.
    let raw_common_dir: &Path = repository.common_dir();
    let common_dir: PathBuf = gix::path::normalize(Cow::Borrowed(raw_common_dir), repository.git_dir())
        .map_or_else(|| raw_common_dir.to_path_buf(), Cow::into_owned);
    let parent: PathBuf = match common_dir.file_name() {
        Some(name) if name == MAIN_GIT_DIR_NAME => {
            common_dir.parent().map_or_else(|| common_dir.clone(), Path::to_path_buf)
        }
        _ => common_dir,
    };
    Some(parent)
}

/// Runs `operation`, retrying it once after [`OBJECT_READ_RETRY_DELAY`] if it fails with
/// [`EngineError::ObjectRead`].
pub fn retry_on_object_read<T>(mut operation: impl FnMut() -> Result<T, EngineError>) -> Result<T, EngineError> {
    match operation() {
        Err(EngineError::ObjectRead(message)) => {
            log::debug!("retrying after object database read failure: {message}");
            thread::sleep(OBJECT_READ_RETRY_DELAY);
            operation()
        }
        result => result,
    }
}

/// Formats `error` and every `source()` beneath it as `outer: inner: root`, skipping repeated messages
/// produced by `#[error(transparent)]` wrappers.
pub fn error_chain(error: &(dyn StdError + 'static)) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut current: Option<&(dyn StdError + 'static)> = Some(error);
    while let Some(layer) = current {
        let message: String = layer.to_string();
        if parts.last().is_none_or(|previous| !previous.contains(&message)) {
            parts.push(message);
        }
        current = layer.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    #[derive(Debug)]
    struct Layer {
        message: &'static str,
        source: Option<Box<Layer>>,
    }

    impl fmt::Display for Layer {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(self.message)
        }
    }

    impl StdError for Layer {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            self.source.as_deref().map(|layer| layer as &(dyn StdError + 'static))
        }
    }

    fn layered(messages: &[&'static str]) -> Layer {
        let mut chain: Option<Box<Layer>> = None;
        for message in messages.iter().rev() {
            chain = Some(Box::new(Layer { message, source: chain }));
        }
        *chain.expect("test chains always have at least one layer")
    }

    #[test]
    fn error_chain_includes_every_source() {
        let error: Layer = layered(&["could not peel HEAD", "failed to load pack index", "access denied"]);
        assert_eq!(error_chain(&error), "could not peel HEAD: failed to load pack index: access denied");
    }

    #[test]
    fn error_chain_skips_transparent_repeats() {
        let error: Layer = layered(&["pack missing", "pack missing", "os error 2"]);
        assert_eq!(error_chain(&error), "pack missing: os error 2");
    }

    #[test]
    fn retry_runs_once_more_only_for_object_read_errors() {
        let mut attempts: u32 = 0;
        let result: Result<u32, EngineError> = retry_on_object_read(|| {
            attempts += 1;
            if attempts == 1 { Err(EngineError::ObjectRead("locked".to_owned())) } else { Ok(attempts) }
        });
        assert_eq!(result.ok(), Some(2));

        let mut other_attempts: u32 = 0;
        let other: Result<(), EngineError> = retry_on_object_read(|| {
            other_attempts += 1;
            Err(EngineError::Cancelled)
        });
        assert!(matches!(other, Err(EngineError::Cancelled)));
        assert_eq!(other_attempts, 1);
    }
}

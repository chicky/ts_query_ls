use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicI32},
};

use futures::future::join_all;
use ropey::Rope;
use tower_lsp::lsp_types::{DiagnosticSeverity, Url};
use ts_query_ls::Options;

use crate::{
    DocumentData, LanguageData, QUERY_LANGUAGE, handlers::diagnostic::get_diagnostics,
    util::get_language_name,
};

use super::get_scm_files;

pub(super) async fn lint_file(
    path: &Path,
    uri: &Url,
    source: &str,
    options: Arc<tokio::sync::RwLock<Options>>,
    language_data: Option<Arc<LanguageData>>,
    exit_code: &AtomicI32,
) {
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&QUERY_LANGUAGE)
        .expect("Error loading Query grammar");
    let tree = parser.parse(source, None).unwrap();
    let rope = Rope::from(source);
    let language_name = get_language_name(uri, &*options.read().await);
    let doc = DocumentData {
        tree,
        rope,
        language_name,
        version: Default::default(),
    };
    // The query construction already validates node names, fields, supertypes,
    // etc.
    let diagnostics = get_diagnostics(uri, doc, language_data, options, false).await;
    if !diagnostics.is_empty() {
        exit_code.store(1, std::sync::atomic::Ordering::Relaxed);
        for diagnostic in diagnostics {
            let kind = match diagnostic.severity {
                Some(DiagnosticSeverity::ERROR) => "Error",
                Some(DiagnosticSeverity::WARNING) => "Warning",
                Some(DiagnosticSeverity::INFORMATION) => "Info",
                Some(DiagnosticSeverity::HINT) => "Hint",
                _ => "Diagnostic",
            };
            eprintln!(
                "{} in \"{}\" on line {}, col {}:\n  {}",
                kind,
                path.to_str().unwrap(),
                diagnostic.range.start.line,
                diagnostic.range.start.character,
                diagnostic.message
            );
        }
    }
}

/// Lint all the given directories according to the given configuration. Linting covers things like
/// invalid capture names or predicate signatures, but not errors like invalid node names or
/// impossible patterns.
pub async fn lint_directories(directories: &[PathBuf], config: String) -> i32 {
    let Ok(options) = serde_json::from_str::<Options>(&config) else {
        eprintln!("Could not parse the provided configuration");
        return 1;
    };
    let options: Arc<tokio::sync::RwLock<Options>> = Arc::new(options.into());
    let exit_code = Arc::new(AtomicI32::new(0));
    // If directories are not specified, lint all files in the current directory
    let scm_files = if directories.is_empty() {
        get_scm_files(&[env::current_dir().expect("Failed to get current directory")])
    } else {
        get_scm_files(directories)
    };
    let tasks = scm_files.into_iter().filter_map(|path| {
        let uri = Url::from_file_path(path.canonicalize().unwrap()).unwrap();
        let exit_code = exit_code.clone();
        let options = options.clone();
        if let Ok(source) = fs::read_to_string(&path) {
            Some(tokio::spawn(async move {
                lint_file(&path, &uri, &source, options, None, &exit_code).await;
            }))
        } else {
            eprintln!("Failed to read {:?}", path.canonicalize().unwrap());
            exit_code.store(1, std::sync::atomic::Ordering::Relaxed);
            None
        }
    });
    join_all(tasks).await;
    exit_code.load(std::sync::atomic::Ordering::Relaxed)
}

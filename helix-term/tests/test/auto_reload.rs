use std::{
    fs::File,
    io::Write,
    time::{Duration, SystemTime},
};

use helix_term::application::Application;
use helix_view::doc;
use helix_view::editor::AutoReload;

use super::*;

fn auto_reload_app(file: &tempfile::NamedTempFile) -> anyhow::Result<Application> {
    let mut config = test_config();
    config.editor.auto_reload = AutoReload {
        interval: 10,
        ..Default::default()
    };
    AppBuilder::new()
        .with_config(config)
        .with_file(file.path(), None)
        .build()
}

/// Overwrite the file as an external process would, bumping the mtime so the change is
/// detected even on filesystems with coarse timestamps.
fn write_externally(file: &tempfile::NamedTempFile, contents: &str) -> anyhow::Result<()> {
    let mut f = File::create(file.path())?;
    f.write_all(contents.as_bytes())?;
    f.set_modified(SystemTime::now() + Duration::from_secs(10))?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn reloads_unmodified_buffer_changed_on_disk() -> anyhow::Result<()> {
    let file = temp_file_with_contents("before\n")?;
    let mut app = auto_reload_app(&file)?;

    write_externally(&file, "after\n")?;

    test_key_sequence(
        &mut app,
        // Any key: the reload timer fires while the event loop waits for idle.
        Some("<esc>"),
        Some(&|app| {
            let doc = doc!(app.editor);
            assert_eq!("after\n", doc.text().to_string());
            assert!(!doc.is_modified());
            assert_status_not_error(&app.editor);
        }),
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn keeps_modified_buffer_changed_on_disk() -> anyhow::Result<()> {
    let file = temp_file_with_contents("before\n")?;
    let mut app = auto_reload_app(&file)?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some("ihello <esc>"),
                Some(&|_| write_externally(&file, "after\n").unwrap()),
            ),
            (
                Some("<esc>"),
                Some(&|app| {
                    let doc = doc!(app.editor);
                    assert_eq!("hello before\n", doc.text().to_string());
                    assert!(doc.is_modified());
                    let (msg, severity) = app.editor.get_status().unwrap();
                    assert_eq!(&helix_view::editor::Severity::Warning, severity);
                    assert!(msg.contains("changed on disk"), "{msg}");
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

fn git(args: &str, dir: &std::path::Path) {
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args.split_whitespace())
        .env_remove("GIT_DIR")
        .env("GIT_CONFIG_COUNT", "4")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_1", "main")
        .env("GIT_CONFIG_KEY_2", "user.name")
        .env("GIT_CONFIG_VALUE_2", "helix-test")
        .env("GIT_CONFIG_KEY_3", "user.email")
        .env("GIT_CONFIG_VALUE_3", "test@helix.org")
        .output()
        .expect("failed to run git")
        .status;
    assert!(status.success(), "`git {args}` failed");
}

#[tokio::test(flavor = "multi_thread")]
async fn refreshes_vcs_state_after_external_checkout() -> anyhow::Result<()> {
    let repo = tempfile::tempdir()?;
    let path = repo.path().join("file.txt");
    std::fs::write(&path, "foo\n")?;
    git("init", repo.path());
    git("add -A", repo.path());
    git("commit -m init", repo.path());

    let mut config = test_config();
    config.editor.auto_reload = AutoReload {
        interval: 10,
        ..Default::default()
    };
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(&path, None)
        .build()?;

    let head = |app: &Application| doc!(app.editor).version_control_head().unwrap().to_string();
    test_key_sequences(
        &mut app,
        vec![
            (
                None,
                Some(&|app| {
                    assert_eq!("main", head(app));
                    git("checkout -b other", repo.path());
                }),
            ),
            (Some("<esc>"), Some(&|app| assert_eq!("other", head(app)))),
        ],
        false,
    )
    .await?;

    Ok(())
}

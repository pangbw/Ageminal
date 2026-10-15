//! 全局 excludes（`core.excludesFile`）的保真度（#18 D2；#58 验收 1 的另一半）。
//!
//! 单开一个测试二进制：本用例要改**本进程**的环境变量（`GIT_CONFIG_GLOBAL`），把全局忽略
//! 规则钉在可控的临时文件上——放独立进程就不会波及其它用例。钉好之后，我们的匹配器与
//! `git check-ignore` 读的是同一份配置，于是可以逐条对照。

mod common;

use std::fs;

use ageminal_files::read_dir;
use common::{check_ignore, relative, run_git, write, TempDir};

#[test]
fn honors_the_global_excludes_file() {
    let dir = TempDir::new("global-excludes");
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "-b", "main"]);

    write(&root, "visible.txt", "v\n");
    write(&root, "globally.txt", "g\n");

    // 一条只存在于全局 excludes 里的规则。
    let excludes = dir.path().join("global-ignore");
    fs::write(&excludes, "globally.txt\n").unwrap();

    // 让本进程的匹配器与子进程 git 都只认这份全局配置，不碰宿主真实配置。
    //
    // `ignore` 找全局配置的顺序：`GIT_CONFIG_GLOBAL` → `$HOME/.gitconfig` → XDG；而它的
    // `home_dir()` 在 **Windows 看 `USERPROFILE`**、在 Unix 看 `HOME`。所以四个环境变量一起钉到
    // 临时目录，两个平台都确定（只在 CI runner 上才暴露过 HOME 不够这个问题）。
    let home = dir.path().join("home");
    fs::create_dir_all(home.join(".config").join("git")).unwrap();
    let config = home.join(".gitconfig");
    fs::write(
        &config,
        format!(
            "[core]\n\texcludesFile = {}\n",
            excludes.to_string_lossy().replace('\\', "/")
        ),
    )
    .unwrap();

    std::env::set_var("HOME", &home);
    std::env::set_var("USERPROFILE", &home);
    std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    std::env::set_var("GIT_CONFIG_GLOBAL", &config);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");

    let entries = read_dir(&root, &root).unwrap().entries;
    let paths: Vec<String> = entries
        .iter()
        .map(|entry| relative(&root, &entry.path))
        .collect();

    // 与裁判同源：同一份 GIT_CONFIG_GLOBAL 下，git 也该命中。
    let ignored = check_ignore(&root, &paths);

    // [DEBUG-ci-global] CI runner 上的平台差异诊断；定位后删。
    eprintln!("[DEBUG-ci-global] temp_dir={:?}", std::env::temp_dir());
    eprintln!(
        "[DEBUG-ci-global] excludes={:?} exists={}",
        excludes,
        excludes.exists()
    );
    eprintln!(
        "[DEBUG-ci-global] config={:?}",
        fs::read_to_string(&config).unwrap_or_default()
    );
    eprintln!(
        "[DEBUG-ci-global] entries={:?}",
        entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.ignored))
            .collect::<Vec<_>>()
    );
    eprintln!("[DEBUG-ci-global] git check-ignore={ignored:?}");

    let global = entries
        .iter()
        .find(|entry| entry.name == "globally.txt")
        .expect("夹具条目应在结果里");
    assert!(
        ignored.contains("globally.txt"),
        "裁判也应命中：{ignored:?}"
    );
    assert!(global.ignored, "全局 excludes 应命中 globally.txt");
}

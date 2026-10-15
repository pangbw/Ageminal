//! 全局 excludes 的保真度（#18 D2；#58 验收 1 的另一半）。
//!
//! 单开一个测试二进制：本用例要改**本进程**的环境变量，把全局忽略规则钉在可控的临时文件上
//! ——放独立进程就不会波及其它用例。钉好之后，我们的匹配器与 `git check-ignore` 读的是同一份
//! 全局配置，于是可以逐条对照。
//!
//! 走 git 的**默认**全局忽略位置（`$XDG_CONFIG_HOME/git/ignore`），而不是 `core.excludesFile`：
//! `ignore` crate 解析 `core.excludesFile` 时用 `expand_tilde` 把路径里**任何** `~` 都替换成
//! home，而 CI runner 的临时目录是 8.3 短名（`RUNNER~1`），会被改坏（上游的坑，本机正常所以
//! 一直没暴露）。默认位置不做这步展开，跨平台都确定。两条路径最终都汇到
//! `ignore::gitignore::gitconfig_excludes_path()`，所以「读全局 excludes」这件事照样被覆盖。

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

    // 让本进程的匹配器与子进程 git 都只认这份全局配置，不碰宿主真实配置。
    // `ignore` 的 `home_dir()` 在 Windows 看 `USERPROFILE`、在 Unix 看 `HOME`——都钉到临时目录。
    let home = dir.path().join("home");
    let xdg = home.join(".config");
    let git_config_dir = xdg.join("git");
    fs::create_dir_all(&git_config_dir).unwrap();
    // 默认全局忽略位置：一条只存在于全局 excludes 里的规则。
    fs::write(git_config_dir.join("ignore"), "globally.txt\n").unwrap();
    // 空的全局 config：明确不让宿主 / runner 的 `core.excludesFile` 掺进来。
    let config = home.join(".gitconfig");
    fs::write(&config, "").unwrap();

    std::env::set_var("HOME", &home);
    std::env::set_var("USERPROFILE", &home);
    std::env::set_var("XDG_CONFIG_HOME", &xdg);
    std::env::set_var("GIT_CONFIG_GLOBAL", &config);
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");

    let entries = read_dir(&root, &root).unwrap().entries;
    let paths: Vec<String> = entries
        .iter()
        .map(|entry| relative(&root, &entry.path))
        .collect();

    // 与裁判同源：同一份全局配置下，git 也该命中。
    let ignored = check_ignore(&root, &paths);
    assert!(
        ignored.contains("globally.txt"),
        "裁判也应命中：{ignored:?}"
    );

    let global = entries
        .iter()
        .find(|entry| entry.name == "globally.txt")
        .expect("夹具条目应在结果里");
    assert!(global.ignored, "全局 excludes 应命中 globally.txt");
}

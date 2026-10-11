//! 工作区模型与投影的行为测试（纯逻辑，无进程、无文件系统）。

use std::path::{Path, PathBuf};

use ageminal_git::worktree::{Marker, Worktree};
use ageminal_workspace::{project_view, Discovery, Project, ProjectSettings, WorktreeName};
use serde_json::json;

fn path(value: &str) -> PathBuf {
    PathBuf::from(value)
}

/// 一个「默认」worktree；各用例只改自己关心的字段。
fn worktree(path_value: &str, name: &str, is_main: bool) -> Worktree {
    Worktree {
        path: path(path_value),
        name: name.to_owned(),
        is_main,
        head: None,
        branch: None,
        branch_label: None,
        detached: false,
        bare: false,
        locked: Marker::Absent,
        prunable: Marker::Absent,
        stale: false,
    }
}

fn names(entries: &[(&str, &str)]) -> Vec<WorktreeName> {
    entries
        .iter()
        .map(|(path_value, name)| WorktreeName {
            path: path(path_value),
            name: (*name).to_owned(),
            extra: Default::default(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Project（state.json）
// ---------------------------------------------------------------------------

#[test]
fn default_project_name_is_directory_basename() {
    assert_eq!(Project::default_name(Path::new("/ws/proj-1")), "proj-1");
    assert_eq!(
        Project::default_name(Path::new("/ws/trailing/")),
        "trailing"
    );
}

#[test]
fn new_project_pins_every_discovered_worktree() {
    // #57 验收：加仓库后侧栏出现其 worktree。
    let discovered = [
        worktree("/ws/proj", "proj", true),
        worktree("/ws/proj/feat", "feat", false),
    ];

    let project = Project::new(path("/ws/proj"), "proj".to_owned(), &discovered);

    assert_eq!(
        project.pinned,
        vec![path("/ws/proj"), path("/ws/proj/feat")]
    );
    assert!(!project.collapsed);
}

#[test]
fn pinning_toggles_membership_and_is_idempotent() {
    let mut project = Project::new(
        path("/ws/proj"),
        "proj".to_owned(),
        &[
            worktree("/ws/proj", "proj", true),
            worktree("/ws/proj/dev", "dev", false),
        ],
    );

    project.set_pinned(Path::new("/ws/proj"), false);
    assert!(!project.is_pinned(Path::new("/ws/proj")));
    project.set_pinned(Path::new("/ws/proj"), false);
    assert_eq!(project.pinned, vec![path("/ws/proj/dev")]);

    project.set_pinned(Path::new("/ws/proj"), true);
    assert!(project.is_pinned(Path::new("/ws/proj")));
    project.set_pinned(Path::new("/ws/proj"), true);
    assert_eq!(project.pinned.len(), 2, "重复加入不应产生重复项");
}

#[test]
fn project_serde_uses_camel_case_with_defaults() {
    // 缺 collapsed / pinned 的旧文档要能读，且默认「未折叠 + 未加入」。
    let project: Project = serde_json::from_value(json!({
        "path": "D:\\ws\\proj",
        "name": "proj",
    }))
    .unwrap();

    assert!(!project.collapsed);
    assert!(project.pinned.is_empty());

    let value = serde_json::to_value(&project).unwrap();
    assert_eq!(value["collapsed"], json!(false));
    assert_eq!(value["pinned"], json!([]));
    assert_eq!(value["path"], json!("D:\\ws\\proj"));
}

#[test]
fn project_keeps_unknown_fields() {
    let mut project: Project = serde_json::from_value(json!({
        "path": "/ws/proj",
        "name": "proj",
        "collapsed": true,
        "pinned": ["/ws/proj/feat"],
        "futureField": { "nested": 1 },
    }))
    .unwrap();

    assert!(project.collapsed);
    project.set_pinned(Path::new("/ws/proj/feat"), false);

    let value = serde_json::to_value(&project).unwrap();
    assert_eq!(
        value["futureField"]["nested"],
        json!(1),
        "未知字段必须原样保留"
    );
}

#[test]
fn project_owns_worktrees_by_path_components() {
    let project = Project::new(path("/ws/proj"), "proj".to_owned(), &[]);

    assert!(project.owns(Path::new("/ws/proj")));
    assert!(project.owns(Path::new("/ws/proj/feat")));
    // **不是**字符串前缀：兄弟目录不算自家
    assert!(!project.owns(Path::new("/ws/proj-2")));
    assert!(!project.owns(Path::new("/ws/other")));
}

// ---------------------------------------------------------------------------
// ProjectSettings / WorktreeName（settings.json 的项目级）
// ---------------------------------------------------------------------------

#[test]
fn normalize_name_drops_blank_names() {
    assert_eq!(
        ProjectSettings::normalize_name(Some("  feat  ")),
        Some("feat")
    );
    assert_eq!(ProjectSettings::normalize_name(Some("   ")), None);
    assert_eq!(ProjectSettings::normalize_name(Some("")), None);
    assert_eq!(ProjectSettings::normalize_name(None), None);
}

#[test]
fn empty_settings_entry_is_detected() {
    let mut settings = ProjectSettings::new(path("/ws/proj"));
    assert!(settings.is_empty());

    settings.rename(Path::new("/ws/proj/feat"), Some("feat"));
    assert!(!settings.is_empty());

    settings.rename(Path::new("/ws/proj/feat"), None);
    assert!(settings.is_empty(), "清掉显示名后又空了");

    // #86 会往 extra 里塞 shell 覆盖 / 环境变量等，那时它就不再是空条目
    let mut with_extra = ProjectSettings::new(path("/ws/proj"));
    with_extra
        .extra
        .insert("shellOverride".to_owned(), json!("bash"));
    assert!(!with_extra.is_empty());
}

#[test]
fn rename_sets_and_clears_override() {
    let mut settings = ProjectSettings::new(path("/ws/proj"));

    settings.rename(Path::new("/ws/proj/feat"), Some("  我的特性  "));
    assert_eq!(
        settings.name_of(Path::new("/ws/proj/feat")),
        Some("我的特性"),
        "两侧空白应去掉"
    );
    assert_eq!(settings.name_of(Path::new("/ws/proj")), None);

    // 同名再设 = 覆盖，不新增
    settings.rename(Path::new("/ws/proj/feat"), Some("改名"));
    assert_eq!(settings.worktree_names.len(), 1);

    // None 与「只有空白」都清除覆盖、回到默认名
    settings.rename(Path::new("/ws/proj/feat"), None);
    assert_eq!(settings.name_of(Path::new("/ws/proj/feat")), None);
    settings.rename(Path::new("/ws/proj/feat"), Some("   "));
    assert_eq!(settings.name_of(Path::new("/ws/proj/feat")), None);
}

#[test]
fn settings_keep_unknown_fields_for_later_sections() {
    // #86 会往同一表项里加 shell 覆盖 / 环境变量 / agent 覆盖。
    let settings: ProjectSettings = serde_json::from_value(json!({
        "path": "/ws/proj",
        "worktreeNames": [{ "path": "/ws/proj/feat", "name": "feat", "innerUnknown": true }],
        "shellOverride": "C:\\Program Files\\Git\\bin\\bash.exe",
    }))
    .unwrap();

    assert_eq!(settings.name_of(Path::new("/ws/proj/feat")), Some("feat"));
    let value = serde_json::to_value(&settings).unwrap();
    assert_eq!(
        value["shellOverride"],
        json!("C:\\Program Files\\Git\\bin\\bash.exe")
    );
    assert_eq!(
        value["worktreeNames"][0]["innerUnknown"],
        json!(true),
        "嵌套未知字段也要保留"
    );
}

// ---------------------------------------------------------------------------
// 投影
// ---------------------------------------------------------------------------

fn discovered() -> Vec<Worktree> {
    let mut linked = worktree("/ws/proj/feat", "feat", false);
    linked.branch = Some("feat".to_owned());
    linked.branch_label = Some("feat".to_owned());

    vec![worktree("/ws/proj", "proj", true), linked]
}

#[test]
fn view_uses_project_name_for_main_and_basename_for_linked() {
    let project = Project::new(path("/ws/proj"), "proj".to_owned(), &discovered());

    let view = project_view(&project, &Discovery::Found(discovered()), &[]);

    assert_eq!(view.name, "proj");
    assert!(view.error.is_none());
    assert_eq!(view.worktrees[0].name, "proj", "主工作树用项目名");
    assert!(view.worktrees[0].is_main);
    assert_eq!(view.worktrees[1].name, "feat");
    assert_eq!(view.worktrees[1].branch_label.as_deref(), Some("feat"));
}

#[test]
fn view_local_name_overrides_default() {
    let project = Project::new(path("/ws/proj"), "proj".to_owned(), &discovered());

    let view = project_view(
        &project,
        &Discovery::Found(discovered()),
        &names(&[("/ws/proj/feat", "我的特性")]),
    );

    assert_eq!(view.worktrees[1].name, "我的特性");
    assert_eq!(view.worktrees[0].name, "proj", "未覆盖的仍用默认名");
}

#[test]
fn view_marks_sidebar_membership() {
    let mut project = Project::new(path("/ws/proj"), "proj".to_owned(), &discovered());
    project.set_pinned(Path::new("/ws/proj"), false);

    let view = project_view(&project, &Discovery::Found(discovered()), &[]);

    assert!(!view.worktrees[0].pinned);
    assert!(view.worktrees[1].pinned);
}

#[test]
fn view_carries_worktree_flags() {
    let mut bare_main = worktree("/ws/bare.git", "bare", true);
    bare_main.bare = true;

    let mut locked = worktree("/ws/proj/locked", "locked", false);
    locked.locked = Marker::Present {
        reason: Some("hold for review".to_owned()),
    };
    locked.detached = true;
    locked.branch_label = Some("bc8ce16".to_owned());

    let mut stale = worktree("/ws/proj/stale", "stale", false);
    stale.prunable = Marker::Present { reason: None };
    stale.stale = true;

    let all = vec![bare_main, locked, stale];
    let project = Project::new(path("/ws/proj"), "proj".to_owned(), &all);

    let view = project_view(&project, &Discovery::Found(all), &[]);

    assert!(view.worktrees[0].bare);
    assert!(view.worktrees[1].locked && view.worktrees[1].detached);
    assert_eq!(view.worktrees[1].branch_label.as_deref(), Some("bc8ce16"));
    assert!(view.worktrees[2].stale);
    assert!(
        view.worktrees[1].pinned,
        "加入项目时全部 pin，flags 不影响成员判定"
    );
}

#[test]
fn view_reports_discovery_failure_without_worktrees() {
    // §15：路径失效 / 非 git 目录 / git 缺失，都要有明确提示。
    let project = Project::new(path("/ws/proj"), "proj".to_owned(), &discovered());

    let view = project_view(
        &project,
        &Discovery::Failed {
            message: "不是 git 仓库：/ws/proj".to_owned(),
        },
        &[],
    );

    assert_eq!(view.name, "proj");
    assert!(view.worktrees.is_empty());
    assert_eq!(view.error.as_deref(), Some("不是 git 仓库：/ws/proj"));
    // 失败不影响落盘事实
    assert_eq!(view.path, path("/ws/proj"));
    assert!(!view.collapsed);
}

#[test]
fn collapsed_flag_reaches_the_view() {
    let mut project = Project::new(path("/ws/proj"), "proj".to_owned(), &discovered());
    project.collapsed = true;

    let view = project_view(&project, &Discovery::Found(discovered()), &[]);

    assert!(view.collapsed);
}

#[test]
fn empty_project_view_has_no_error() {
    let project = Project::new(path("/ws/proj"), "proj".to_owned(), &[]);

    let view = project_view(&project, &Discovery::Found(Vec::new()), &[]);

    assert!(view.worktrees.is_empty());
    assert!(view.error.is_none());
}

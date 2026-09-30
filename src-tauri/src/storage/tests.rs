//! 持久化内核的行为测试：断言落盘的文件内容、备份文件名，以及跨「重启」的读取。
//!
//! 多数用例走公开的 [`Store`]；迁移链与备份 GC 属于内核机制，由 `doc::load` /
//! `atomic::gc_backups` 直接驱动（REQUIREMENTS.md §20 的夹具约定）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::doc::{self, Document, Migration};
use super::{atomic, Notice, Store, WindowState};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// 用完即删的临时目录。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "ageminal-store-{}-{tag}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn backup_names(dir: &Path) -> Vec<String> {
    let backups = dir.join("backups");
    let mut names: Vec<String> = fs::read_dir(backups)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/storage")
        .join(name)
}

// ---------------------------------------------------------------------------
// 测试用文档：带一条 1 → 2 → 3 的迁移链
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Demo {
    #[serde(default = "demo_current")]
    schema_version: u32,
    #[serde(default)]
    step: u32,
    #[serde(default)]
    order: Vec<String>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

fn demo_current() -> u32 {
    3
}

impl Default for Demo {
    fn default() -> Self {
        Self {
            schema_version: 3,
            step: 0,
            order: Vec::new(),
            extra: Map::new(),
        }
    }
}

fn push_order(value: &mut Value, entry: &str) -> Result<(), String> {
    let object = value.as_object_mut().ok_or("根不是对象")?;
    let order = object
        .entry("order")
        .or_insert_with(|| Value::Array(Vec::new()));
    order
        .as_array_mut()
        .ok_or("order 不是数组")?
        .push(Value::from(entry));
    Ok(())
}

fn demo_v1_to_v2(value: &mut Value) -> Result<(), String> {
    value
        .as_object_mut()
        .ok_or("根不是对象")?
        .insert("step".into(), Value::from(2));
    push_order(value, "v1->v2")
}

fn demo_v2_to_v3(value: &mut Value) -> Result<(), String> {
    value
        .as_object_mut()
        .ok_or("根不是对象")?
        .insert("step".into(), Value::from(3));
    push_order(value, "v2->v3")
}

impl Document for Demo {
    const FILE: &'static str = "demo.json";
    const CURRENT_VERSION: u32 = 3;

    fn migrations() -> &'static [Migration] {
        static MIGRATIONS: [Migration; 2] = [
            Migration {
                from: 1,
                apply: demo_v1_to_v2,
            },
            Migration {
                from: 2,
                apply: demo_v2_to_v3,
            },
        ];
        &MIGRATIONS
    }

    fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

/// 迁移必然失败的文档。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FailDemo {
    #[serde(default = "fail_current")]
    schema_version: u32,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

fn fail_current() -> u32 {
    2
}

impl Default for FailDemo {
    fn default() -> Self {
        Self {
            schema_version: 2,
            extra: Map::new(),
        }
    }
}

fn fail_v1_to_v2(_value: &mut Value) -> Result<(), String> {
    Err("故意失败".to_owned())
}

impl Document for FailDemo {
    const FILE: &'static str = "fail.json";
    const CURRENT_VERSION: u32 = 2;

    fn migrations() -> &'static [Migration] {
        static MIGRATIONS: [Migration; 1] = [Migration {
            from: 1,
            apply: fail_v1_to_v2,
        }];
        &MIGRATIONS
    }

    fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

#[test]
fn settings_survive_restart() {
    let dir = TempDir::new("restart");

    {
        let mut store = Store::open(dir.path()).unwrap();
        store
            .update_settings(|settings| settings.general.language = Some("en-US".to_owned()))
            .unwrap();
    }

    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.settings().general.language.as_deref(), Some("en-US"));
    assert!(store.notices().is_empty());
}

#[test]
fn fresh_settings_have_no_language() {
    // 「未设置」必须是一个**可区分**的状态：语言检测（#54）靠它决定是否走系统 locale。
    let dir = TempDir::new("fresh-lang");
    let store = Store::open(dir.path()).unwrap();

    assert_eq!(store.settings().general.language, None);
}

#[test]
fn settings_are_written_immediately() {
    let dir = TempDir::new("immediate");
    let mut store = Store::open(dir.path()).unwrap();

    store
        .update_settings(|settings| settings.general.confirm_close = false)
        .unwrap();

    let raw = read_json(&dir.path().join("settings.json"));
    assert_eq!(raw["general"]["confirmClose"], false);
}

#[test]
fn state_flush_is_debounced() {
    let dir = TempDir::new("debounce");
    let mut store = Store::open(dir.path()).unwrap();
    let start = Instant::now();

    store
        .update_state(
            |state| {
                state.window = Some(WindowState {
                    width: 1000.0,
                    height: 700.0,
                    x: Some(10.0),
                    y: Some(20.0),
                    ..Default::default()
                })
            },
            start,
        )
        .unwrap();

    assert!(read_json(&dir.path().join("state.json"))["window"].is_null());
    assert!(!store
        .flush_state_if_due(start + Duration::from_millis(100))
        .unwrap());
    assert!(store
        .flush_state_if_due(start + Duration::from_millis(600))
        .unwrap());

    let raw = read_json(&dir.path().join("state.json"));
    assert_eq!(raw["window"]["width"], 1000.0);
    assert_eq!(raw["window"]["maximized"], false);
}

#[test]
fn flush_writes_pending_state_before_exit() {
    let dir = TempDir::new("exit-flush");
    let mut store = Store::open(dir.path()).unwrap();

    store
        .update_state(
            |state| {
                state.window = Some(WindowState {
                    width: 800.0,
                    height: 600.0,
                    maximized: true,
                    ..Default::default()
                })
            },
            Instant::now(),
        )
        .unwrap();
    store.flush().unwrap();

    let raw = read_json(&dir.path().join("state.json"));
    assert_eq!(raw["window"]["maximized"], true);
}

#[test]
fn corrupt_file_is_backed_up_and_rebuilt() {
    let dir = TempDir::new("corrupt");
    fs::write(dir.path().join("settings.json"), "{ 这不是 JSON").unwrap();

    let store = Store::open(dir.path()).unwrap();

    assert!(matches!(
        store.notices(),
        [Notice::CorruptRecovered { backup }] if backup.path.exists()
    ));
    assert_eq!(store.settings().general.language, None);
    assert!(backup_names(dir.path())
        .iter()
        .any(|name| name.starts_with("settings.json.corrupt-")));
    // 重建后的文件是合法文档
    assert_eq!(
        read_json(&dir.path().join("settings.json"))["schemaVersion"],
        1
    );
}

#[test]
fn migration_chain_runs_in_order_and_is_idempotent() {
    let dir = TempDir::new("migrate");
    fs::copy(fixture("demo-v1.json"), dir.path().join("demo.json")).unwrap();

    let (first, notices) = doc::load::<Demo>(dir.path()).unwrap();
    assert!(notices.is_empty());
    assert_eq!(first.step, 3);
    assert_eq!(first.order, vec!["v1->v2".to_owned(), "v2->v3".to_owned()]);
    assert_eq!(first.schema_version, 3);
    // 未知字段原样保留
    assert_eq!(first.extra.get("unknownKey"), Some(&Value::from("keep-me")));
    // 迁移前的安全备份
    assert!(backup_names(dir.path())
        .iter()
        .any(|name| name.starts_with("demo.json.migrate-")));

    // 再加载一次：链不再执行，结果一致
    let (second, notices) = doc::load::<Demo>(dir.path()).unwrap();
    assert_eq!(first, second);
    assert!(notices.is_empty());
    assert_eq!(
        backup_names(dir.path())
            .iter()
            .filter(|name| name.starts_with("demo.json.migrate-"))
            .count(),
        1
    );
}

#[test]
fn successful_migration_also_collects_backups() {
    let dir = TempDir::new("migrate-gc");
    let backups = dir.path().join("backups");
    fs::create_dir_all(&backups).unwrap();
    for index in 0..(atomic::MAX_BACKUPS + 5) {
        fs::write(backups.join(format!("demo.json.corrupt-{index}")), b"x").unwrap();
    }
    fs::copy(fixture("demo-v1.json"), dir.path().join("demo.json")).unwrap();

    doc::load::<Demo>(dir.path()).unwrap();

    assert!(
        fs::read_dir(&backups).unwrap().count() <= atomic::MAX_BACKUPS,
        "迁移成功这条路径也要收口备份数量"
    );
}

#[test]
fn failed_migration_is_backed_up_and_rebuilt() {
    let dir = TempDir::new("migrate-fail");
    fs::write(dir.path().join("fail.json"), r#"{"schemaVersion":1}"#).unwrap();

    let (document, notices) = doc::load::<FailDemo>(dir.path()).unwrap();

    assert_eq!(document, FailDemo::default());
    assert!(matches!(
        notices.as_slice(),
        [Notice::MigrationFailed { backup, reason }]
            if backup.path.exists() && reason.contains("故意失败")
    ));
    assert!(backup_names(dir.path())
        .iter()
        .any(|name| name.starts_with("fail.json.migrate-failed-")));
    // 已重建为当前版本的默认文档
    assert_eq!(read_json(&dir.path().join("fail.json"))["schemaVersion"], 2);
}

#[test]
fn newer_schema_version_is_writable_and_keeps_unknown_fields() {
    let dir = TempDir::new("forward");
    let path = dir.path().join("settings.json");
    fs::write(
        &path,
        r#"{"schemaVersion":9,"general":{"language":"en-US","unknownInner":true},"futureKey":{"nested":1}}"#,
    )
    .unwrap();

    let mut store = Store::open(dir.path()).unwrap();
    assert_eq!(store.settings().schema_version, 9);
    assert!(matches!(
        store.notices(),
        [Notice::WrittenByNewerVersion {
            found: 9,
            current: 1,
            ..
        }]
    ));

    store
        .update_settings(|settings| settings.general.language = Some("ja-JP".to_owned()))
        .unwrap();

    let raw = read_json(&path);
    assert_eq!(raw["schemaVersion"], 9, "不得降级高版本写入的文档");
    assert_eq!(raw["general"]["language"], "ja-JP");
    assert_eq!(
        raw["general"]["unknownInner"], true,
        "嵌套未知字段也必须保留"
    );
    assert_eq!(raw["futureKey"]["nested"], 1, "未知字段必须原样保留");
}

#[test]
fn backups_are_bounded() {
    let dir = TempDir::new("gc");
    let backups = dir.path().join("backups");
    fs::create_dir_all(&backups).unwrap();
    for index in 0..(atomic::MAX_BACKUPS + 5) {
        fs::write(backups.join(format!("settings.json.corrupt-{index}")), b"x").unwrap();
    }

    atomic::gc_backups(&backups).unwrap();

    assert_eq!(fs::read_dir(&backups).unwrap().count(), atomic::MAX_BACKUPS);
}

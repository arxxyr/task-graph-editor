//! 主题偏好：启动时恢复，实际切换后以同目录临时文件原子保存。

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use bevy::prelude::*;
use serde_json::{Map, Value};

use super::UiSet;
use super::theme::{ThemeId, ThemeSelection};

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// 可注入的主题存储；构造时只确定路径，不读取配置文件。
#[derive(Resource)]
pub struct ThemePreferencesStore {
    path: Option<PathBuf>,
    /// 保留未知键；读取失败或顶层不是对象时保持 None，禁止覆盖原文件。
    document: Option<Map<String, Value>>,
    /// 记录已经观察过的选择，包括保存失败的选择，避免每帧重复尝试。
    observed: Option<ThemeId>,
}

impl ThemePreferencesStore {
    /// 完全关闭配置读写，供截图和隔离测试注入。
    pub fn disabled() -> Self {
        Self {
            path: None,
            document: None,
            observed: None,
        }
    }

    /// 显式指定配置路径；文件读取推迟到 PreStartup。
    pub fn at_path(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            ..Self::disabled()
        }
    }

    fn load(&mut self) -> Option<ThemeId> {
        let path = self.path.as_ref()?;
        let content = match fs::read(path) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.document = Some(Map::new());
                return None;
            }
            Err(error) => {
                warn!(path = %path.display(), %error, "读取主题偏好失败，本次运行停止写入");
                return None;
            }
        };
        let document = match serde_json::from_slice::<Map<String, Value>>(&content) {
            Ok(document) => document,
            Err(error) => {
                warn!(path = %path.display(), %error, "主题偏好不是有效 JSON 对象，保留原文件");
                return None;
            }
        };
        let theme = match document.get("theme") {
            Some(value) => match serde_json::from_value::<ThemeId>(value.clone()) {
                Ok(theme) => theme,
                Err(_) => {
                    warn!(path = %path.display(), "主题偏好包含未知主题，使用默认主题并保留原文件");
                    ThemeId::default()
                }
            },
            None => ThemeId::default(),
        };
        self.document = Some(document);
        Some(theme)
    }

    fn save(&mut self, theme: ThemeId) -> io::Result<()> {
        let (Some(path), Some(document)) = (&self.path, &self.document) else {
            return Ok(());
        };
        let mut next = document.clone();
        next.insert(
            "theme".into(),
            serde_json::to_value(theme).map_err(io::Error::other)?,
        );
        let mut content = serde_json::to_vec_pretty(&next).map_err(io::Error::other)?;
        content.push(b'\n');
        atomic_write(path, &content)?;
        self.document = Some(next);
        Ok(())
    }
}

impl Default for ThemePreferencesStore {
    fn default() -> Self {
        match crate::ssh_config::home_dir().filter(|home| home.is_absolute()) {
            Some(home) => Self::at_path(
                home.join(".config")
                    .join("task-graph-editor")
                    .join("theme.json"),
            ),
            // 没有可靠的主目录时不得退回当前工作目录。
            None => Self::disabled(),
        }
    }
}

pub struct ThemePreferencesPlugin;

impl Plugin for ThemePreferencesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ThemePreferencesStore>()
            .add_systems(PreStartup, load_preferences)
            .add_systems(Update, save_preferences.after(UiSet::Rebuild));
    }
}

fn load_preferences(
    mut store: ResMut<ThemePreferencesStore>,
    mut selection: ResMut<ThemeSelection>,
) {
    if let Some(theme) = store.load() {
        selection.set_if_neq(ThemeSelection(theme));
    }
    // 初始值与启动加载都只建立基线，不产生磁盘写入。
    store.observed = Some(selection.0);
}

fn save_preferences(selection: Res<ThemeSelection>, mut store: ResMut<ThemePreferencesStore>) {
    match store.observed {
        Some(previous) if previous == selection.0 => return,
        None => {
            store.observed = Some(selection.0);
            return;
        }
        Some(_) => {}
    }
    store.observed = Some(selection.0);
    if let Err(error) = store.save(selection.0)
        && let Some(path) = &store.path
    {
        warn!(path = %path.display(), %error, "保存主题偏好失败，保留旧文件；再次切换主题时重试");
    }
}

/// 只负责自己独占创建的临时文件，任何失败路径都先关闭再删除。
struct TemporaryFile {
    path: PathBuf,
    file: Option<File>,
    published: bool,
}

impl TemporaryFile {
    fn create(parent: &Path, filename: &OsStr) -> io::Result<Self> {
        for _ in 0..128 {
            let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
            let mut name = std::ffi::OsString::from(".");
            name.push(filename);
            name.push(format!(".{}.{sequence}.tmp", std::process::id()));
            let path = parent.join(name);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                        published: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "无法独占创建主题偏好临时文件",
        ))
    }

    fn write_and_publish(mut self, target: &Path, content: &[u8]) -> io::Result<()> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("主题偏好临时文件已经关闭"))?;
        file.write_all(content)?;
        file.sync_all()?;
        // Windows 上文件仍被打开时可能无法重命名，提交前显式关闭。
        drop(self.file.take());
        fs::rename(&self.path, target)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn atomic_write(path: &Path, content: &[u8]) -> io::Result<()> {
    let filename = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "主题偏好路径缺少文件名"))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    TemporaryFile::create(parent, filename)?.write_and_publish(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let root = std::env::temp_dir();
            fs::create_dir_all(&root).unwrap();
            for _ in 0..128 {
                let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
                let path = root.join(format!(
                    "tge-theme-preferences-{}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("创建隔离测试目录失败：{error}"),
                }
            }
            panic!("无法独占创建隔离测试目录");
        }

        fn path(&self) -> PathBuf {
            self.0.join("theme.json")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn app(store: ThemePreferencesStore, theme: ThemeId) -> App {
        let mut app = App::new();
        app.insert_resource(store)
            .insert_resource(ThemeSelection(theme))
            .add_plugins(ThemePreferencesPlugin);
        app
    }

    fn choose(app: &mut App, theme: ThemeId) {
        app.world_mut()
            .resource_mut::<ThemeSelection>()
            .set_if_neq(ThemeSelection(theme));
        app.update();
    }

    fn read(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn 缺失配置启动不创建目录且相同选择不写入() {
        let directory = TestDirectory::new();
        let path = directory.0.join("not-created/theme.json");
        let mut app = app(
            ThemePreferencesStore::at_path(path.clone()),
            ThemeId::default(),
        );
        app.update();
        app.world_mut()
            .resource_mut::<ThemeSelection>()
            .set_changed();
        app.update();
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn 禁用存储保持注入主题且不建立写入文档() {
        let mut app = app(ThemePreferencesStore::disabled(), ThemeId::WarmPaper);
        app.update();
        assert_eq!(
            app.world().resource::<ThemeSelection>().0,
            ThemeId::WarmPaper
        );
        choose(&mut app, ThemeId::DuskSand);
        let store = app.world().resource::<ThemePreferencesStore>();
        assert!(store.path.is_none());
        assert!(store.document.is_none());
        assert_eq!(store.observed, Some(ThemeId::DuskSand));
    }

    #[test]
    fn 只在启动阶段读取且重启恢复上次选择() {
        let directory = TestDirectory::new();
        let path = directory.path();
        let store = ThemePreferencesStore::at_path(path.clone());
        let mut first = app(store, ThemeId::default());
        // 插件已注册后再创建配置，证明构造资源与插件注册不会提前读取。
        fs::write(&path, b"{\"theme\":\"mist-indigo\"}").unwrap();
        first.update();
        assert_eq!(
            first.world().resource::<ThemeSelection>().0,
            ThemeId::MistIndigo
        );
        choose(&mut first, ThemeId::DuskSand);
        drop(first);
        let saved = fs::read(&path).unwrap();
        let mut second = app(
            ThemePreferencesStore::at_path(path.clone()),
            ThemeId::default(),
        );
        second.update();
        assert_eq!(
            second.world().resource::<ThemeSelection>().0,
            ThemeId::DuskSand
        );
        assert_eq!(fs::read(&path).unwrap(), saved);
    }

    #[test]
    fn 保存主题保留未知字段和嵌套对象() {
        let directory = TestDirectory::new();
        let path = directory.path();
        let original = b"{\"theme\":\"mist-indigo\",\"revision\":7,\"future\":{\"enabled\":true}}";
        fs::write(&path, original).unwrap();
        let mut app = app(
            ThemePreferencesStore::at_path(path.clone()),
            ThemeId::default(),
        );
        app.update();
        assert_eq!(fs::read(&path).unwrap(), original);
        choose(&mut app, ThemeId::WarmPaper);
        assert_eq!(
            read(&path),
            serde_json::json!({
                "theme": "warm-paper",
                "revision": 7,
                "future": { "enabled": true }
            })
        );
    }

    #[test]
    fn 未知主题回退默认但启动不覆写原值() {
        let directory = TestDirectory::new();
        let path = directory.path();
        let original = b"{\"theme\":\"future-theme\",\"future\":true}";
        fs::write(&path, original).unwrap();
        let mut app = app(
            ThemePreferencesStore::at_path(path.clone()),
            ThemeId::WarmPaper,
        );
        app.update();
        assert_eq!(
            app.world().resource::<ThemeSelection>().0,
            ThemeId::default()
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        choose(&mut app, ThemeId::DuskSand);
        assert_eq!(
            read(&path),
            serde_json::json!({"theme": "dusk-sand", "future": true})
        );
    }

    #[test]
    fn 损坏或非对象配置不会被后续主题选择覆盖() {
        for content in [b"{".as_slice(), b"[]".as_slice(), b"null".as_slice()] {
            let directory = TestDirectory::new();
            let path = directory.path();
            fs::write(&path, content).unwrap();
            let mut app = app(
                ThemePreferencesStore::at_path(path.clone()),
                ThemeId::default(),
            );
            app.update();
            choose(&mut app, ThemeId::DuskSand);
            choose(&mut app, ThemeId::WarmPaper);
            assert_eq!(fs::read(&path).unwrap(), content);
            assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
        }
    }

    #[test]
    fn 保存失败不逐帧重试且下一次切换可恢复() {
        let directory = TestDirectory::new();
        let parent = directory.0.join("config");
        let backup = directory.0.join("saved-config");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("theme.json");
        let original = b"{\"theme\":\"graphite-teal\",\"future\":true}";
        fs::write(&path, original).unwrap();
        let mut app = app(
            ThemePreferencesStore::at_path(path.clone()),
            ThemeId::default(),
        );
        app.update();
        // 用普通文件阻断父目录，所有平台都能确定性触发创建目录失败。
        fs::rename(&parent, &backup).unwrap();
        fs::write(&parent, b"blocked").unwrap();
        choose(&mut app, ThemeId::DuskSand);
        assert_eq!(fs::read(backup.join("theme.json")).unwrap(), original);
        fs::remove_file(&parent).unwrap();
        fs::rename(&backup, &parent).unwrap();
        app.world_mut()
            .resource_mut::<ThemeSelection>()
            .set_changed();
        app.update();
        app.update();
        assert_eq!(fs::read(&path).unwrap(), original);
        choose(&mut app, ThemeId::WarmPaper);
        assert_eq!(
            read(&path),
            serde_json::json!({"theme": "warm-paper", "future": true})
        );
    }

    #[test]
    fn 提交失败保留目标并清理自己的临时文件() {
        let directory = TestDirectory::new();
        let target = directory.path();
        fs::create_dir(&target).unwrap();
        fs::write(target.join("sentinel"), b"untouched").unwrap();
        assert!(atomic_write(&target, b"{}\n").is_err());
        assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"untouched");
        assert_eq!(fs::read_dir(&directory.0).unwrap().count(), 1);
    }

    #[test]
    fn 重建阶段切换主题会在同一帧保存() {
        let directory = TestDirectory::new();
        let path = directory.path();
        let mut app = app(
            ThemePreferencesStore::at_path(path.clone()),
            ThemeId::default(),
        );
        app.update();
        app.add_systems(
            Update,
            (|mut selection: ResMut<ThemeSelection>| {
                selection.set_if_neq(ThemeSelection(ThemeId::WarmPaper));
            })
            .in_set(UiSet::Rebuild),
        );
        app.update();
        assert_eq!(read(&path), serde_json::json!({"theme": "warm-paper"}));
    }
}

//! 文件列表：远程 JSON 文件的浏览、选中与右键操作
//!
//! 列表在文件集合变化时整体重建；选中态单独用 `Selected` 组件同步，
//! 不触发重建。右键菜单是常驻的绝对定位浮层（Feathers 的菜单只支持左键触发），
//! 由 [`ContextMenu`] 资源驱动内容与位置。

use bevy::feathers::controls::FeathersListRow;
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeTextColor, ThemedText};
use bevy::feathers::tokens;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::{Selected, UiScale};

use super::shell::{ContextMenuRoot, FileListSlot};
use super::theme;
use super::widgets::{self, BoxedScene, boxed};
use super::worker_bridge::AppAction;
use super::{FileBrowser, Session, UiSet};

/// 列表中的一行，记录对应的文件名
#[derive(Component, Clone, Default)]
pub struct FileRow(pub String);

/// 列表底部的空白区（右键可上传）
#[derive(Component, Clone, Default)]
struct FileListBlank;

/// 列表容器插槽
#[derive(Component, Clone, Default)]
struct FileRowsSlot;

/// 卡片标题文字（计数随文件数更新）
#[derive(Component, Clone, Default)]
struct FileListTitle;

/// 右键菜单状态
#[derive(Resource, Default)]
pub struct ContextMenu {
    /// 是否打开
    pub open: bool,
    /// 打开位置（逻辑像素）
    pub position: Vec2,
    /// 右键的目标文件；`None` 表示点在空白处
    pub target: Option<String>,
    /// 版本号：每次打开递增，驱动菜单项重建
    pub version: u64,
}

impl ContextMenu {
    /// 在指定位置为某个目标打开菜单
    fn open_at(&mut self, position: Vec2, target: Option<String>) {
        self.open = true;
        self.position = position;
        self.target = target;
        self.version += 1;
    }

    /// 关闭菜单
    fn close(&mut self) {
        if self.open {
            self.open = false;
            self.version += 1;
        }
    }
}

/// 已渲染的列表版本，避免重复重建
#[derive(Resource, Default)]
struct RenderedList {
    /// 已渲染的文件列表版本
    files: u64,
    /// 已渲染的右键菜单版本
    menu: u64,
}

/// 构建文件列表卡片
fn file_list_card(browser: &FileBrowser) -> impl Scene {
    let title = format!("文件列表 ({})", browser.files.len());
    widgets::card_titled(
        title,
        FileListTitle,
        bsn_list![(
            Node {
                width: percent(100),
                flex_direction: FlexDirection::Column,
                row_gap: px(1),
                min_height: px(40),
            }
            FileRowsSlot
        )],
    )
}

/// 单个文件行
fn file_row(filename: &str, selected: bool) -> impl Scene {
    let name = filename.to_string();
    let row_marker = FileRow(name.clone());
    // Selected 是标记组件，只在选中时插入
    let selected_marker = selected.then(|| template_value(Selected));
    bsn! {
        @FeathersListRow
        Node {
            width: percent(100),
            border_radius: {BorderRadius::all(px(theme::RADIUS_SM))},
        }
        template_value(row_marker)
        {selected_marker}
        Children [(
            Text(name)
            ThemedText
            TextFont { font_size: px(12.0) }
        )]
    }
}

/// 列表底部空白区：右键可上传
fn file_list_blank() -> impl Scene {
    bsn! {
        Node {
            width: percent(100),
            min_height: px(24),
        }
        FileListBlank
    }
}

/// 首次构建文件列表卡片
fn spawn_file_list(
    browser: Res<FileBrowser>,
    slots: Query<Entity, Added<FileListSlot>>,
    mut commands: Commands,
) {
    for slot in &slots {
        commands
            .entity(slot)
            .queue_spawn_related_scenes::<Children>(bsn_list![file_list_card(&browser)]);
    }
}

/// 文件集合变化时重建行
fn rebuild_file_rows(
    browser: Res<FileBrowser>,
    mut rendered: ResMut<RenderedList>,
    slots: Query<Entity, With<FileRowsSlot>>,
    mut commands: Commands,
) {
    let Ok(slot) = slots.single() else {
        return;
    };
    // 版本号相同则无需重建；version 为 0 时也要建一次（初始空列表的提示行）
    if rendered.files == browser.list_version && rendered.files != 0 {
        return;
    }
    rendered.files = browser.list_version.max(1);

    let mut rows: Vec<BoxedScene> = match browser.files.is_empty() {
        true => vec![boxed(bsn! {
            Node { padding: {UiRect::all(px(4.0))} }
            Children [(widgets::hint("未连接，或远程目录下无 JSON 文件"))]
        })],
        false => browser
            .files
            .iter()
            .map(|name| {
                let selected = browser.selected.as_deref() == Some(name.as_str());
                boxed(file_row(name, selected))
            })
            .collect(),
    };
    rows.push(boxed(file_list_blank()));

    commands
        .entity(slot)
        .despawn_related::<Children>()
        .queue_spawn_related_scenes::<Children>(rows);
}

/// 卡片标题里的计数跟随文件数
fn sync_list_title(browser: Res<FileBrowser>, mut titles: Query<&mut Text, With<FileListTitle>>) {
    if !browser.is_changed() {
        return;
    }
    for mut text in &mut titles {
        text.0 = format!("文件列表 ({})", browser.files.len());
    }
}

/// 选中态同步：只加减 `Selected` 组件，不重建列表
fn sync_row_selection(
    browser: Res<FileBrowser>,
    rows: Query<(Entity, &FileRow, Has<Selected>)>,
    mut commands: Commands,
) {
    if !browser.is_changed() {
        return;
    }
    for (entity, row, has_selected) in &rows {
        let should_select = browser.selected.as_deref() == Some(row.0.as_str());
        match (should_select, has_selected) {
            (true, false) => {
                commands.entity(entity).insert(Selected);
            }
            (false, true) => {
                commands.entity(entity).remove::<Selected>();
            }
            _ => {}
        }
    }
}

/// 文件行按下：左键加载，右键弹菜单
fn on_row_press(
    mut press: On<Pointer<Press>>,
    rows: Query<&FileRow>,
    session: Res<Session>,
    ui_scale: Res<UiScale>,
    mut menu: ResMut<ContextMenu>,
    mut writer: MessageWriter<AppAction>,
) {
    let Ok(row) = rows.get(press.entity) else {
        return;
    };
    press.propagate(false);

    match press.button {
        PointerButton::Secondary => {
            menu.open_at(
                press.pointer_location.position / ui_scale.0,
                Some(row.0.clone()),
            );
        }
        PointerButton::Primary => {
            menu.close();
            // 忙碌时忽略点击，避免打断进行中的请求
            if session.interactive() {
                writer.write(AppAction::LoadFile(row.0.clone()));
            }
        }
        PointerButton::Middle => {}
    }
}

/// 空白区右键：只提供上传
fn on_blank_press(
    mut press: On<Pointer<Press>>,
    blanks: Query<(), With<FileListBlank>>,
    ui_scale: Res<UiScale>,
    mut menu: ResMut<ContextMenu>,
) {
    if blanks.get(press.entity).is_err() {
        return;
    }
    press.propagate(false);
    match press.button {
        PointerButton::Secondary => {
            menu.open_at(press.pointer_location.position / ui_scale.0, None);
        }
        _ => menu.close(),
    }
}

/// 点在菜单以外的任何地方都关闭菜单
fn on_press_anywhere(
    press: On<Pointer<Press>>,
    menu_roots: Query<Entity, With<ContextMenuRoot>>,
    parents: Query<&ChildOf>,
    mut menu: ResMut<ContextMenu>,
) {
    if !menu.open {
        return;
    }
    let Ok(root) = menu_roots.single() else {
        return;
    };
    // 点在菜单自身或其子孙上时不关闭，交给菜单项的处理
    let in_menu = press.entity == root
        || parents
            .iter_ancestors(press.entity)
            .any(|ancestor| ancestor == root);
    if !in_menu {
        menu.close();
    }
}

/// 菜单项：点击后发出操作并关闭菜单
#[derive(Component, Clone)]
struct MenuAction(AppAction);

impl Default for MenuAction {
    fn default() -> Self {
        Self(AppAction::UploadFile)
    }
}

/// 一个菜单条目
fn menu_item(label: &str, danger: bool, enabled: bool, action: AppAction) -> impl Scene {
    let text_token = match (enabled, danger) {
        (false, _) => tokens::MENUITEM_TEXT_DISABLED,
        (true, true) => theme::DANGER_BG,
        (true, false) => tokens::MENUITEM_TEXT,
    };
    bsn! {
        Node {
            width: percent(100),
            padding: {UiRect::axes(px(10.0), px(5.0))},
            border_radius: {BorderRadius::all(px(4.0))},
            align_items: AlignItems::Center,
        }
        Button
        template_value(MenuAction(action))
        ThemeBackgroundColor({tokens::MENU_BG})
        Children [(
            Text({label.to_string()})
            ThemeTextColor({text_token})
            TextFont { font_size: px(12.0) }
        )]
    }
}

/// 菜单分隔线
fn menu_divider() -> impl Scene {
    bsn! {
        Node {
            width: percent(100),
            height: px(1),
            margin: {UiRect::vertical(px(3.0))},
        }
        ThemeBackgroundColor({theme::DIVIDER})
    }
}

/// 按当前状态重建右键菜单内容并定位
fn rebuild_context_menu(
    menu: Res<ContextMenu>,
    session: Res<Session>,
    mut rendered: ResMut<RenderedList>,
    mut roots: Query<(Entity, &mut Node), With<ContextMenuRoot>>,
    mut commands: Commands,
) {
    if rendered.menu == menu.version {
        return;
    }
    let Ok((root, mut node)) = roots.single_mut() else {
        return;
    };
    rendered.menu = menu.version;

    if !menu.open {
        node.display = Display::None;
        commands.entity(root).despawn_related::<Children>();
        return;
    }

    node.display = Display::Flex;
    node.left = px(menu.position.x);
    node.top = px(menu.position.y);

    let idle = session.interactive();
    let items: Vec<BoxedScene> = match &menu.target {
        Some(filename) => vec![
            boxed(menu_item("上传文件", false, idle, AppAction::UploadFile)),
            boxed(menu_item(
                "备份",
                false,
                idle,
                AppAction::BackupFile(filename.clone()),
            )),
            boxed(menu_divider()),
            boxed(menu_item(
                "删除",
                true,
                idle,
                AppAction::DeleteFile(filename.clone()),
            )),
        ],
        None => vec![boxed(menu_item(
            "上传文件",
            false,
            idle,
            AppAction::UploadFile,
        ))],
    };

    commands
        .entity(root)
        .despawn_related::<Children>()
        .queue_spawn_related_scenes::<Children>(items);
}

/// 菜单项点击
fn on_menu_item_click(
    mut click: On<Pointer<Click>>,
    items: Query<&MenuAction>,
    session: Res<Session>,
    mut menu: ResMut<ContextMenu>,
    mut writer: MessageWriter<AppAction>,
) {
    let Ok(item) = items.get(click.entity) else {
        return;
    };
    click.propagate(false);
    menu.close();
    // 忙碌时不下发，行为与旧版的禁用态一致
    if session.interactive() {
        writer.write(item.0.clone());
    }
}

/// 菜单项悬停高亮
fn on_menu_item_hover(
    over: On<Pointer<Over>>,
    items: Query<(), With<MenuAction>>,
    mut commands: Commands,
) {
    if items.get(over.entity).is_ok() {
        commands
            .entity(over.entity)
            .insert(ThemeBackgroundColor(tokens::MENUITEM_BG_HOVER));
    }
}

/// 菜单项移出恢复
fn on_menu_item_out(
    out: On<Pointer<Out>>,
    items: Query<(), With<MenuAction>>,
    mut commands: Commands,
) {
    if items.get(out.entity).is_ok() {
        commands
            .entity(out.entity)
            .insert(ThemeBackgroundColor(tokens::MENU_BG));
    }
}

/// 文件列表插件
pub struct FileListPlugin;

impl Plugin for FileListPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ContextMenu>()
            .init_resource::<RenderedList>()
            .add_observer(on_row_press)
            .add_observer(on_blank_press)
            .add_observer(on_press_anywhere)
            .add_observer(on_menu_item_click)
            .add_observer(on_menu_item_hover)
            .add_observer(on_menu_item_out)
            .add_systems(
                Update,
                (
                    spawn_file_list,
                    rebuild_file_rows,
                    sync_list_title,
                    sync_row_selection,
                    rebuild_context_menu,
                )
                    .in_set(UiSet::Rebuild),
            );
    }
}

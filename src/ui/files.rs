//! 文件列表：远程 JSON 文件的浏览、选中与右键操作
//!
//! 列表在文件集合变化时整体重建；选中态单独用 `Selected` 组件同步，
//! 不触发重建。右键菜单是常驻的绝对定位浮层（Feathers 的菜单只支持左键触发），
//! 由 [`ContextMenu`] 资源驱动内容与位置。

use bevy::feathers::controls::FeathersListRow;
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeTextColor, ThemedText};
use bevy::feathers::tokens;
use bevy::picking::hover::Hovered;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{Interaction, Selected, UiScale};
use bevy::window::PrimaryWindow;

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

/// 显示当前列表的真实目录，避免把仍在编辑的连接表单目录误当成文件来源。
#[derive(Component, Clone, Default)]
struct FileListDirectory;

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
///
/// 用 `Option` 而不是 `0` 表示"还没建过"：版本号本身从 0 开始，
/// 若拿 0 当哨兵值，第一次真实更新（版本 0 → 1）会被误判成已渲染而丢掉。
#[derive(Resource, Default)]
struct RenderedList {
    /// 已渲染的文件列表版本
    files: Option<u64>,
    /// 已渲染的右键菜单版本
    menu: Option<u64>,
}

/// 构建文件列表卡片
fn file_list_card(browser: &FileBrowser) -> impl Scene {
    let title = format!("文件列表 ({})", browser.files.len());
    let directory = directory_label(browser);
    widgets::card_titled(
        title,
        FileListTitle,
        bsn_list![(
            Text(directory)
            FileListDirectory
            ThemeTextColor({tokens::TEXT_DIM})
            TextFont { font_size: px(11.0) }
            TextLayout { linebreak: {LineBreak::AnyCharacter} }
            Node { width: percent(100), margin: {UiRect::bottom(px(5.0))} }
        ), (
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

fn directory_label(browser: &FileBrowser) -> String {
    match &browser.remote_dir {
        Some(directory) => format!("当前目录：{directory}"),
        None => "当前目录：尚未加载".into(),
    }
}

fn sync_file_directory(
    browser: Res<FileBrowser>,
    mut labels: Query<(&mut Text, Ref<FileListDirectory>)>,
) {
    for (mut text, marker) in &mut labels {
        if browser.is_changed() || marker.is_added() {
            text.0 = directory_label(&browser);
        }
    }
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
        Hovered
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
    pending: Query<(), With<widgets::SlotPending>>,
    mut commands: Commands,
) {
    let Ok(slot) = slots.single() else {
        return;
    };
    if rendered.files == Some(browser.list_version) {
        return;
    }
    // 上一批还没落地就等着，不推进版本号，下一帧自动重试
    if pending.contains(slot) {
        return;
    }
    debug!(
        was = ?rendered.files,
        now = browser.list_version,
        count = browser.files.len(),
        "重建文件列表行"
    );
    rendered.files = Some(browser.list_version);

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

    widgets::replace_slot_children(&mut commands, slot, rows);
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

/// 文件行左键：加载该文件
fn on_row_press(
    mut press: On<Pointer<Press>>,
    rows: Query<&FileRow>,
    parents: Query<&ChildOf>,
    session: Res<Session>,
    mut writer: MessageWriter<AppAction>,
) {
    // 点中的可能是行内的文字节点，往上找到挂了 FileRow 的那层
    let Some(entity) = widgets::self_or_ancestor(press.entity, &parents, |e| rows.contains(e))
    else {
        return;
    };
    if press.button != PointerButton::Primary {
        return;
    }
    let Ok(row) = rows.get(entity) else {
        return;
    };
    press.propagate(false);
    // 忙碌时忽略点击，避免打断进行中的请求
    if session.interactive() {
        writer.write(AppAction::LoadFile(row.0.clone()));
    }
}

/// 右键打开菜单
///
/// 不走 picking 的 `Pointer<Press>`：那条路命中的是最内层的文字节点，
/// 还得依赖事件冒泡。这里直接读鼠标输入，再从悬停状态找目标——
/// `Hovered` 只挂在行本身，不会跑到子节点上去。
fn open_context_menu(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    rows: Query<(&FileRow, &Hovered)>,
    blanks: Query<&Hovered, With<FileListBlank>>,
    ui_scale: Res<UiScale>,
    mut menu: ResMut<ContextMenu>,
) {
    if !mouse.just_pressed(MouseButton::Right) {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let position = cursor / ui_scale.0;

    // 先看是否落在某个文件行上，其次看列表空白区
    if let Some((row, _)) = rows.iter().find(|(_, hovered)| hovered.get()) {
        menu.open_at(position, Some(row.0.clone()));
    } else if blanks.iter().any(Hovered::get) {
        menu.open_at(position, None);
    } else {
        menu.close();
    }
}

/// 点在菜单以外的地方就关闭
///
/// 只认左键：右键由 [`open_context_menu`] 负责，那里会直接覆盖或关闭。
fn close_menu_on_outside_click(
    mouse: Res<ButtonInput<MouseButton>>,
    items: Query<&Interaction, With<MenuAction>>,
    mut menu: ResMut<ContextMenu>,
) {
    if !menu.open || !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    // 有菜单项正被悬停或按下，说明点的是菜单内部，交给菜单项自己处理
    if items.iter().any(|state| *state != Interaction::None) {
        return;
    }
    menu.close();
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
    pending: Query<(), With<widgets::SlotPending>>,
    mut commands: Commands,
) {
    if rendered.menu == Some(menu.version) {
        return;
    }
    let Ok((root, mut node)) = roots.single_mut() else {
        return;
    };
    // 关闭和切换目标都要立即隐藏旧菜单，等待在飞场景落地后再替换。
    // 不能提前推进版本，否则待生成的旧菜单会漏掉下一帧的清理。
    if pending.contains(root) {
        node.display = Display::None;
        return;
    }
    rendered.menu = Some(menu.version);

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

    widgets::replace_slot_children(&mut commands, root, items);
}

/// 菜单项被按下：发出操作并关闭菜单
///
/// 用 `Interaction` 而不是 `Pointer<Click>`：后者命中的是菜单项里的文字节点，
/// 标记组件挂在外层，查不到就成了"点了没反应"。`Interaction` 只在挂了它的
/// 节点（这里是带 `Button` 的菜单项）上更新，没有这个问题。
fn handle_menu_item_press(
    items: Query<(&Interaction, &MenuAction), Changed<Interaction>>,
    session: Res<Session>,
    mut menu: ResMut<ContextMenu>,
    mut writer: MessageWriter<AppAction>,
) {
    for (state, item) in &items {
        if *state != Interaction::Pressed {
            continue;
        }
        debug!(action = ?item.0, "菜单项被按下");
        menu.close();
        // 忙碌时不下发，行为与旧版的禁用态一致
        if session.interactive() {
            writer.write(item.0.clone());
        }
    }
}

/// 交互状态刚变化的菜单项
type ChangedMenuItems<'w, 's> =
    Query<'w, 's, (Entity, &'static Interaction), (Changed<Interaction>, With<MenuAction>)>;

/// 菜单项悬停高亮
fn sync_menu_item_style(items: ChangedMenuItems, mut commands: Commands) {
    for (entity, state) in &items {
        let token = match state {
            Interaction::Hovered | Interaction::Pressed => tokens::MENUITEM_BG_HOVER,
            Interaction::None => tokens::MENU_BG,
        };
        commands.entity(entity).insert(ThemeBackgroundColor(token));
    }
}

/// 文件列表插件
pub struct FileListPlugin;

impl Plugin for FileListPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ContextMenu>()
            .init_resource::<RenderedList>()
            .add_observer(on_row_press)
            .add_systems(
                Update,
                (
                    open_context_menu,
                    close_menu_on_outside_click,
                    handle_menu_item_press,
                )
                    .in_set(UiSet::Input),
            )
            .add_systems(
                Update,
                (
                    spawn_file_list,
                    rebuild_file_rows,
                    sync_list_title,
                    sync_file_directory,
                    sync_row_selection,
                    rebuild_context_menu,
                    sync_menu_item_style,
                )
                    .in_set(UiSet::Rebuild),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::picking::hover::{HoverMap, update_is_hovered};
    use bevy::picking::pointer::PointerId;
    use bevy::scene::ScenePlugin;

    #[test]
    fn 空白场景维护真实悬停状态且右键打开上传菜单() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), ScenePlugin))
            .init_resource::<HoverMap>()
            .init_resource::<ButtonInput<MouseButton>>()
            .insert_resource(UiScale(1.25))
            .init_resource::<ContextMenu>()
            .add_systems(Update, (update_is_hovered, open_context_menu).chain());
        let mut window = Window::default();
        window.set_cursor_position(Some(Vec2::new(100.0, 50.0)));
        app.world_mut().spawn((window, PrimaryWindow));
        let blank = app.world_mut().spawn_scene(file_list_blank()).unwrap().id();
        assert!(app.world().get::<Hovered>(blank).is_some());

        app.world_mut()
            .resource_mut::<HoverMap>()
            .0
            .entry(PointerId::Mouse)
            .or_default()
            .insert(
                blank,
                bevy::picking::backend::HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            );
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Right);
        app.update();
        let menu = app.world().resource::<ContextMenu>();
        assert!(menu.open);
        assert!(menu.target.is_none());
        assert_eq!(menu.position, Vec2::new(80.0, 40.0));
    }

    #[test]
    fn 菜单内容仍在生成时换目标或关闭不会重复排队() {
        let mut app = App::new();
        app.init_resource::<ContextMenu>()
            .init_resource::<RenderedList>()
            .insert_resource(Session::new(
                crate::model::LoginConfig::default(),
                Vec::new(),
            ))
            .add_systems(Update, rebuild_context_menu);
        let root = app
            .world_mut()
            .spawn((ContextMenuRoot, Node::default(), widgets::SlotPending))
            .id();
        let old_item = app.world_mut().spawn(ChildOf(root)).id();
        app.world_mut()
            .resource_mut::<ContextMenu>()
            .open_at(Vec2::ZERO, Some("新文件.json".into()));
        app.update();
        assert_eq!(
            app.world().get::<Node>(root).unwrap().display,
            Display::None
        );
        assert_eq!(app.world().resource::<RenderedList>().menu, None);
        assert_eq!(app.world().get::<Children>(root).unwrap().len(), 1);

        app.world_mut().resource_mut::<ContextMenu>().close();
        app.update();
        assert_eq!(app.world().resource::<RenderedList>().menu, None);
        app.world_mut()
            .entity_mut(root)
            .remove::<widgets::SlotPending>();
        app.update();
        assert!(app.world().get_entity(old_item).is_err());
        assert_eq!(app.world().resource::<RenderedList>().menu, Some(2));
    }

    #[test]
    fn 目录标签更新保留文件列表实体() {
        let mut app = App::new();
        app.init_resource::<FileBrowser>()
            .add_systems(Update, sync_file_directory);
        let label = app
            .world_mut()
            .spawn((Text::new("旧目录"), FileListDirectory))
            .id();
        let row = app.world_mut().spawn(FileRow("task.json".into())).id();
        app.world_mut().resource_mut::<FileBrowser>().remote_dir = Some("/A/task_graphs".into());
        app.update();
        assert_eq!(
            app.world().get::<Text>(label).unwrap().0,
            "当前目录：/A/task_graphs"
        );
        assert!(app.world().get::<FileRow>(row).is_some());
    }
}

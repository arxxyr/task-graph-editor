//! 文档切换和窗口关闭前保留未保存修改，确认仅作用于当时的文档。

use bevy::ecs::system::SystemParam;
use bevy::feathers::controls::ButtonVariant;
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeBorderColor};
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::prelude::*;
use bevy::window::WindowCloseRequested;

use super::connect::ActionButton;
use super::editor::InputValidation;
use super::graph_edit::GraphEditing;
use super::widgets::{self, boxed};
use super::worker_bridge::AppAction;
use super::{Editor, Session, UiSet, theme};

#[derive(Clone)]
pub(super) struct PendingTransition {
    pub document_version: u64,
    pub action: AppAction,
}

#[derive(Resource, Default)]
pub struct DocumentGuard {
    pub(super) pending: Option<PendingTransition>,
    pub(super) awaiting_save: bool,
    version: u64,
}

impl DocumentGuard {
    pub fn active(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn request(&mut self, editor: &Editor, action: AppAction) {
        self.pending = Some(PendingTransition {
            document_version: editor.document_version,
            action,
        });
        self.awaiting_save = false;
        self.version += 1;
    }

    pub(super) fn cancel(&mut self) {
        self.pending = None;
        self.awaiting_save = false;
        self.version += 1;
    }

    pub(super) fn confirm(&mut self, document_version: u64, editor: &Editor) -> Option<AppAction> {
        if !self.pending.as_ref().is_some_and(|pending| {
            pending.document_version == document_version
                && editor.document_version == document_version
        }) {
            return None;
        }
        let transition = self.pending.take()?;
        self.awaiting_save = false;
        self.version += 1;
        Some(transition.action)
    }
}

pub(super) fn replaces_document(action: &AppAction, editor: &Editor) -> bool {
    match action {
        AppAction::LoadFile(_) | AppAction::Disconnect | AppAction::CloseWindow => true,
        AppAction::DeleteFile(name) => editor
            .document
            .as_ref()
            .is_some_and(|document| document.filename == *name),
        _ => false,
    }
}

#[derive(Component)]
struct PromptSlot {
    version: Option<u64>,
}

#[derive(Component)]
struct NeedsPromptFocus;

fn setup(mut commands: Commands) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            display: Display::None,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        },
        GlobalZIndex(1000),
        PromptSlot { version: None },
    ));
}

fn collect_close_requests(
    mut requests: MessageReader<WindowCloseRequested>,
    mut actions: MessageWriter<AppAction>,
) {
    if requests.read().next().is_some() {
        actions.write(AppAction::CloseWindow);
    }
    requests.clear();
}

#[derive(SystemParam)]
struct PromptResources<'w> {
    guard: ResMut<'w, DocumentGuard>,
    editor: Res<'w, Editor>,
    editing: Res<'w, GraphEditing>,
    validation: Res<'w, InputValidation>,
    session: Res<'w, Session>,
}

fn update_prompt(
    resources: PromptResources,
    mut slots: Query<(Entity, &mut PromptSlot, &mut Node)>,
    pending_scenes: Query<(), With<widgets::SlotPending>>,
    mut actions: MessageWriter<AppAction>,
    proxy: Option<Res<bevy::winit::EventLoopProxyWrapper>>,
    mut commands: Commands,
) {
    let PromptResources {
        mut guard,
        editor,
        editing,
        validation,
        session,
    } = resources;
    if guard
        .pending
        .as_ref()
        .is_some_and(|pending| pending.document_version != editor.document_version)
    {
        guard.cancel();
    }
    if guard.awaiting_save
        && editor.pending_save.is_none()
        && !editor.has_unsaved_changes()
        && !editing.has_draft_changes()
        && !validation.has_errors(&editor)
        && let Some(action) = guard.confirm(editor.document_version, &editor)
    {
        actions.write(action);
        // 当前帧重建后排入的动作主动唤醒下一帧，不依赖 reactive 空闲计时。
        if let Some(proxy) = &proxy {
            let _ = proxy.send_event(bevy::winit::WinitUserEvent::WakeUp);
        }
    }
    for (entity, mut slot, mut node) in &mut slots {
        node.display = match guard.active() {
            true => Display::Flex,
            false => Display::None,
        };
        if slot.version == Some(guard.version) || pending_scenes.contains(entity) {
            continue;
        }
        let Some(pending) = &guard.pending else {
            slot.version = Some(guard.version);
            continue;
        };
        let mut controls = vec![boxed(widgets::button(
            "继续编辑",
            ButtonVariant::Primary,
            ActionButton(AppAction::CancelDiscard),
        ))];
        if session.is_connected && editor.document.is_some() {
            controls.push(boxed(widgets::button(
                "保存并继续",
                ButtonVariant::Normal,
                ActionButton(AppAction::SaveAndContinue),
            )));
        }
        controls.push(boxed(widgets::button(
            "放弃修改并继续",
            ButtonVariant::Normal,
            ActionButton(AppAction::ConfirmDiscard(pending.document_version)),
        )));
        widgets::replace_slot_children(
            &mut commands,
            entity,
            vec![boxed(bsn! {
                Node {
                    width: px(460), max_width: percent(90),
                    flex_direction: FlexDirection::Column,
                    padding: {UiRect::all(px(20))},
                    row_gap: px(16), border: {UiRect::all(px(1))},
                    border_radius: {BorderRadius::all(px(8))},
                }
                ThemeBackgroundColor({theme::SIDEBAR_BG})
                ThemeBorderColor({theme::DIVIDER})
                Children [
                    (widgets::readonly_value("当前文档有未保存的修改")),
                    (widgets::hint("切换文件、断开或关闭前，请选择如何处理修改。无效输入和尚未应用的流程草稿也会保留。")),
                    (
                        Node { flex_direction: FlexDirection::Row, flex_wrap: FlexWrap::Wrap, column_gap: px(8), row_gap: px(8) }
                        Children [{controls}]
                    )
                ]
            })],
        );
        commands.entity(entity).insert(NeedsPromptFocus);
        slot.version = Some(guard.version);
    }
}

fn focus_prompt(
    prompts: Query<Entity, With<NeedsPromptFocus>>,
    buttons: Query<(Entity, &ActionButton)>,
    mut focus: ResMut<InputFocus>,
    mut commands: Commands,
) {
    for prompt in &prompts {
        if let Some((entity, _)) = buttons
            .iter()
            .find(|(_, button)| matches!(button.0, AppAction::CancelDiscard))
        {
            focus.set(entity, FocusCause::Navigated);
            commands.entity(prompt).remove::<NeedsPromptFocus>();
        }
    }
}

pub struct DocumentGuardPlugin;

impl Plugin for DocumentGuardPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DocumentGuard>()
            .add_message::<WindowCloseRequested>()
            .add_systems(Startup, setup)
            .add_systems(Update, collect_close_requests.in_set(UiSet::Input))
            .add_systems(
                Update,
                (update_prompt, focus_prompt).chain().in_set(UiSet::Rebuild),
            );
    }
}

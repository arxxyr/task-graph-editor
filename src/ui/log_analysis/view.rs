//! 日志页面场景、增量列表与状态同步。

use super::*;
#[path = "report_view.rs"]
mod report_view;

fn action_button(label: impl Into<String>, action: LogAction) -> impl Scene {
    let marker = match &action {
        LogAction::Toggle { version, index } => ChoiceMarker {
            version: *version,
            index: *index,
        },
        _ => ChoiceMarker {
            version: u64::MAX,
            index: usize::MAX,
        },
    };
    let variant = if matches!(action, LogAction::Analyze) {
        ButtonVariant::Primary
    } else {
        ButtonVariant::Plain
    };
    bsn! {
        @FeathersButton {
            @caption: {bsn! { Text({label.into()}) ThemedText TextLayout { linebreak: bevy::text::LineBreak::AnyCharacter } }},
            @variant: variant
        }
        template_value(action)
        template_value(marker)
        on(click)
    }
}

fn caption(marker: impl widgets::Marker) -> impl Scene {
    bsn! {
        Text("") ThemedText TextFont { font_size: px(12) }
        TextLayout { linebreak: bevy::text::LineBreak::AnyCharacter }
        template_value(marker)
    }
}

fn report_text(text: impl Into<String>) -> impl Scene {
    bsn! {
        Text({text.into()}) ThemedText TextFont { font_size: px(12) }
        TextLayout { linebreak: bevy::text::LineBreak::AnyCharacter }
        Node { min_width: px(0), width: percent(100) }
    }
}

fn action_menu(label: &str, entries: Vec<(&str, LogAction)>) -> impl Scene {
    widgets::action_menu(
        label,
        entries
            .into_iter()
            .map(|(label, action)| {
                boxed(bsn! {
                    widgets::menu_action(label, action, widgets::ButtonGate::Managed)
                    on(click)
                })
            })
            .collect(),
    )
}

pub fn pane() -> impl Scene {
    bsn! {
        Node {
            display: Display::None, width: percent(100), height: percent(100),
            flex_direction: FlexDirection::Column, row_gap: px(16),
            padding: px(20), min_height: px(0),
        }
        LogPane
        Children [
            (
                Node { align_items: AlignItems::Center, column_gap: px(8), flex_shrink: 0.0, flex_wrap: FlexWrap::Wrap }
                Children [
                    widgets::subheading("日志分析"), widgets::spacer(),
                    action_button("报告目录", LogAction::ReportDirectory),
                    action_button("开始分析", LogAction::Analyze),
                    action_button("取消", LogAction::Cancel),
                ]
            ),
            (Node { flex_direction: FlexDirection::Column, row_gap: px(12), flex_grow: 1.0, min_height: px(0), overflow: {Overflow::scroll_y()} }
            ScrollArea
            Children [
            (widgets::collapsible("日志选择与分析设置", None, true, bsn_list![
                (
                    Node { column_gap: px(8), align_items: AlignItems::Center, flex_wrap: FlexWrap::Wrap, row_gap: px(8) }
                    Children [
                        widgets::text_field("~/.ros/log", DirectoryField),
                        action_button("读取远程", LogAction::Remote),
                        action_menu("导入本地", vec![("选择日志文件", LogAction::LocalFiles), ("选择日志目录", LogAction::LocalDirectory)]),
                    ]
                ),
                (
                    Node { column_gap: px(8), align_items: AlignItems::Center }
                    Children [
                        action_button("上一级", LogAction::Parent),
                        widgets::hint("筛选"), widgets::text_field("", FilterField),
                        action_menu("选择", vec![("全选筛选结果", LogAction::SelectAll), ("清空选择", LogAction::Clear)]),
                    ]
                ),
                (Node { column_gap: px(16), row_gap: px(8), flex_wrap: FlexWrap::Wrap } Children [
                    action_button("动作与轮次", LogAction::Mode(false)),
                    action_button("工件节拍", LogAction::Mode(true)),
                    statistics_checkbox("纳入暂停轮次", StatisticsFlag::Paused),
                    statistics_checkbox("纳入错误轮次", StatisticsFlag::Errors),
                ]),
                (
                    Node { flex_direction: FlexDirection::Column, row_gap: px(8) }
                    WorkpieceOptionsPane
                    Children [
                        (Node { column_gap: px(8), align_items: AlignItems::Center } Children [
                            widgets::hint("北京时间日期"), widgets::text_field("", DateField), widgets::hint("YYYY-MM-DD · 留空为全部"),
                        ]),
                        (Node { flex_wrap: FlexWrap::Wrap, column_gap: px(20), row_gap: px(8) } Children [
                            statistics_checkbox("纳入人工干预轮次", StatisticsFlag::Interventions),
                            statistics_checkbox("扣除搬框等待", StatisticsFlag::DeductWait),
                        ]),
                    ]
                ),
                caption(SelectionCaption),
                (
                    Node { flex_direction: FlexDirection::Column, row_gap: px(2),
                        max_height: px(300), min_height: px(48), overflow: {Overflow::scroll_y()}, flex_shrink: 0.0 }
                    ScrollArea
                    ListSlot
                ),
                (Node { justify_content: JustifyContent::FlexEnd, column_gap: px(8) }
                    Children [action_button("上一页", LogAction::Previous), action_button("下一页", LogAction::Next)]),

                widgets::collapsible("口径说明与输出位置", None, false, bsn_list![
                    caption(ModeCaption),
                    widgets::hint("设置在下次分析生效；缺失、重复、乱序及未闭合的工件周期始终排除。"),
                    caption(OutputCaption),
                    action_button("更改报告目录", LogAction::Output),
                ]),
            ]) ReportSetup),
            caption(LogStatus),
            (
                Node { flex_direction: FlexDirection::Column, row_gap: px(8), flex_shrink: 0.0 }
                ReportSlot
            ),
            ]),
        ]
    }
}

#[derive(Component, Clone, Default)]
pub(super) struct OutputCaption;

type CaptionQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Text,
        Option<&'static LogStatus>,
        Has<OutputCaption>,
    ),
    Or<(With<LogStatus>, With<SelectionCaption>, With<OutputCaption>)>,
>;

pub(super) fn sync_pane(mode: Res<ViewMode>, mut panes: Query<&mut Node, With<LogPane>>) {
    let display = match *mode {
        ViewMode::Logs => Display::Flex,
        _ => Display::None,
    };
    for mut pane in &mut panes {
        if pane.display != display {
            pane.display = display;
        }
    }
}

pub(super) fn sync(
    logs: Res<Logs>,
    session: Res<Session>,
    mut captions: CaptionQuery,
    mut choices: Query<(&ChoiceMarker, &LogAction, &mut ButtonVariant)>,
    mut fields: Query<&mut EditableText, With<DirectoryField>>,
    mut rendered_directory: Local<Option<u64>>,
) {
    for (mut text, status, output) in &mut captions {
        if !logs.is_changed() && !session.is_changed() && !text.is_added() {
            continue;
        }
        let value = match status {
            _ if output => format!("报告目录：{}", logs.output_parent.display()),
            Some(_) => logs.status.clone(),
            None => {
                let source = match &logs.source {
                    Some((generation, directory))
                        if *generation == session.connection_generation && session.is_connected =>
                    {
                        format!("远程列表：{directory}")
                    }
                    Some((_, directory)) => format!("远程列表已失效：{directory}（请刷新）"),
                    None => "本地文件".into(),
                };
                format!(
                    "{source}\n共 {} 项 · 已选 {} 份日志 · 第 {} / {} 页",
                    logs.choices.len(),
                    logs.selected.len(),
                    logs.page + 1,
                    logs.visible_indices().len().div_ceil(PAGE_SIZE).max(1)
                )
            }
        };
        if text.0 != value {
            text.0 = value;
        }
    }
    for (choice, action, mut variant) in &mut choices {
        if !logs.is_changed() && !variant.is_added() {
            continue;
        }
        let selected = match action {
            LogAction::Mode(value) => *value == logs.workpiece,
            _ => choice.version == logs.version && logs.selected.contains(&choice.index),
        };
        variant.set_if_neq(match selected {
            true => ButtonVariant::Primary,
            false if matches!(action, LogAction::Analyze) => ButtonVariant::Primary,
            false => ButtonVariant::Plain,
        });
    }
    if *rendered_directory != Some(logs.directory_version) && !fields.is_empty() {
        for mut field in &mut fields {
            if field.value().to_string() != logs.directory {
                field.queue_edit(TextEdit::SelectAll);
                field.queue_edit(TextEdit::Insert(logs.directory.as_str().into()));
            }
        }
        *rendered_directory = Some(logs.directory_version);
    }
}

pub(super) fn rebuild(
    mut logs: ResMut<Logs>,
    lists: Query<Entity, With<ListSlot>>,
    reports: Query<Entity, With<ReportSlot>>,
    pending: Query<(), With<widgets::SlotPending>>,
    proxy: Option<Res<EventLoopProxyWrapper>>,
    mut commands: Commands,
) {
    if logs.rendered_list != Some(logs.list_revision) {
        for slot in &lists {
            if pending.contains(slot) {
                wake_fn(proxy.as_deref())();
                continue;
            }
            let items: Vec<BoxedScene> = logs
                .visible_indices()
                .iter()
                .copied()
                .skip(logs.page * PAGE_SIZE)
                .take(PAGE_SIZE)
                .map(|index| {
                    let choice = &logs.choices[index];
                    let version = logs.version;
                    let action = match choice.directory {
                        true => LogAction::Enter { version, index },
                        false => LogAction::Toggle { version, index },
                    };
                    boxed(bsn! {
                        action_button(choice.label.clone(), action)
                        Node { width: percent(100), justify_content: JustifyContent::FlexStart, min_height: px(30), flex_shrink: 0.0 }
                    })
                })
                .collect();
            let items = match items.is_empty() {
                true => vec![boxed(widgets::hint(match logs.source {
                    Some(_) => {
                        "没有匹配的日期目录或主控日志。远程列表仅显示 YYYYMMDD 日期目录和 master_control 日志，也请检查文件名筛选条件。"
                    }
                    None => "没有文件。选择本地日志，或输入远程目录后点击浏览。",
                }))],
                false => items,
            };
            widgets::replace_slot_children(&mut commands, slot, items);
            wake_fn(proxy.as_deref())();
            logs.rendered_list = Some(logs.list_revision);
        }
    }
    if logs.rendered_report != Some(logs.report_revision) {
        for slot in &reports {
            if pending.contains(slot) {
                wake_fn(proxy.as_deref())();
                continue;
            }
            let mut items = Vec::<BoxedScene>::new();
            if let Some(report) = &logs.report {
                items.extend(report_view::content(&logs, report));
                for warning in &report.warnings {
                    items.push(boxed(report_text(format!("注意：{warning}"))));
                }
            }
            widgets::replace_slot_children(&mut commands, slot, items);
            wake_fn(proxy.as_deref())();
            logs.rendered_report = Some(logs.report_revision);
        }
    }
}

/// 日志页暂时隐藏任务图操作组，切回编辑视图恢复原实体与状态。
type DocumentSlots<'w, 's> = Query<
    'w,
    's,
    &'static mut Node,
    Or<(
        With<super::super::shell::ActionBarSlot>,
        With<super::super::shell::FileListSlot>,
    )>,
>;

pub(super) fn sync_document_actions(mode: Res<ViewMode>, mut slots: DocumentSlots) {
    let display = match *mode {
        ViewMode::Logs => Display::None,
        _ => Display::Flex,
    };
    for mut node in &mut slots {
        if node.display != display {
            node.display = display;
        }
    }
}

/// 禁用态只由本系统维护，跨帧新增按钮也同步；重建命令完成后才访问实体。
pub(super) fn sync_gates(
    logs: Res<Logs>,
    session: Res<Session>,
    mut buttons: Query<(
        Entity,
        &LogAction,
        Has<bevy::ui::InteractionDisabled>,
        &mut Node,
    )>,
    mut commands: Commands,
) {
    for (entity, action, disabled, mut node) in &mut buttons {
        let visible = match action {
            LogAction::Cancel => logs.task.is_some() || logs.remote.is_some(),
            LogAction::Parent => logs.source.is_some(),
            LogAction::Previous | LogAction::Next => logs.visible.len() > PAGE_SIZE,
            LogAction::ReportLog(_) => logs.report.as_ref().is_some_and(|r| r.reports.len() > 1),
            LogAction::ReportPage(_) => logs
                .report
                .as_ref()
                .and_then(|r| r.reports.get(logs.report_index))
                .is_some_and(|r| r.rounds.len() > PAGE_SIZE),
            _ => true,
        };
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
        let enabled = match action {
            LogAction::Cancel => logs.task.is_some() || logs.remote.is_some(),
            LogAction::Open(_) => true,
            LogAction::ReportDirectory => logs.report.is_some(),
            LogAction::ReportLog(forward) => {
                !logs.busy()
                    && match forward {
                        true => logs
                            .report
                            .as_ref()
                            .is_some_and(|r| logs.report_index + 1 < r.reports.len()),
                        false => logs.report_index > 0,
                    }
            }
            LogAction::ReportPage(forward) => {
                !logs.busy()
                    && match forward {
                        true => logs
                            .report
                            .as_ref()
                            .and_then(|r| r.reports.get(logs.report_index))
                            .is_some_and(|r| (logs.report_page + 1) * PAGE_SIZE < r.rounds.len()),
                        false => logs.report_page > 0,
                    }
            }
            LogAction::Analyze => !logs.busy() && !logs.selected.is_empty(),
            LogAction::Remote => !logs.busy() && session.is_connected && session.interactive(),
            LogAction::Parent => !logs.busy() && logs.source.is_some() && session.is_connected,
            LogAction::Previous => !logs.busy() && logs.page > 0,
            LogAction::Next => !logs.busy() && (logs.page + 1) * PAGE_SIZE < logs.visible.len(),
            _ => !logs.busy(),
        };
        match (enabled, disabled) {
            (true, true) => {
                commands
                    .entity(entity)
                    .remove::<bevy::ui::InteractionDisabled>();
            }
            (false, false) => {
                commands
                    .entity(entity)
                    .insert(bevy::ui::InteractionDisabled);
            }
            _ => {}
        }
    }
}

#[derive(Component, Clone, Default)]
pub(super) struct WorkpieceOptionsPane;
#[derive(Component, Clone, Default)]
pub(super) struct ModeCaption;

fn statistics_checkbox(label: &str, flag: StatisticsFlag) -> impl Scene {
    bsn! { @FeathersCheckbox { @caption: {bsn! { Text({label.to_string()}) ThemedText }} } template_value(flag) }
}

pub(super) fn sync_statistics(
    logs: Res<Logs>,
    flags: Query<(
        Entity,
        &StatisticsFlag,
        Has<bevy::ui::Checked>,
        Has<bevy::ui::InteractionDisabled>,
    )>,
    mut panes: Query<&mut Node, With<WorkpieceOptionsPane>>,
    dates: Query<(Entity, Has<bevy::ui::InteractionDisabled>), With<DateField>>,
    mut captions: Query<&mut Text, With<ModeCaption>>,
    mut commands: Commands,
) {
    for (entity, flag, checked, disabled) in &flags {
        let value = match flag {
            StatisticsFlag::Paused => logs.statistics.include_paused,
            StatisticsFlag::Errors => logs.statistics.include_errors,
            StatisticsFlag::Interventions => logs.include_interventions,
            StatisticsFlag::DeductWait => logs.deduct_wait,
        };
        if value != checked {
            match value {
                true => {
                    commands.entity(entity).insert(bevy::ui::Checked);
                }
                false => {
                    commands.entity(entity).remove::<bevy::ui::Checked>();
                }
            }
        }
        if logs.busy() != disabled {
            match logs.busy() {
                true => {
                    commands
                        .entity(entity)
                        .insert(bevy::ui::InteractionDisabled);
                }
                false => {
                    commands
                        .entity(entity)
                        .remove::<bevy::ui::InteractionDisabled>();
                }
            }
        }
    }
    for (entity, disabled) in &dates {
        if logs.busy() != disabled {
            match logs.busy() {
                true => {
                    commands
                        .entity(entity)
                        .insert(bevy::ui::InteractionDisabled);
                }
                false => {
                    commands
                        .entity(entity)
                        .remove::<bevy::ui::InteractionDisabled>();
                }
            }
        }
    }
    for mut pane in &mut panes {
        pane.display = match logs.workpiece {
            true => Display::Flex,
            false => Display::None,
        };
    }
    for mut text in &mut captions {
        let value = match logs.workpiece {
            true => {
                "工件节拍：相邻取件导航创建为周期边界，校验取件、气密换件、回框放件成功记录。支持日期、工位和小时统计及交互证据报告。"
            }
            false => {
                "动作与轮次：沿用日志中的循环标记，生成动作明细和甘特图。勾选项仅影响常规轮次耗时统计，完整明细仍保留。"
            }
        };
        if text.0 != value {
            text.0 = value.into();
        }
    }
}

#[derive(Component, Clone, Default)]
pub(super) struct ReportSetup;

/// 新报告生成后收起配置，后续翻页和用户手动展开不改变其状态。
pub(super) fn sync_report_setup(
    logs: Res<Logs>,
    mut panels: Query<(Ref<ReportSetup>, &mut widgets::Collapsible)>,
    mut previous: Local<Option<PathBuf>>,
) {
    let output = logs.report.as_ref().map(|r| r.output.clone());
    for (marker, mut section) in &mut panels {
        if output.is_some() && (*previous != output || marker.is_added()) {
            section.open = false;
        }
    }
    *previous = output;
}

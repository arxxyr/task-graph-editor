use bevy::feathers::FeathersCorePlugin;
use bevy::feathers::controls::{
    ButtonVariant, FeathersButton, FeathersDisclosureToggle, FeathersMenuButton, FeathersSlider,
    FeathersTextInput,
};
use bevy::feathers::focus::FocusWithinIndicator;
use bevy::feathers::theme::{
    InheritableThemeTextColor, ThemeBackgroundColor, ThemeBorderColor, ThemeTextColor, ThemedText,
};
use bevy::input::InputPlugin;
use bevy::input_focus::{FocusCause, InputFocus, InputFocusVisible};
use bevy::picking::hover::Hovered;
use bevy::text::{EditableText, TextCursorStyle};
use bevy::ui::{InteractionDisabled, Pressed};
use bevy::ui_widgets::{SliderPrecision, SliderRange, SliderValue};

use super::*;

#[test]
fn embedded_palettes_match_ids_names_modes_and_exact_srgb() {
    let records: Vec<serde_json::Value> = serde_json::from_str(palette::PALETTES_JSON).unwrap();
    let parsed = palette::parse_palettes(palette::PALETTES_JSON).unwrap();
    for (index, id) in ThemeId::ALL.into_iter().enumerate() {
        assert_eq!(records[index]["id"], id.id());
        assert_eq!(records[index]["name"], id.label());
        assert_eq!(records[index]["mode"] == "dark", id.is_dark());
        assert_eq!(records[index]["colors"].as_object().unwrap().len(), 36);
        assert_eq!(parsed[index].id, id);
        assert_eq!(serde_json::to_value(id).unwrap(), id.id());
        assert_eq!(
            serde_json::from_value::<ThemeId>(records[index]["id"].clone()).unwrap(),
            id
        );
        let theme = create_app_theme(id);
        for (token, key) in [
            (CONTENT_BG, "canvas"),
            (CARD_BG, "panel"),
            (INPUT_BORDER, "border"),
            (GRAPH_BORDER, "border"),
            (GRAPH_SELECTED_BG, "selected"),
            (GRAPH_SELECTED_TEXT, "selected_text"),
            (POSE_SELECTED_BG, "pose_bg"),
            (POSE_SELECTED_BORDER, "pose_border"),
            (POSE_SELECTED_TEXT, "pose_text"),
            (COPY_BG, "copy_bg"),
            (COPY_BG_HOVER, "copy_hover"),
            (COPY_TEXT, "copy_text"),
            (tokens::TEXT_INPUT_X_AXIS, "axis_x"),
            (tokens::TEXT_INPUT_Y_AXIS, "axis_y"),
            (tokens::TEXT_INPUT_Z_AXIS, "axis_z"),
        ] {
            let hex = records[index]["colors"][key].as_str().unwrap();
            assert_eq!(theme.color(&token), Color::Srgba(Srgba::hex(hex).unwrap()));
        }
    }
    assert!(serde_json::from_str::<ThemeId>("\"deep-ocean\"").is_err());
}

#[test]
fn incomplete_duplicate_or_invalid_palettes_are_rejected() {
    let source: serde_json::Value = serde_json::from_str(palette::PALETTES_JSON).unwrap();
    let mut missing = source.clone();
    missing[0]["colors"]
        .as_object_mut()
        .unwrap()
        .remove("copy_bg");
    assert!(palette::parse_palettes(&missing.to_string()).is_err());
    let mut duplicate = source.clone();
    duplicate[1]["id"] = duplicate[0]["id"].clone();
    assert!(palette::parse_palettes(&duplicate.to_string()).is_err());
    for invalid in ["#FFFFFF00", "#XYZ123", "FFFFFF", "#fff"] {
        let mut malformed = source.clone();
        malformed[2]["colors"]["text"] = invalid.into();
        assert!(palette::parse_palettes(&malformed.to_string()).is_err());
    }
}

#[test]
fn every_feathers_color_is_explicit_and_comes_from_the_selected_palette() {
    // 上游主题仅作为 token 清单使用，生产工厂从空映射创建，不能继承其深色值。
    let upstream = bevy::feathers::dark_theme::create_dark_theme();
    let source: Vec<serde_json::Value> = serde_json::from_str(palette::PALETTES_JSON).unwrap();
    for (id, record) in ThemeId::ALL.into_iter().zip(source) {
        let theme = create_app_theme(id);
        assert_eq!(upstream.color.len(), 137);
        for token in upstream.color.keys() {
            assert!(
                theme.0.color.contains_key(token),
                "{} 缺少 {token}",
                id.id()
            );
        }
        let mut allowed: Vec<Color> = record["colors"]
            .as_object()
            .unwrap()
            .values()
            .map(|hex| Color::Srgba(Srgba::hex(hex.as_str().unwrap()).unwrap()))
            .collect();
        let p = palette::colors(id);
        allowed.extend([Color::NONE, soft_border(p.border, p.panel)]);
        for (token, color) in &theme.0.color {
            if let Some(swatch) = ThemeId::ALL
                .into_iter()
                .find(|swatch| swatch.swatch_token() == *token)
            {
                assert_eq!(*color, palette::colors(swatch).accent);
                continue;
            }
            assert!(
                allowed.contains(color),
                "{} 的 {token} 遗留其他色板颜色",
                id.id()
            );
        }
        assert_ne!(theme.color(&CARD_BORDER), theme.color(&GRAPH_BORDER));
    }
}

fn luminance(color: Color) -> f32 {
    let color = color.to_linear();
    color.red * 0.2126 + color.green * 0.7152 + color.blue * 0.0722
}

fn contrast(first: Color, second: Color) -> f32 {
    let first = luminance(first);
    let second = luminance(second);
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

#[test]
fn four_themes_keep_text_readable_in_normal_pressed_selected_and_error_states() {
    for id in ThemeId::ALL {
        let theme = create_app_theme(id);
        let pairs = [
            (tokens::TEXT_MAIN, CARD_BG),
            (tokens::TEXT_INPUT_TEXT, tokens::TEXT_INPUT_BG),
            (FIELD_LABEL, CARD_BG),
            (READONLY_TEXT, SIDEBAR_BG),
            (tokens::BUTTON_TEXT, tokens::BUTTON_BG),
            (tokens::BUTTON_TEXT, tokens::BUTTON_BG_HOVER),
            (tokens::BUTTON_TEXT, tokens::BUTTON_BG_PRESSED),
            (tokens::BUTTON_PRIMARY_TEXT, tokens::BUTTON_PRIMARY_BG),
            (tokens::BUTTON_PRIMARY_TEXT, tokens::BUTTON_PRIMARY_BG_HOVER),
            (
                tokens::BUTTON_PRIMARY_TEXT,
                tokens::BUTTON_PRIMARY_BG_PRESSED,
            ),
            (tokens::MENUITEM_TEXT, tokens::MENUITEM_BG_PRESSED),
            (tokens::MENUITEM_TEXT, tokens::MENUITEM_BG_FOCUSED),
            (tokens::SLIDER_TEXT, tokens::SLIDER_BAR_PRESSED),
            (tokens::SLIDER_TEXT, tokens::SLIDER_BG_PRESSED),
            (tokens::TEXT_INPUT_TEXT, tokens::TEXT_INPUT_SELECTION),
            (tokens::LISTROW_TEXT, tokens::LISTROW_BG_SELECTED),
            (GRAPH_SELECTED_TEXT, GRAPH_SELECTED_BG),
            (POSE_SELECTED_TEXT, POSE_SELECTED_BG),
            (COPY_TEXT, COPY_BG),
            (COPY_TEXT, COPY_BG_HOVER),
            (STATUS_OK, STATUSBAR_BG),
            (STATUS_WARN, STATUSBAR_BG),
            (STATUS_ERROR, STATUSBAR_BG),
            (DANGER_TEXT, DANGER_BG),
        ];
        for (foreground, background) in pairs {
            let ratio = contrast(theme.color(&foreground), theme.color(&background));
            assert!(
                ratio >= 4.5,
                "{} 的 {foreground}/{background} 对比度只有 {ratio:.2}",
                id.id()
            );
        }
        assert!(
            contrast(
                theme.color(&INPUT_BORDER),
                theme.color(&tokens::TEXT_INPUT_BG)
            ) >= 3.0
        );
        assert!(contrast(theme.color(&GRAPH_BORDER), theme.color(&CARD_BG)) >= 3.0);
    }
}

#[derive(Resource, Default)]
struct QueuedTheme(Option<ThemeId>);

#[derive(Resource, Default)]
struct ColorsAtContent(Vec<(Entity, Color)>);

fn record_content_colors(
    colors: Query<(Entity, &TextColor)>,
    mut snapshot: ResMut<ColorsAtContent>,
) {
    snapshot.0.clear();
    snapshot
        .0
        .extend(colors.iter().map(|(entity, color)| (entity, color.0)));
}

fn select_during_rebuild(mut queued: ResMut<QueuedTheme>, mut selection: ResMut<ThemeSelection>) {
    if let Some(id) = queued.0.take() {
        selection.set_if_neq(ThemeSelection(id));
    }
}

fn theme_app(initial: ThemeId) -> App {
    let mut app = App::new();
    // 使用真实 Feathers 主题、样式及传播系统，不启动窗口、渲染器、配置或剪贴板。
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        bevy::scene::ScenePlugin,
        InputPlugin,
    ))
    .init_asset::<Font>()
    .init_asset::<Image>()
    .init_asset::<bevy::shader::Shader>()
    .init_resource::<InputFocus>()
    .init_resource::<InputFocusVisible>()
    .init_resource::<UiScale>()
    .init_resource::<QueuedTheme>()
    .init_resource::<ColorsAtContent>()
    .insert_resource(ThemeSelection(initial))
    .add_plugins((FeathersCorePlugin, ThemePlugin))
    .add_systems(
        Update,
        select_during_rebuild.in_set(super::super::UiSet::Rebuild),
    )
    .add_systems(
        PostUpdate,
        record_content_colors.in_set(bevy::ui::UiSystems::Content),
    );
    app
}

fn themed_button(app: &mut App, variant: ButtonVariant) -> (Entity, Entity) {
    let button = app
        .world_mut()
        .spawn((
            Node::default(),
            FeathersButton,
            variant,
            Hovered::default(),
            ThemeBackgroundColor(tokens::BUTTON_BG),
            InheritableThemeTextColor(tokens::BUTTON_TEXT),
        ))
        .id();
    let text = app
        .world_mut()
        .spawn((Text::new("操作"), ThemedText, ChildOf(button)))
        .id();
    (button, text)
}

#[test]
fn switching_theme_refreshes_existing_feathers_entities_in_the_same_frame() {
    let mut app = theme_app(ThemeId::DuskSand);
    assert_eq!(
        app.world().resource::<ThemeSelection>().0,
        ThemeId::DuskSand
    );
    let surface = app
        .world_mut()
        .spawn((
            Node::default(),
            ThemeBackgroundColor(CARD_BG),
            ThemeBorderColor(INPUT_BORDER),
            ScrollPosition(Vec2::new(17.0, 83.0)),
        ))
        .id();
    let direct = app
        .world_mut()
        .spawn((Text::new("提示"), ThemeTextColor(READONLY_TEXT)))
        .id();
    let (normal, normal_text) = themed_button(&mut app, ButtonVariant::Normal);
    app.world_mut().entity_mut(normal).insert(Pressed);
    let (primary, primary_text) = themed_button(&mut app, ButtonVariant::Primary);
    app.world_mut().entity_mut(primary).insert(Hovered(true));
    let (disabled, disabled_text) = themed_button(&mut app, ButtonVariant::Primary);
    app.world_mut()
        .entity_mut(disabled)
        .insert(InteractionDisabled);
    let input_frame = app
        .world_mut()
        .spawn((
            Node::default(),
            ThemeBackgroundColor(tokens::TEXT_INPUT_BG),
            InheritableThemeTextColor(tokens::TEXT_INPUT_TEXT),
            FocusWithinIndicator,
        ))
        .id();
    let input = app
        .world_mut()
        .spawn((
            FeathersTextInput,
            EditableText::new("0.123456789 未保存"),
            TextCursorStyle::default(),
            ChildOf(input_frame),
        ))
        .id();
    let slider = app
        .world_mut()
        .spawn((
            Node::default(),
            FeathersSlider,
            SliderValue(0.25),
            SliderRange::new(0.0, 1.0),
            SliderPrecision(2),
            Hovered(true),
            BackgroundGradient(vec![Gradient::Linear(LinearGradient {
                stops: vec![
                    ColorStop::new(Color::NONE, percent(0)),
                    ColorStop::new(Color::NONE, percent(25)),
                    ColorStop::new(Color::NONE, percent(25)),
                    ColorStop::new(Color::NONE, percent(100)),
                ],
                ..default()
            })]),
        ))
        .id();
    let disclosure = app
        .world_mut()
        .spawn((
            Node::default(),
            FeathersDisclosureToggle,
            UiTransform::default(),
        ))
        .id();
    let icon = app
        .world_mut()
        .spawn((ImageNode::default(), ChildOf(disclosure)))
        .id();
    app.world_mut()
        .resource_mut::<InputFocus>()
        .set(input, FocusCause::Pressed);
    app.world_mut().resource_mut::<InputFocusVisible>().0 = true;
    app.update();
    let count = app.world().entities().len();
    let selection = app
        .world()
        .get::<EditableText>(input)
        .unwrap()
        .editor()
        .raw_selection();
    let selection = (selection.anchor().index(), selection.focus().index());
    let children: Vec<Entity> = app
        .world()
        .get::<Children>(input_frame)
        .unwrap()
        .iter()
        .collect();

    for id in [
        ThemeId::WarmPaper,
        ThemeId::GraphiteTeal,
        ThemeId::MistIndigo,
        ThemeId::DuskSand,
    ] {
        // 在 Update 的 Rebuild 阶段才发出主题变化；单次 update 后就必须看到最终颜色。
        app.world_mut().resource_mut::<QueuedTheme>().0 = Some(id);
        app.update();
        let p = palette::colors(id);
        let world = app.world();
        assert_eq!(world.entities().len(), count);
        assert_eq!(world.get::<BackgroundColor>(surface).unwrap().0, p.panel);
        assert_eq!(
            *world.get::<BorderColor>(surface).unwrap(),
            BorderColor::all(p.border)
        );
        assert_eq!(world.get::<TextColor>(direct).unwrap().0, p.faint);
        assert_eq!(world.get::<BackgroundColor>(normal).unwrap().0, p.selected);
        assert_eq!(world.get::<TextColor>(normal_text).unwrap().0, p.text);
        assert_eq!(
            world.get::<BackgroundColor>(primary).unwrap().0,
            p.accent_hover
        );
        assert_eq!(world.get::<TextColor>(primary_text).unwrap().0, p.on_accent);
        assert_eq!(
            world.get::<BackgroundColor>(disabled).unwrap().0,
            p.disabled_bg
        );
        assert_eq!(
            world.get::<TextColor>(disabled_text).unwrap().0,
            p.disabled_text
        );
        assert_eq!(world.get::<TextColor>(input).unwrap().0, p.text);
        // 检查排版阶段读到的实际颜色，不能只在一帧结束后看组件是否最终更新。
        let content_colors = &world.resource::<ColorsAtContent>().0;
        for (entity, expected) in [
            (direct, p.faint),
            (normal_text, p.text),
            (primary_text, p.on_accent),
            (disabled_text, p.disabled_text),
            (input, p.text),
        ] {
            assert!(content_colors.contains(&(entity, expected)));
        }
        let cursor = world.get::<TextCursorStyle>(input).unwrap();
        assert_eq!(cursor.color, p.accent);
        assert_eq!(cursor.selection_color, p.selected);
        assert_eq!(cursor.unfocused_selection_color, p.header);
        assert_eq!(world.get::<Outline>(input_frame).unwrap().color, p.accent);
        let gradient = world.get::<BackgroundGradient>(slider).unwrap();
        let Gradient::Linear(linear) = &gradient.0[0] else {
            panic!("滑块应使用线性渐变")
        };
        assert_eq!(linear.stops[0].color, p.selected);
        assert_eq!(linear.stops[2].color, p.header);
        assert_eq!(linear.stops[1].point, percent(25));
        assert_eq!(world.get::<SliderValue>(slider).unwrap().0, 0.25);
        assert_eq!(world.get::<ImageNode>(icon).unwrap().color, p.text);
        assert_eq!(world.resource::<InputFocus>().get(), Some(input));
        assert_eq!(
            world.get::<ScrollPosition>(surface).unwrap().0,
            Vec2::new(17.0, 83.0)
        );
        assert_eq!(
            world
                .get::<EditableText>(input)
                .unwrap()
                .value()
                .to_string(),
            "0.123456789 未保存"
        );
        let current_selection = world
            .get::<EditableText>(input)
            .unwrap()
            .editor()
            .raw_selection();
        assert_eq!(
            (
                current_selection.anchor().index(),
                current_selection.focus().index()
            ),
            selection
        );
        assert_eq!(
            world
                .get::<Children>(input_frame)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            children
        );
    }
}

#[test]
fn unchanged_selection_does_not_rewrite_the_theme_or_existing_colors() {
    let mut app = theme_app(ThemeId::MistIndigo);
    let (button, caption) = themed_button(&mut app, ButtonVariant::Normal);
    app.update();
    let before = app
        .world()
        .get_resource_ref::<UiTheme>()
        .unwrap()
        .last_changed();
    let color_before = app
        .world()
        .get_entity(caption)
        .unwrap()
        .get_ref::<TextColor>()
        .unwrap()
        .last_changed();
    app.update();
    assert_eq!(
        app.world()
            .get_resource_ref::<UiTheme>()
            .unwrap()
            .last_changed(),
        before
    );
    assert_eq!(
        app.world()
            .get_entity(caption)
            .unwrap()
            .get_ref::<TextColor>()
            .unwrap()
            .last_changed(),
        color_before
    );
    assert!(app.world().get_entity(button).is_ok());
}

#[test]
fn late_text_inputs_use_current_palette_in_either_component_insertion_order() {
    for id in ThemeId::ALL {
        let mut app = theme_app(id);
        app.update();
        app.update();
        let p = palette::colors(id);
        // 使用 Feathers 场景提供的默认样式，验证插入时会覆盖其默认蓝色。
        let marker_first = app
            .world_mut()
            .spawn((FeathersTextInput, EditableText::new("迟到控件")))
            .id();
        app.world_mut()
            .entity_mut(marker_first)
            .insert(TextCursorStyle::default());
        let style_first = app.world_mut().spawn(TextCursorStyle::default()).id();
        app.world_mut()
            .entity_mut(style_first)
            .insert(FeathersTextInput);
        for entity in [marker_first, style_first] {
            assert_eq!(app.world().get::<TextColor>(entity).unwrap().0, p.text);
            let cursor = app.world().get::<TextCursorStyle>(entity).unwrap();
            assert_eq!(cursor.color, p.accent);
            assert_eq!(cursor.selection_color, p.selected);
            assert_eq!(cursor.unfocused_selection_color, p.header);
            assert_eq!(cursor.selected_text_color, None);
        }
        app.world_mut()
            .entity_mut(marker_first)
            .insert(InteractionDisabled);
        assert_eq!(
            app.world().get::<TextColor>(marker_first).unwrap().0,
            p.disabled_text
        );
        app.world_mut()
            .entity_mut(marker_first)
            .remove::<InteractionDisabled>();
        assert_eq!(
            app.world().get::<TextColor>(marker_first).unwrap().0,
            p.text
        );
    }
}

#[test]
fn menu_arrow_images_follow_theme_and_disabled_state_without_touching_other_images() {
    let mut app = theme_app(ThemeId::WarmPaper);
    app.update();
    app.update();
    let (button, _) = themed_button(&mut app, ButtonVariant::Normal);
    app.world_mut()
        .entity_mut(button)
        .insert(FeathersMenuButton);
    let arrow = app
        .world_mut()
        .spawn((ImageNode::default(), ChildOf(button)))
        .id();
    let wrapper = app
        .world_mut()
        .spawn((Node::default(), ChildOf(button)))
        .id();
    let nested_image = app
        .world_mut()
        .spawn((ImageNode::default(), ChildOf(wrapper)))
        .id();
    let unrelated = app.world_mut().spawn(ImageNode::default()).id();
    app.update();
    assert_eq!(
        app.world().get::<ImageNode>(arrow).unwrap().color,
        palette::colors(ThemeId::WarmPaper).text
    );
    let changed = app
        .world()
        .get_entity(arrow)
        .unwrap()
        .get_ref::<ImageNode>()
        .unwrap()
        .last_changed();
    app.update();
    assert_eq!(
        app.world()
            .get_entity(arrow)
            .unwrap()
            .get_ref::<ImageNode>()
            .unwrap()
            .last_changed(),
        changed
    );

    for id in ThemeId::ALL {
        app.world_mut().resource_mut::<QueuedTheme>().0 = Some(id);
        app.world_mut()
            .entity_mut(button)
            .insert(InteractionDisabled);
        app.update();
        let p = palette::colors(id);
        assert_eq!(
            app.world().get::<ImageNode>(arrow).unwrap().color,
            p.disabled_text
        );
        app.world_mut()
            .entity_mut(button)
            .remove::<InteractionDisabled>();
        app.update();
        assert_eq!(app.world().get::<ImageNode>(arrow).unwrap().color, p.text);
        assert_eq!(
            app.world().get::<ImageNode>(nested_image).unwrap().color,
            Color::WHITE
        );
        assert_eq!(
            app.world().get::<ImageNode>(unrelated).unwrap().color,
            Color::WHITE
        );
    }
    // 主题和按钮都已稳定后才挂上图像，模拟 BSN 的跨帧子场景。
    let late_arrow = app
        .world_mut()
        .spawn((ImageNode::default(), ChildOf(button)))
        .id();
    app.update();
    assert_eq!(
        app.world().get::<ImageNode>(late_arrow).unwrap().color,
        palette::colors(ThemeId::MistIndigo).text
    );
}

#[test]
fn actual_bsn_theme_menu_labels_inherit_all_four_palettes_through_caption_wrappers() {
    let mut app = theme_app(ThemeId::WarmPaper);
    app.add_plugins(super::super::theme_picker::ThemePickerPlugin);
    app.update();
    app.world_mut()
        .commands()
        .spawn_empty()
        .queue_spawn_related_scenes::<Children>(bsn_list![
            super::super::theme_picker::theme_picker()
        ]);
    app.world_mut().flush();
    for _ in 0..4 {
        app.update();
    }
    let mut query = app.world_mut().query::<(Entity, &Text, &TextColor)>();
    let labels: Vec<_> = query
        .iter(app.world())
        .filter(|(_, text, _)| ThemeId::ALL.into_iter().any(|id| text.0 == id.label()))
        .map(|(entity, text, color)| (entity, text.0.clone(), color.0))
        .collect();
    assert_eq!(labels.len(), 4);
    assert!(
        labels
            .iter()
            .all(|(_, _, color)| *color == palette::colors(ThemeId::WarmPaper).text)
    );

    for id in ThemeId::ALL {
        app.world_mut().resource_mut::<QueuedTheme>().0 = Some(id);
        app.update();
        let world = app.world();
        let expected = world.resource::<UiTheme>().color(&tokens::MENUITEM_TEXT);
        for (entity, label, _) in &labels {
            assert_eq!(&world.get::<Text>(*entity).unwrap().0, label);
            assert_eq!(world.get::<TextColor>(*entity).unwrap().0, expected);
            assert!(
                world
                    .resource::<ColorsAtContent>()
                    .0
                    .contains(&(*entity, expected))
            );
        }
    }
}

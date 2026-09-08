//! 参数树重建与旧控件写入的无窗口生命周期回归。

use super::*;
use bevy::input_focus::InputFocus;
use bevy::scene::ScenePlugin;
use bevy::text::{FontCx, LayoutCx};

fn fixture(value: f64) -> TaskGraphData {
    let mut pose = RobotPose::default();
    pose.chassis_pose.position.x = value;
    crate::model::parse_task_graph(
        &serde_json::json!({
            "map_id":"m", "task_id":"t", "config":{"context":{
                "home_pose": serde_json::to_string(&pose).unwrap(),
                "matrix": [[value, value + 1.0]],
                "speed": value,
            }}
        })
        .to_string(),
    )
    .unwrap()
}

fn field_path(data: &TaskGraphData, key: &str) -> Vec<usize> {
    vec![
        data.context_fields
            .iter()
            .position(|field| field.key == key)
            .unwrap(),
    ]
}

fn isolated_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default(), ScenePlugin))
        .init_asset::<Font>()
        .init_resource::<InputFocus>()
        .init_resource::<FontCx>()
        .init_resource::<LayoutCx>()
        .init_resource::<bevy::clipboard::Clipboard>()
        .insert_resource(Session::new(
            crate::model::LoginConfig::default(),
            Vec::new(),
        ))
        .init_resource::<Editor>()
        .init_resource::<StatusLine>()
        .add_message::<AppAction>()
        .configure_sets(
            Update,
            (UiSet::Input, UiSet::Update, UiSet::Rebuild).chain(),
        )
        .add_plugins((EditorPanelPlugin, widgets::WidgetsPlugin))
        .add_systems(PostUpdate, bevy::text::apply_text_edits);
    let font = Font::from_bytes(
        include_bytes!("../../../assets/fonts/SarasaTermSCNerd-Regular.ttf").to_vec(),
    );
    let mut fonts = app.world_mut().resource_mut::<FontCx>();
    let registered = fonts.collection.register_fonts(font.data, None);
    let family = fonts
        .collection
        .family_name(registered[0].0)
        .unwrap()
        .to_string();
    fonts.set_sans_serif_family(&family).unwrap();
    app
}

fn settle(app: &mut App) {
    for _ in 0..6 {
        app.update();
    }
}

fn displayed_value(app: &App, field: Entity) -> f64 {
    app.world()
        .get::<Children>(field)
        .unwrap()
        .iter()
        .find_map(|child| app.world().get::<EditableText>(child))
        .unwrap()
        .value()
        .to_string()
        .parse::<f64>()
        .unwrap()
}

#[test]
fn 重建先销毁旧卡片和未初始化数值再初始化新树选中与完整数值() {
    let mut app = isolated_app();
    let root = app.world_mut().spawn(EditorSlot).id();
    let old_card = app
        .world_mut()
        .spawn((ChildOf(root), PoseCard::default()))
        .id();
    let old_input = app
        .world_mut()
        .spawn_scene(widgets::number_field(
            -10.0,
            None,
            ValueBinding::new(vec![0], ValueSlot::Scalar),
        ))
        .unwrap()
        .id();
    app.world_mut().entity_mut(old_input).insert(ChildOf(root));

    let value = 0.9216510910864573;
    let data = fixture(value);
    let target = PoseTarget::field(field_path(&data, "home_pose"));
    let speed_path = field_path(&data, "speed");
    {
        let mut editor = app.world_mut().resource_mut::<Editor>();
        editor.load(Some(data));
        editor.selected_pose = Some(target.clone());
    }
    settle(&mut app);
    assert!(app.world().get_entity(old_card).is_err());
    assert!(app.world().get_entity(old_input).is_err());
    let cards: Vec<_> = app
        .world_mut()
        .query::<(&PoseCard, &ThemeBackgroundColor, &ThemeBorderColor)>()
        .iter(app.world())
        .map(|(card, bg, border)| (card.target.clone(), bg.0.clone(), border.0.clone()))
        .collect();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].0, target);
    assert_eq!(
        (cards[0].1.clone(), cards[0].2.clone()),
        pose_card_tokens(true)
    );
    let field = app
        .world_mut()
        .query::<(Entity, &ValueBinding)>()
        .iter(app.world())
        .find(|(_, binding)| {
            binding.field_path == speed_path && matches!(binding.slot, ValueSlot::Scalar)
        })
        .unwrap()
        .0;
    assert_eq!(displayed_value(&app, field).to_bits(), value.to_bits());
    assert!(app.world().get::<NumberFieldInit>(field).is_none());
}

#[test]
fn 重建不填充旧懒加载区块且新树展开只生成当前行数值一次() {
    let mut app = isolated_app();
    let root = app.world_mut().spawn(EditorSlot).id();
    let section = app
        .world_mut()
        .spawn((ChildOf(root), Collapsible { open: true }))
        .id();
    let data = fixture(12.25);
    let path = field_path(&data, "matrix");
    let old_body = app
        .world_mut()
        .spawn((
            ChildOf(section),
            LazyBody {
                path: path.clone(),
                kind: LazyKind::Array2DRow(0),
            },
        ))
        .id();
    app.world_mut().resource_mut::<Editor>().load(Some(data));
    settle(&mut app);
    assert!(app.world().get_entity(section).is_err());
    assert!(app.world().get_entity(old_body).is_err());

    let (new_body, parent) = app
        .world_mut()
        .query::<(Entity, &LazyBody, &ChildOf)>()
        .iter(app.world())
        .find(|(_, body, _)| body.path == path && body.kind == LazyKind::Array2DRow(0))
        .map(|(entity, _, parent)| (entity, parent.parent()))
        .unwrap();
    app.world_mut().get_mut::<Collapsible>(parent).unwrap().open = true;
    settle(&mut app);
    assert!(app.world().get::<LazyFilled>(new_body).is_some());
    let values: Vec<_> = app
        .world_mut()
        .query::<(Entity, &ValueBinding)>()
        .iter(app.world())
        .filter(|(_, binding)| binding.field_path == path)
        .map(|(entity, _)| entity)
        .collect();
    assert_eq!(values.len(), 2);
    let mut displayed: Vec<_> = values
        .iter()
        .map(|entity| displayed_value(&app, *entity))
        .collect();
    displayed.sort_by(f64::total_cmp);
    assert_eq!(displayed, vec![12.25, 13.25]);
    settle(&mut app);
    for entity in values {
        assert!(app.world().get::<ValueBinding>(entity).is_some());
    }
    assert_eq!(
        app.world_mut()
            .query::<&ValueBinding>()
            .iter(app.world())
            .filter(|binding| binding.field_path == path)
            .count(),
        2
    );
}

# 四套界面主题

四套主题已正式接入应用，沿用左侧 SSH / 文件选择、右侧参数 / 流程图 / 编辑操作的布局。
[assets/themes/palettes.json](../../assets/themes/palettes.json) 是唯一色板来源，编译时嵌入；
[src/ui/theme.rs](../../src/ui/theme.rs) 将颜色映射到界面 token。

默认主题为 **石墨青**。右上角「选择主题」可立即切换四套配色，无需连接 SSH。
换色只刷新现有颜色组件，不重建控件，不因主题刷新重置焦点、输入光标或滚动位置。

实际选择变化后会保存到 `~/.config/task-graph-editor/theme.json`，下次启动自动恢复。
启动加载本身不写入；未知主题回退石墨青并保留原值，损坏或非对象 JSON 不会被覆盖。
保存采用同目录临时文件原子替换，并保留未知字段；失败后等待下一次实际切换再重试。

实现分工：`theme/palette.rs` 解析并校验色板，`theme/refresh.rs` 补齐派生颜色刷新，
`theme_picker.rs` 管理固定四项菜单，`theme_preferences.rs` 管理独立偏好文件。
离线 UI 测试和截图夹具在插件装配前注入 `ThemePreferencesStore::disabled()`，
持久化测试用 `ThemePreferencesStore::at_path(...)` 指向隔离临时目录。

## 四套方案

| id | 名称 | 模式 | 气质与使用场景 | 语义区分 |
|---|---|---|---|---|
| `graphite-teal` | 石墨青 | dark | 默认主题，中性石墨灰与青绿，适合日常参数编辑。 | 青绿主操作与文件选择；淡紫位姿选择；琥珀警告。成功使用偏黄的绿色，并保留文字说明。 |
| `dusk-sand` | 暮砂金 | dark | 深暖灰与低饱和砂金，适合偏好温暖深色的参数编辑，和石墨青及两套浅色形成独立选择。 | 砂金主操作与文件选择；蓝灰位姿选择；高饱和珊瑚橙警告。警告同时保留文字或图标。 |
| `warm-paper` | 暖纸橙 | light | 米白纸面与烧橙，适合明亮环境中的长文本、文件与参数校对。 | 烧橙主操作；青绿色位姿选择；深金色警告。主色与警告同属暖色，警告必须同时保留文字或图标。 |
| `mist-indigo` | 雾白靛 | light | 冷白与雾灰配靛蓝，适合明亮环境，也便于比较参数和流程图两种视图。 | 靛蓝主操作；蓝绿色位姿选择；深金色警告。普通边框保持灰蓝，避免把所有容器都染成选中态。 |

这些场景描述是设计意图；舒适度还受屏幕、环境光和个人偏好影响。

## 数据约定

[palettes.json](palettes.json) 是指向 `../../assets/themes/palettes.json` 的相对符号链接，不维护副本。
JSON 顶层为数组，每项包含 `id`、`name`、`mode`、`note`、`colors`。四项的 `colors` 均使用相同的 36 个键；颜色为不带透明度的 `#RRGGBB` sRGB 值，`mode` 为 `dark` 或 `light`。

`accent` 用于实色主按钮，配 `on_accent`；`selected` 是带主色倾向的柔和底色，配 `selected_text`。位姿卡片单独使用 `pose_*`，警告文字使用 `warning`，不再复用一套高亮色。

左下“复制提示”按钮单独使用 `copy_bg`、`copy_hover`、`copy_text`：普通和悬停底色均为不透明实色，文字不继承状态消息的成功、警告或错误颜色。复制反馈保留这组颜色，只更新按钮文案。

卡片边框、分组边框和栏间分隔采用 `border` 42% + `panel` 58% 的 sRGB 混合色，让容器退后、控件边界更清楚。输入框与流程图连线仍使用原始 `border`；下文对比度表中的边框数值也指原始色。

## 与界面 token 的对应关系

下表记录实际角色映射；大写标识可在 `src/ui/theme.rs` 中找到。

| 色板键 | 界面角色与对应 token |
|---|---|
| `canvas` | 主体底色：`WINDOW_BG`、`CONTENT_BG`。 |
| `shell` | 顶栏、侧栏、状态栏：`APPBAR_BG`、`SIDEBAR_BG`、`STATUSBAR_BG`。 |
| `panel` | 卡片与菜单内容：`CARD_BG`、`PANE_BODY_BG`、`GROUP_BODY_BG`、`SUBPANE_BODY_BG`、`MENU_BG`。 |
| `header` | 卡片、折叠分组标题：`CARD_HEADER_BG`、`GROUP_HEADER_BG`、`PANE_HEADER_BG`、`SUBPANE_HEADER_BG`。 |
| `input` | 输入控件底色：`TEXT_INPUT_BG`、`CHECKBOX_BG`、`SLIDER_BG`、`SWITCH_BG`。 |
| `button` | 普通按钮与徽章底色：`BUTTON_BG`、`BADGE_BG`。 |
| `hover` | 普通交互悬停底色：`BUTTON_BG_HOVER`、`BUTTON_PLAIN_BG_HOVER`、`MENUITEM_BG_HOVER`、`LISTROW_BG_HOVER`。 |
| `copy_bg`、`copy_hover`、`copy_text` | 复制提示按钮的 `COPY_BG`、`COPY_BG_HOVER`、`COPY_TEXT`，由 `StatusCopyButton` 局部样式使用，不全局替换普通按钮颜色。 |
| `border` | 输入框、菜单、控件与图边：`INPUT_BORDER`、`MENU_BORDER`、`CHECKBOX_BORDER`、`GRAPH_BORDER`。卡片和分隔线使用柔化后的派生色。 |
| `text` | 正文及普通按钮文字：`TEXT_MAIN`、`BUTTON_TEXT`、`TEXT_INPUT_TEXT`、`LISTROW_TEXT`、`CHECKBOX_TEXT`、`SECTION_TEXT`。 |
| `muted` | 标签与次级说明：`TEXT_DIM`、`FIELD_LABEL`、`BADGE_TEXT`。 |
| `faint` | 只读说明与低优先级信息：`READONLY_TEXT`、`DOT_DISCONNECTED`。保留不透明文字。 |
| `accent` | 主操作、光标和焦点：`BUTTON_PRIMARY_BG`、`TEXT_INPUT_CURSOR`、`FOCUS_RING`、`CHECKBOX_BG_CHECKED`、`SWITCH_BG_CHECKED`。 |
| `accent_hover` | 主操作悬停：`BUTTON_PRIMARY_BG_HOVER`、`CHECKBOX_BG_CHECKED_HOVER`、`SWITCH_BG_CHECKED_HOVER`。 |
| `on_accent` | 实色按钮上的文字/标记：`BUTTON_PRIMARY_TEXT`、`CHECKBOX_MARK`、`SWITCH_SLIDE_BG_CHECKED`、`DANGER_TEXT`。 |
| `selected` | 文件行、文本选区及图节点选择：`LISTROW_BG_SELECTED`、`TEXT_INPUT_SELECTION`、`GRAPH_SELECTED_BG`；普通按钮按下态、菜单按下/聚焦态和滑块填充也采用此底色。 |
| `selected_text` | 流程图选中节点文字：`GRAPH_SELECTED_TEXT`。文件列表的 `LISTROW_TEXT` 由普通行和选中行共用 `text`，两者在 `selected` 上均已计算；参数 / 流程图主按钮用 `accent` + `on_accent`。 |
| `success` | 成功状态与已连接指示：`STATUS_OK`、`DOT_CONNECTED`。 |
| `warning` | 警告、重连提示与检查点：`STATUS_WARN`、`DOT_RECONNECTING`、`CHECKPOINT_DOT`。 |
| `danger` | 错误文字与危险操作底色：`STATUS_ERROR`、`DANGER_BG`。危险按钮用 `on_accent` 作为文字色。 |
| `disabled_bg`、`disabled_text` | 对应各控件的 `*_BG_DISABLED`、`*_TEXT_DISABLED`。禁用仍由控件状态控制。 |
| `pose_bg`、`pose_border`、`pose_text` | 分别对应 `POSE_SELECTED_BG`、`POSE_SELECTED_BORDER`、`POSE_SELECTED_TEXT`，与主操作和警告颜色分开。 |
| `flow`、`act`、`data`、`log`、`domain`、`back` | 分别对应 `GRAPH_FLOW`、`GRAPH_ACT`、`GRAPH_DATA`、`GRAPH_LOG`、`GRAPH_DOMAIN`、`GRAPH_BACK`。用于类别文字、细边、连线和图例，节点本体保留 `panel`。 |
| `axis_x`、`axis_y`、`axis_z` | 分别对应 `TEXT_INPUT_X_AXIS`、`TEXT_INPUT_Y_AXIS`、`TEXT_INPUT_Z_AXIS`，保持 X 红 / Y 绿 / Z 蓝及字母标识。 |

修改色板时保留以下配对关系：

- 普通按钮、菜单按下态和滑块填充使用 `selected`，保留普通 `text`。只有提供独立 `on_accent` 的控件才使用实色主色。
- 主按钮、复选框勾选标记和危险按钮均使用 `on_accent`，深浅主题分别采用各自验证过的文字与底色配对。
- `selected` 和 `pose_bg` 已是不透明的目标色，不额外叠加透明度；若改变透明度，须针对合成后的实际底色重算。

警告、成功、错误、流程类别和坐标轴仍需同时保留文字、形状或字母，不仅依赖色相。`faint` 用于静态说明；悬停按钮与列表行中的文字使用 `text` 或 `muted`，复制按钮始终使用专用 `copy_text`。

## 实际对比度

计算直接使用 JSON 中的最终十六进制值。先把各通道除以 255 得到 sRGB 通道值 `c`；当 `c <= 0.04045` 时线性值为 `c / 12.92`，否则为 `((c + 0.055) / 1.055)^2.4`。相对亮度 `L = 0.2126R + 0.7152G + 0.0722B`；对比度为 `(L亮 + 0.05) / (L暗 + 0.05)`。

以下显示值四舍五入到两位小数，单位为 `:1`；是否达到阈值使用未舍入值判断。指定的正文、次级文字与弱化文字在三种常用底色上全部达到 4.5。

| 方案 | 背景 | text | muted | faint |
|---|---|---:|---:|---:|
| 石墨青 | `panel` | 12.15 | 8.06 | 7.24 |
| 石墨青 | `input` | 14.72 | 9.76 | 8.76 |
| 石墨青 | `shell` | 13.86 | 9.19 | 8.25 |
| 暮砂金 | `panel` | 11.55 | 7.26 | 6.24 |
| 暮砂金 | `input` | 13.73 | 8.63 | 7.42 |
| 暮砂金 | `shell` | 13.07 | 8.22 | 7.07 |
| 暖纸橙 | `panel` | 13.21 | 6.47 | 5.42 |
| 暖纸橙 | `input` | 13.80 | 6.76 | 5.66 |
| 暖纸橙 | `shell` | 11.50 | 5.63 | 4.72 |
| 雾白靛 | `panel` | 14.27 | 6.01 | 5.04 |
| 雾白靛 | `input` | 14.77 | 6.22 | 5.21 |
| 雾白靛 | `shell` | 13.04 | 5.49 | 4.60 |

| 方案 | on_accent / accent | on_accent / accent_hover | selected_text / selected | pose_text / pose_bg |
|---|---:|---:|---:|---:|
| 石墨青 | 8.54 | 10.28 | 8.96 | 9.23 |
| 暮砂金 | 7.27 | 9.19 | 7.26 | 9.04 |
| 暖纸橙 | 6.03 | 7.61 | 8.20 | 7.06 |
| 雾白靛 | 7.90 | 9.93 | 9.36 | 6.99 |

复制按钮普通底色和悬停底色对状态栏 `shell` 都达到 2，专用文字对两种底色都达到 4.5；底色的区别使用实际不透明颜色实现。

| 方案 | copy_bg / shell | copy_hover / shell | copy_text / copy_bg | copy_text / copy_hover |
|---|---:|---:|---:|---:|
| 石墨青 | 2.59 | 2.96 | 5.83 | 5.11 |
| 暮砂金 | 2.42 | 3.02 | 6.10 | 4.87 |
| 暖纸橙 | 4.84 | 6.70 | 5.64 | 7.80 |
| 雾白靛 | 4.99 | 6.44 | 5.65 | 7.29 |

补充检查也采用最终色值：语义列取 `success` / `warning` / `danger` 在 `panel` 上的最小值，流程列取六种流程色在 `panel` 上的最小值，轴色列取三轴在 `input` 上的最小值。

| 方案 | text / selected | 三种语义色最小值 | 六种流程色最小值 | 三轴色最小值 | on_accent / danger |
|---|---:|---:|---:|---:|---:|
| 石墨青 | 9.14 | 7.18 | 7.12 | 7.96 | 8.04 |
| 暮砂金 | 8.14 | 6.98 | 7.09 | 8.40 | 7.93 |
| 暖纸橙 | 10.95 | 6.14 | 5.17 | 5.29 | 6.51 |
| 雾白靛 | 12.10 | 6.29 | 4.73 | 5.37 | 6.51 |

`border` 对 `input` 的对比度依次为 **3.73 / 3.92 / 3.76 / 3.55**，对 `panel` 为 **3.08 / 3.30 / 3.60 / 3.44**。这只是已列配对的数值验证，不能替代完整界面的可访问性与视觉验证。

边界也记录在这里：暮砂金、暖纸橙、雾白靛的 `faint / hover` 分别为 **3.93 / 3.86 / 3.88**，因此悬停行不要用 `faint` 作为文字色。四套方案按表中顺序的 `disabled_text / disabled_bg` 分别为 **4.68 / 4.49 / 4.20 / 3.73**，禁用态没有计入上述“全部达到 4.5”的结论。

## 复算方式

在仓库根目录运行以下只读 Python 代码，无需额外依赖。它验证四套键集合、十六进制格式和本次要求的全部对比度；输出每套正文组最小值、六组文字配对，以及复制按钮底色对状态栏的最小值。

```bash
python3 - <<'PY'
import json, re
from pathlib import Path

palettes = json.loads(Path('assets/themes/palettes.json').read_text())
keys = set('canvas shell panel header input button hover border text muted faint accent accent_hover on_accent selected selected_text success warning danger disabled_bg disabled_text pose_bg pose_border pose_text flow act data log domain back axis_x axis_y axis_z copy_bg copy_hover copy_text'.split())

def luminance(color):
    channels = [int(color[i:i+2], 16) / 255 for i in (1, 3, 5)]
    linear = [c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4 for c in channels]
    return sum(c * weight for c, weight in zip(linear, (0.2126, 0.7152, 0.0722)))

def contrast(first, second):
    low, high = sorted((luminance(first), luminance(second)))
    return (high + 0.05) / (low + 0.05)

assert len(palettes) == 4
assert len({p['id'] for p in palettes}) == 4
for palette in palettes:
    colors = palette['colors']
    assert set(colors) == keys
    assert all(re.fullmatch(r'#[0-9A-F]{6}', value) for value in colors.values())
    basic = [contrast(colors[text], colors[bg]) for text in ('text', 'muted', 'faint') for bg in ('panel', 'input', 'shell')]
    pairs = [('on_accent', 'accent'), ('on_accent', 'accent_hover'), ('selected_text', 'selected'), ('pose_text', 'pose_bg'), ('copy_text', 'copy_bg'), ('copy_text', 'copy_hover')]
    ratios = [contrast(colors[first], colors[second]) for first, second in pairs]
    assert min(basic + ratios) >= 4.5, palette['name']
    copy_surfaces = [contrast(colors[key], colors['shell']) for key in ('copy_bg', 'copy_hover')]
    assert min(copy_surfaces) >= 2, palette['name']
    print(palette['name'], '正文组最小值', f'{min(basic):.2f}', '关键配对', ', '.join(f'{value:.2f}' for value in ratios), '复制底色最小值', f'{min(copy_surfaces):.2f}')
PY
```

本目录维护设计说明、对比度记录和色板链接。调整颜色时只编辑资源目录中的权威 JSON，重新计算本页数值，并检查四套主题的实际界面。

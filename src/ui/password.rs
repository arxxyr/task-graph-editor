//! 密码输入控件
//!
//! Bevy 0.19 的 `EditableText` 明确未实现密码遮罩（官方留给使用方自行处理），
//! 这里在 Feathers 文本框之上加一层：真实密码存在 [`PasswordInput`] 里，
//! 文本框内只放等长的遮罩字符。
//!
//! 因为遮罩串与真实串**字符数始终相同**，任何一次编辑在遮罩串上产生的差异区间
//! 就精确对应真实串上要替换的区间，据此把编辑同步回真实值，再把文本框重置为
//! 新长度的遮罩并还原光标位置。删除、选区替换、粘贴都走同一条路径。

use bevy::feathers::controls::FeathersTextInput;
use bevy::prelude::*;
use bevy::text::{EditableText, TextEdit, TextEditChange};

/// 遮罩字符
const MASK_CHAR: char = '•';

/// 密码输入状态：挂在 Feathers 文本框实体上
#[derive(Component, Default, Clone)]
pub struct PasswordInput {
    /// 真实密码
    pub real: String,
}

impl PasswordInput {
    /// 用初始密码构造
    pub fn new(initial: impl Into<String>) -> Self {
        Self {
            real: initial.into(),
        }
    }

    /// 当前真实值对应的遮罩串
    fn mask(&self) -> String {
        MASK_CHAR.to_string().repeat(self.real.chars().count())
    }
}

/// 一次编辑在真实串上的效果：`[start, end)` 区间被替换为 `inserted`
#[derive(Debug, PartialEq, Eq)]
struct Diff {
    /// 替换区间起点（字符下标）
    start: usize,
    /// 替换区间终点（字符下标，不含）
    end: usize,
    /// 新插入的内容
    inserted: String,
}

/// 依据编辑后的文本与光标位置，还原这次编辑在真实串上做了什么
///
/// 不能靠内容比对：遮罩串每个字符都一样，删掉中间任意一个得到的结果完全相同，
/// 前后缀匹配只会一律判成"删末尾"。光标位置才是无歧义的信息源：
///
/// - 编辑后光标停在插入内容的**右端**，所以从光标向左连续的非遮罩字符就是本次插入的内容；
/// - 剩下的长度差 `插入数 - 长度变化` 就是被替换（删除）掉的字符数；
/// - 退格与 Delete 键删完后光标都落在删除区间的起点，两种情况天然统一。
///
/// 参数 `real_len` 是编辑前真实串的字符数，`cur` 是编辑后的文本框内容，
/// `cursor` 是编辑后光标所在的字符位置。
fn resolve_edit(real_len: usize, cur: &[char], cursor: usize) -> Diff {
    let cursor = cursor.min(cur.len());

    // 光标左侧连续的非遮罩字符即本次插入的内容
    let mut inserted_len = 0;
    while inserted_len < cursor && cur[cursor - inserted_len - 1] != MASK_CHAR {
        inserted_len += 1;
    }

    // 长度变化 = 插入数 - 删除数，据此反推删除了多少
    let delta = cur.len() as isize - real_len as isize;
    let replaced = (inserted_len as isize - delta).max(0) as usize;

    let start = cursor - inserted_len;
    Diff {
        start,
        end: (start + replaced).min(real_len),
        inserted: cur[start..cursor].iter().collect(),
    }
}

/// 把一次编辑应用到真实密码串上，返回编辑后光标应处的字符位置
fn apply_diff(real: &mut String, diff: &Diff) -> usize {
    let mut chars: Vec<char> = real.chars().collect();
    // 区间可能因为控件的 max_characters 限制等原因越界，这里夹紧后再替换
    let start = diff.start.min(chars.len());
    let end = diff.end.clamp(start, chars.len());
    chars.splice(start..end, diff.inserted.chars());
    *real = chars.into_iter().collect();
    start + diff.inserted.chars().count()
}

/// 读取文本框中光标所在的字符位置
fn cursor_char_index(editable: &EditableText) -> usize {
    let text = editable.editor().raw_text();
    let byte_index = editable.editor().raw_selection().focus().index();
    // parley 给的是字节偏移，遮罩字符占 3 字节，必须换算成字符下标
    text.get(..byte_index)
        .map(|prefix| prefix.chars().count())
        .unwrap_or_else(|| text.chars().count())
}

/// 文本框内容变化时，把编辑同步回真实密码并重置遮罩
///
/// 挂在密码框实体上（见 [`crate::ui::widgets::password_field`]）。
pub fn on_password_edit(
    change: On<TextEditChange>,
    mut inputs: Query<(&mut PasswordInput, &mut EditableText)>,
) {
    let Ok((mut state, mut editable)) = inputs.get_mut(change.event_target()) else {
        return;
    };

    let current: String = editable.value().to_string();
    // 内容已经是当前真实值的遮罩，说明是光标移动或本函数自己的重置，无需处理
    if current == state.mask() {
        return;
    }

    let cur: Vec<char> = current.chars().collect();
    let cursor = cursor_char_index(&editable);
    let diff = resolve_edit(state.real.chars().count(), &cur, cursor);
    let new_cursor = apply_diff(&mut state.real, &diff);

    // 重置为新长度的遮罩：全选后整体替换，光标落在末尾
    let mask = state.mask();
    let mask_len = mask.chars().count();
    editable.queue_edit(TextEdit::SelectAll);
    editable.queue_edit(TextEdit::Insert(mask.into()));
    // 从末尾左移回到编辑位置
    for _ in new_cursor..mask_len {
        editable.queue_edit(TextEdit::Left(false));
    }
}

/// 新建密码框的查询：刚挂上 [`PasswordInput`] 的文本框
type NewPasswordInputs<'w, 's> = Query<
    'w,
    's,
    (&'static PasswordInput, &'static mut EditableText),
    (Added<PasswordInput>, With<FeathersTextInput>),
>;

/// 新建的密码框：按初始真实值填入遮罩
fn init_password_mask(mut inputs: NewPasswordInputs) {
    for (state, mut editable) in &mut inputs {
        if state.real.is_empty() {
            continue;
        }
        editable.queue_edit(TextEdit::SelectAll);
        editable.queue_edit(TextEdit::Insert(state.mask().into()));
    }
}

/// 密码控件插件
pub struct PasswordInputPlugin;

impl Plugin for PasswordInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, init_password_mask);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟一次编辑：给出编辑后的文本框内容与光标字符位置，返回新的真实值与光标
    fn edit(real: &str, after: &str, cursor: usize) -> (String, usize) {
        let mut state = PasswordInput::new(real);
        let cur: Vec<char> = after.chars().collect();
        let diff = resolve_edit(state.real.chars().count(), &cur, cursor);
        let new_cursor = apply_diff(&mut state.real, &diff);
        (state.real, new_cursor)
    }

    /// 构造 n 个遮罩字符
    fn mask(n: usize) -> String {
        MASK_CHAR.to_string().repeat(n)
    }

    #[test]
    fn 末尾追加字符() {
        // "abc" → 文本框 "•••X"，光标在 4
        let (real, cursor) = edit("abc", &format!("{}X", mask(3)), 4);
        assert_eq!(real, "abcX");
        assert_eq!(cursor, 4);
    }

    #[test]
    fn 中间插入字符() {
        // "abc" → 在下标 1 插入 Z，文本框 "•Z••"，光标在 2
        let (real, cursor) = edit("abc", &format!("{}Z{}", mask(1), mask(2)), 2);
        assert_eq!(real, "aZbc");
        assert_eq!(cursor, 2);
    }

    #[test]
    fn 退格删除末字符() {
        // "abc" → 退格，文本框 "••"，光标在 2
        let (real, cursor) = edit("abc", &mask(2), 2);
        assert_eq!(real, "ab");
        assert_eq!(cursor, 2);
    }

    #[test]
    fn 退格删除中间字符() {
        // "abcd" 光标在 2 处退格：删掉下标 1，文本框 "•••"，光标落到 1
        let (real, cursor) = edit("abcd", &mask(3), 1);
        assert_eq!(real, "acd");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn delete键删除中间字符() {
        // "abcd" 光标在 1 处按 Delete：删掉下标 1，光标不动仍在 1
        let (real, cursor) = edit("abcd", &mask(3), 1);
        assert_eq!(real, "acd");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn 删除首字符() {
        let (real, cursor) = edit("abc", &mask(2), 0);
        assert_eq!(real, "bc");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn 全选替换() {
        let (real, cursor) = edit("oldpass", "NEW", 3);
        assert_eq!(real, "NEW");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn 全部清空() {
        let (real, cursor) = edit("abc", "", 0);
        assert_eq!(real, "");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn 从空开始输入() {
        let (real, cursor) = edit("", "p", 1);
        assert_eq!(real, "p");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn 选区替换为多字符_粘贴() {
        // "abcdef" 选中 1..4（bcd）粘贴 "XY" → 文本框 "•XY••"，光标在 3
        let (real, cursor) = edit("abcdef", &format!("{}XY{}", mask(1), mask(2)), 3);
        assert_eq!(real, "aXYef");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn 一次粘贴多个字符到末尾() {
        let (real, cursor) = edit("ab", &format!("{}XYZ", mask(2)), 5);
        assert_eq!(real, "abXYZ");
        assert_eq!(cursor, 5);
    }

    #[test]
    fn 选区删除不插入() {
        // "abcdef" 选中 2..5（cde）按删除 → 文本框 "•••"，光标在 2
        let (real, cursor) = edit("abcdef", &mask(3), 2);
        assert_eq!(real, "abf");
        assert_eq!(cursor, 2);
    }

    #[test]
    fn 遮罩长度始终等于真实长度() {
        let state = PasswordInput::new("密码pass123");
        assert_eq!(state.mask().chars().count(), "密码pass123".chars().count());
    }

    #[test]
    fn 多字节真实值按字符定位() {
        // 真实值含中文，遮罩仍是等量的 •；删掉首字符应得到 "码abc"
        let (real, cursor) = edit("密码abc", &mask(4), 0);
        assert_eq!(real, "码abc");
        assert_eq!(cursor, 0);
    }

    #[test]
    fn 输入多字节字符() {
        // "ab" 末尾输入一个中文字符
        let (real, cursor) = edit("ab", &format!("{}中", mask(2)), 3);
        assert_eq!(real, "ab中");
        assert_eq!(cursor, 3);
    }

    #[test]
    fn 光标越界时不panic() {
        let (real, _) = edit("abc", &mask(3), 99);
        assert_eq!(real, "abc");
    }
}

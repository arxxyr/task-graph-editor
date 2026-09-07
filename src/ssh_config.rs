//! 本地 OpenSSH 客户端配置（`~/.ssh/config`）解析
//!
//! 实现 ssh_config(5) 中与"选主机"相关的子集：
//! - `Host` / `Match` 块（`Match` 支持 `all` / `canonical` / `final` / `host` / `originalhost`
//!   条件及 `!` 取反，其余需要运行期信息的条件一律视为不匹配）
//! - `Include`（支持 `~`、相对 `~/.ssh` 的路径与 `*` / `?` 通配，最大嵌套 16 层）
//! - `HostName` / `Port` / `User` / `IdentityFile` / `ProxyJump` / `ProxyCommand`
//! - "首次出现的值生效"规则（`IdentityFile` 例外：多次出现按顺序累加）
//! - `%h` / `%d` 等 token 与 `~` 展开
//!
//! 解析本身只依赖标准库且不读环境变量，便于单元测试；
//! [`load_user_hosts`] 是唯一触碰真实 `~/.ssh/config` 的入口。

use std::path::{Component, Path, PathBuf};

/// Include 最大嵌套深度（与 OpenSSH `MAX_INCLUDE_DEPTH` 一致）
const MAX_INCLUDE_DEPTH: usize = 16;

/// OpenSSH 默认尝试的私钥文件名（保持 OpenSSH 顺序；剔除 libssh2 不支持的 FIDO/XMSS/DSA 类型）
const DEFAULT_IDENTITY_FILES: [&str; 3] = ["id_rsa", "id_ecdsa", "id_ed25519"];

/// SSH 默认端口
const DEFAULT_PORT: u16 = 22;

/// 从 ssh config 解析出的一台主机（`Host` 别名及其生效配置）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHostEntry {
    /// `Host` 行中的别名（不含通配符）
    pub alias: String,
    /// 实际连接的主机名：`HostName`（已展开 `%h`），缺省为别名本身
    pub host_name: String,
    /// 端口，缺省 22
    pub port: u16,
    /// 登录用户名（`User`），缺省 None（OpenSSH 此时使用本地用户名）
    pub user: Option<String>,
    /// 私钥文件列表（`IdentityFile`，已展开 `~` 与 token，按出现顺序）
    pub identity_files: Vec<PathBuf>,
    /// 跳板机（`ProxyJump`），`none` 视为未设置
    pub proxy_jump: Option<String>,
    /// 代理命令（`ProxyCommand`），`none` 视为未设置
    pub proxy_command: Option<String>,
}

/// 解析环境：决定 Include 相对路径与 `~` / `%d` / `%u` 的展开基准
#[derive(Debug, Clone)]
pub struct ParseEnv {
    /// `~/.ssh` 目录（Include 相对路径的基准）
    pub ssh_dir: PathBuf,
    /// 用户主目录（`~` 与 `%d` 展开）
    pub home: PathBuf,
    /// 本地用户名（`%u` 展开），未知时为空串
    pub local_user: String,
}

/// 块头：决定块内指令对哪些别名生效
#[derive(Debug)]
enum BlockHead {
    /// 文件开头尚未进入任何 Host/Match 块的全局指令
    Global,
    /// `Host` 模式列表（原样保留，含 `!` 前缀）
    Host(Vec<String>),
    /// `Match` 条件列表
    Match(Vec<String>),
}

/// 一个配置块：块头 + 其后的指令（小写关键字, 参数列表）
#[derive(Debug)]
struct Block {
    head: BlockHead,
    directives: Vec<(String, Vec<String>)>,
}

impl Block {
    fn new(head: BlockHead) -> Self {
        Self {
            head,
            directives: Vec::new(),
        }
    }

    /// 块是否对给定别名生效
    fn matches(&self, alias: &str) -> bool {
        match &self.head {
            BlockHead::Global => true,
            BlockHead::Host(patterns) => match_pattern_list(alias, patterns),
            BlockHead::Match(criteria) => eval_match(criteria, alias),
        }
    }
}

// ============================================================
// 公共入口
// ============================================================

/// 读取当前用户的 `~/.ssh/config` 并解析出全部主机条目
///
/// 文件不存在、不可读或没有任何 `Host` 条目时返回空列表，不报错。
pub fn load_user_hosts() -> Vec<SshHostEntry> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };
    let ssh_dir = home.join(".ssh");
    let Some(content) = read_lossy(&ssh_dir.join("config")) else {
        return Vec::new();
    };
    let env = ParseEnv {
        ssh_dir,
        home,
        local_user: local_user_name().unwrap_or_default(),
    };
    parse_hosts(&content, &env)
}

/// 解析 ssh config 文本，返回所有具体别名（不含通配符）及其生效配置，按首次出现顺序排列
pub fn parse_hosts(content: &str, env: &ParseEnv) -> Vec<SshHostEntry> {
    let mut blocks = vec![Block::new(BlockHead::Global)];
    parse_into(content, env, 0, &mut blocks);

    collect_aliases(&blocks)
        .into_iter()
        .map(|alias| resolve(&alias, &blocks, env))
        .collect()
}

/// 用户主目录：`HOME`（Unix）或 `USERPROFILE`（Windows）
pub fn home_dir() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(std::env::var_os)
        .find(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// 本地登录用户名：`USER`（Unix）或 `USERNAME`（Windows）
pub fn local_user_name() -> Option<String> {
    ["USER", "USERNAME"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|v| !v.is_empty())
}

/// 展开用户手动输入的路径中的 `~`（无主目录信息时原样返回）
pub fn expand_user_path(raw: &str) -> PathBuf {
    match home_dir() {
        Some(home) => expand_tilde(raw, &home),
        None => PathBuf::from(raw),
    }
}

/// `~/.ssh` 下实际存在的默认私钥文件（按 OpenSSH 尝试顺序）
pub fn default_identity_files() -> Vec<PathBuf> {
    home_dir()
        .map(|home| default_identity_files_in(&home.join(".ssh")))
        .unwrap_or_default()
}

/// 指定目录下实际存在的默认私钥文件
fn default_identity_files_in(ssh_dir: &Path) -> Vec<PathBuf> {
    DEFAULT_IDENTITY_FILES
        .iter()
        .map(|name| ssh_dir.join(name))
        .filter(|path| path.is_file())
        .collect()
}

// ============================================================
// 逐行解析为块
// ============================================================

/// 把配置文本追加解析到 `blocks`（最后一个块即当前块；Include 的内容原地展开）
fn parse_into(content: &str, env: &ParseEnv, depth: usize, blocks: &mut Vec<Block>) {
    for line in content.lines() {
        let Some((keyword, args)) = tokenize_line(line) else {
            continue;
        };
        match keyword.as_str() {
            "host" => blocks.push(Block::new(BlockHead::Host(args))),
            "match" => blocks.push(Block::new(BlockHead::Match(args))),
            "include" => {
                if depth >= MAX_INCLUDE_DEPTH {
                    continue;
                }
                for pattern in &args {
                    for path in include_paths(pattern, env) {
                        if let Some(included) = read_lossy(&path) {
                            parse_into(&included, env, depth + 1, blocks);
                        }
                    }
                }
            }
            _ => {
                if let Some(current) = blocks.last_mut() {
                    current.directives.push((keyword, args));
                }
            }
        }
    }
}

/// 将一行拆为（小写关键字, 参数列表）；空行与注释行返回 None
///
/// 关键字与参数之间允许空白、`=` 或两者混用（`Port 22` / `Port=22` / `Port = 22`）。
fn tokenize_line(line: &str) -> Option<(String, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let keyword_end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let keyword = line[..keyword_end].to_ascii_lowercase();
    let mut rest = line[keyword_end..].trim_start();
    if let Some(after_equals) = rest.strip_prefix('=') {
        rest = after_equals.trim_start();
    }
    Some((keyword, split_args(rest)))
}

/// 按空白拆分参数：支持双引号包裹含空格的参数；未加引号且以 `#` 开头的 token 视为行尾注释
fn split_args(text: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut token_started = false;

    for c in text.chars() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                token_started = true;
            }
            c if c.is_whitespace() && !in_quotes => {
                if token_started {
                    args.push(std::mem::take(&mut current));
                    token_started = false;
                }
            }
            '#' if !in_quotes && !token_started => break,
            c => {
                current.push(c);
                token_started = true;
            }
        }
    }
    if token_started {
        args.push(current);
    }
    args
}

/// 以宽松方式读取文件：不存在、无权限、非 UTF-8 都不报错（非 UTF-8 按 lossy 解码）
fn read_lossy(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

// ============================================================
// Include 路径展开
// ============================================================

/// 展开一个 Include 参数：`~` → 主目录；相对路径 → 相对 `~/.ssh`；再做通配展开
fn include_paths(pattern: &str, env: &ParseEnv) -> Vec<PathBuf> {
    let expanded = expand_tilde(pattern, &env.home);
    let path = if expanded.is_absolute() {
        expanded
    } else {
        env.ssh_dir.join(expanded)
    };
    glob_expand(&path)
}

/// 极简 glob：逐路径分量展开 `*` / `?`（结果按文件名排序，与 glob(3) 一致）
///
/// 无通配符的分量直接拼接、不检查存在性，交给后续读取失败静默跳过。
fn glob_expand(path: &Path) -> Vec<PathBuf> {
    let mut results = vec![PathBuf::new()];

    for component in path.components() {
        let name = component.as_os_str().to_string_lossy();
        let is_wildcard = matches!(component, Component::Normal(_)) && name.contains(['*', '?']);
        if !is_wildcard {
            for result in &mut results {
                result.push(component.as_os_str());
            }
            continue;
        }

        let mut expanded = Vec::new();
        for base in &results {
            let dir = if base.as_os_str().is_empty() {
                Path::new(".")
            } else {
                base.as_path()
            };
            let Ok(read_dir) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut names: Vec<String> = read_dir
                .flatten()
                .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
                .filter(|entry_name| glob_name_matches(entry_name, &name))
                .collect();
            names.sort();
            expanded.extend(names.into_iter().map(|entry_name| base.join(entry_name)));
        }
        results = expanded;
    }

    results
}

/// 文件名通配匹配：区分大小写；与 glob(3) 一致，`*` / `?` 不匹配以 `.` 开头的隐藏文件
fn glob_name_matches(name: &str, pattern: &str) -> bool {
    if name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    wildcard_match(name, pattern)
}

// ============================================================
// 模式匹配
// ============================================================

/// `*` / `?` 通配匹配（区分大小写；`*` 可匹配空串）
fn wildcard_match(text: &str, pattern: &str) -> bool {
    let text: Vec<char> = text.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    let (mut ti, mut pi) = (0usize, 0usize);
    // 最近一次 `*` 之后的模式位置，以及该 `*` 当前吞掉的文本终点（用于回溯）
    let mut star: Option<(usize, usize)> = None;

    while ti < text.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == text[ti]) {
            ti += 1;
            pi += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star = Some((pi + 1, ti));
            pi += 1;
        } else if let Some((star_pi, star_ti)) = star {
            pi = star_pi;
            ti = star_ti + 1;
            star = Some((star_pi, star_ti + 1));
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}

/// 主机名通配匹配：不区分大小写（OpenSSH 会先把命令行主机名转为小写）
fn host_pattern_match(host: &str, pattern: &str) -> bool {
    wildcard_match(&host.to_lowercase(), &pattern.to_lowercase())
}

/// OpenSSH `Host` 行语义：命中任一正向模式即匹配，但命中任一 `!` 取反模式则整行不匹配
fn match_pattern_list(host: &str, patterns: &[String]) -> bool {
    let mut positive = false;
    for pattern in patterns {
        let (negated, pattern) = strip_negation(pattern);
        if pattern.is_empty() || !host_pattern_match(host, pattern) {
            continue;
        }
        if negated {
            return false;
        }
        positive = true;
    }
    positive
}

/// 去掉 `!` 取反前缀，返回（是否取反, 剩余文本）
fn strip_negation(token: &str) -> (bool, &str) {
    match token.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, token),
    }
}

/// 评估 `Match` 条件列表（全部条件同时成立才匹配）
///
/// `host` / `originalhost` 的参数是逗号分隔的模式列表；需要运行期信息的条件
/// （`user` / `localuser` / `exec` / `tagged` / `localnetwork` 等）一律视为不匹配。
/// 本解析器不执行 OpenSSH 的 canonical/final 重解析阶段，相应块（含取反条件）整体跳过。
fn eval_match(criteria: &[String], alias: &str) -> bool {
    if criteria.is_empty() {
        return false;
    }
    let mut index = 0;
    while index < criteria.len() {
        let (negated, name) = strip_negation(&criteria[index]);
        let matched = match name.to_ascii_lowercase().as_str() {
            "all" => true,
            // 下拉配置解析只运行初始阶段；不猜测 OpenSSH 重解析条件。
            // 即使带 ! 也整体跳过，避免将不支持的条件误解释为命中。
            "canonical" | "final" => return false,
            "host" | "originalhost" => {
                index += 1;
                let Some(patterns) = criteria.get(index) else {
                    return false;
                };
                let patterns: Vec<String> = patterns.split(',').map(str::to_owned).collect();
                match_pattern_list(alias, &patterns)
            }
            _ => return false,
        };
        if matched == negated {
            return false;
        }
        index += 1;
    }
    true
}

// ============================================================
// 别名收集与配置解析
// ============================================================

/// 收集所有 `Host` 行中的具体别名（跳过通配符与取反模式），按首次出现顺序去重
fn collect_aliases(blocks: &[Block]) -> Vec<String> {
    let mut aliases: Vec<String> = Vec::new();
    for block in blocks {
        let BlockHead::Host(patterns) = &block.head else {
            continue;
        };
        for pattern in patterns {
            let is_concrete = !pattern.starts_with('!') && !pattern.contains(['*', '?']);
            if is_concrete && !aliases.iter().any(|existing| existing == pattern) {
                aliases.push(pattern.clone());
            }
        }
    }
    aliases
}

/// 按"首次出现的值生效"规则解析某个别名的生效配置
fn resolve(alias: &str, blocks: &[Block], env: &ParseEnv) -> SshHostEntry {
    let mut host_name: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut user: Option<String> = None;
    let mut raw_identity_files: Vec<String> = Vec::new();
    let mut proxy_jump: Option<String> = None;
    let mut proxy_command: Option<String> = None;

    for block in blocks.iter().filter(|block| block.matches(alias)) {
        for (keyword, args) in &block.directives {
            match keyword.as_str() {
                "hostname" => set_once(&mut host_name, args.first().cloned()),
                "port" => set_once(&mut port, args.first().and_then(|arg| arg.parse().ok())),
                "user" => set_once(&mut user, args.first().cloned()),
                "identityfile" => raw_identity_files.extend(args.first().cloned()),
                "proxyjump" => set_once(&mut proxy_jump, args.first().cloned()),
                "proxycommand" if !args.is_empty() => {
                    set_once(&mut proxy_command, Some(args.join(" ")));
                }
                _ => {}
            }
        }
    }

    let host_name = host_name
        .map(|raw| expand_tokens(&raw, |token| (token == 'h').then(|| alias.to_owned())))
        .unwrap_or_else(|| alias.to_owned());
    let port = port.unwrap_or(DEFAULT_PORT);

    let identity_files = raw_identity_files
        .iter()
        .map(|raw| {
            let expanded = expand_tokens(raw, |token| match token {
                'd' => Some(env.home.to_string_lossy().into_owned()),
                'h' => Some(host_name.clone()),
                'n' => Some(alias.to_owned()),
                'p' => Some(port.to_string()),
                'r' => user.clone(),
                'u' if !env.local_user.is_empty() => Some(env.local_user.clone()),
                _ => None,
            });
            expand_tilde(&expanded, &env.home)
        })
        .collect();

    SshHostEntry {
        alias: alias.to_owned(),
        host_name,
        port,
        user,
        identity_files,
        proxy_jump: proxy_jump.filter(|value| !value.eq_ignore_ascii_case("none")),
        proxy_command: proxy_command.filter(|value| !value.eq_ignore_ascii_case("none")),
    }
}

/// 仅在尚未赋值时写入（"首次出现的值生效"）
fn set_once<T>(slot: &mut Option<T>, value: Option<T>) {
    if slot.is_none() {
        *slot = value;
    }
}

/// 展开 `%x` token：`%%` → `%`；`lookup` 返回 None 的 token 原样保留
fn expand_tokens(raw: &str, lookup: impl Fn(char) -> Option<String>) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            None => out.push('%'),
            Some('%') => out.push('%'),
            Some(token) => match lookup(token) {
                Some(value) => out.push_str(&value),
                None => {
                    out.push('%');
                    out.push(token);
                }
            },
        }
    }
    out
}

/// 展开路径开头的 `~`（仅当前用户；`~user/` 形式不支持，原样返回）
fn expand_tilde(raw: &str, home: &Path) -> PathBuf {
    if raw == "~" {
        return home.to_path_buf();
    }
    match raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None => PathBuf::from(raw),
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_env() -> ParseEnv {
        ParseEnv {
            ssh_dir: PathBuf::from("/home/tester/.ssh"),
            home: PathBuf::from("/home/tester"),
            local_user: "tester".into(),
        }
    }

    fn find<'a>(hosts: &'a [SshHostEntry], alias: &str) -> &'a SshHostEntry {
        hosts
            .iter()
            .find(|host| host.alias == alias)
            .unwrap_or_else(|| panic!("缺少别名 {alias}"))
    }

    /// 创建本测试专用的临时目录（进程 ID + 名称保证互不干扰）
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "task-graph-editor-sshcfg-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir
    }

    #[test]
    fn tokenize_supports_whitespace_equals_quotes_and_comments() {
        assert_eq!(
            tokenize_line("  HostName  example.com  "),
            Some(("hostname".into(), vec!["example.com".into()]))
        );
        assert_eq!(
            tokenize_line("Port=2222"),
            Some(("port".into(), vec!["2222".into()]))
        );
        assert_eq!(
            tokenize_line("Port = 2222"),
            Some(("port".into(), vec!["2222".into()]))
        );
        assert_eq!(
            tokenize_line(r#"IdentityFile "~/.ssh/my key" # 注释"#),
            Some(("identityfile".into(), vec!["~/.ssh/my key".into()]))
        );
        assert_eq!(
            tokenize_line("Host a b   c"),
            Some(("host".into(), vec!["a".into(), "b".into(), "c".into()]))
        );
        assert_eq!(tokenize_line("# 整行注释"), None);
        assert_eq!(tokenize_line("   "), None);
        // 参数内部的 # 不是注释
        assert_eq!(
            tokenize_line("User a#b"),
            Some(("user".into(), vec!["a#b".into()]))
        );
    }

    #[test]
    fn wildcard_matching() {
        assert!(wildcard_match("robot-01", "robot-*"));
        assert!(wildcard_match("robot-01", "robot-0?"));
        assert!(wildcard_match("robot", "robot*"));
        assert!(wildcard_match("abc", "*"));
        assert!(wildcard_match("", "*"));
        assert!(wildcard_match("a.b.c", "*.c"));
        assert!(wildcard_match("aXbXc", "a*b*c"));
        assert!(!wildcard_match("robot-100", "robot-0?"));
        assert!(!wildcard_match("robot", "robot-*"));
        assert!(!wildcard_match("abc", "abd"));
        assert!(!wildcard_match("", "?"));
        // 主机名匹配不区分大小写
        assert!(host_pattern_match("Robot-01", "robot-*"));
    }

    #[test]
    fn pattern_list_negation() {
        let patterns = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(match_pattern_list("robot-01", &patterns(&["robot-*"])));
        assert!(!match_pattern_list(
            "robot-01",
            &patterns(&["robot-*", "!robot-01"])
        ));
        assert!(match_pattern_list(
            "robot-02",
            &patterns(&["!robot-01", "robot-*"])
        ));
        // 只有取反模式、没有正向命中：不匹配
        assert!(!match_pattern_list("other", &patterns(&["!robot-01"])));
        assert!(!match_pattern_list("other", &patterns(&[])));
    }

    #[test]
    fn basic_resolution_and_defaults() {
        let config = "
Host robot
    HostName 192.168.1.10
    User linux
    Port 2222

Host bare
";
        let hosts = parse_hosts(config, &test_env());
        assert_eq!(hosts.len(), 2);

        let robot = find(&hosts, "robot");
        assert_eq!(robot.host_name, "192.168.1.10");
        assert_eq!(robot.user.as_deref(), Some("linux"));
        assert_eq!(robot.port, 2222);
        assert!(robot.identity_files.is_empty());
        assert!(robot.proxy_jump.is_none());

        let bare = find(&hosts, "bare");
        assert_eq!(bare.host_name, "bare");
        assert_eq!(bare.port, DEFAULT_PORT);
        assert_eq!(bare.user, None);
    }

    #[test]
    fn first_value_wins_with_wildcard_defaults() {
        let config = "
User global-first

Host robot
    Port 2222

Host robot-*
    User pattern-user
    HostName should-not-win

Host *
    User star-user
    Port 9999
    IdentityFile ~/.ssh/star_key
";
        let hosts = parse_hosts(config, &test_env());
        // 通配符块不产生别名
        assert_eq!(hosts.len(), 1);

        let robot = find(&hosts, "robot");
        // 全局指令在最前面，先于所有块生效
        assert_eq!(robot.user.as_deref(), Some("global-first"));
        assert_eq!(robot.port, 2222);
        // robot-* 不匹配 robot
        assert_eq!(robot.host_name, "robot");
        // IdentityFile 来自 Host * 块并做 ~ 展开
        assert_eq!(
            robot.identity_files,
            vec![PathBuf::from("/home/tester/.ssh/star_key")]
        );
    }

    #[test]
    fn multiple_aliases_per_host_line_and_dedup() {
        let config = "
Host a b !c d-*
    HostName shared.example.com

Host a
    Port 2200
";
        let hosts = parse_hosts(config, &test_env());
        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, vec!["a", "b"]);
        assert_eq!(find(&hosts, "a").host_name, "shared.example.com");
        assert_eq!(find(&hosts, "a").port, 2200);
        assert_eq!(find(&hosts, "b").port, DEFAULT_PORT);
    }

    #[test]
    fn hostname_token_and_identity_expansion() {
        let config = "
Host web1
    HostName %h.internal.example.com
    User deploy
    Port 2201
    IdentityFile %d/.ssh/keys/%n_%r_%p_%u
    IdentityFile ~/.ssh/id_%h
    IdentityFile /abs/%%literal/%z
";
        let hosts = parse_hosts(config, &test_env());
        let web1 = find(&hosts, "web1");
        assert_eq!(web1.host_name, "web1.internal.example.com");
        assert_eq!(
            web1.identity_files,
            vec![
                PathBuf::from("/home/tester/.ssh/keys/web1_deploy_2201_tester"),
                PathBuf::from("/home/tester/.ssh/id_web1.internal.example.com"),
                PathBuf::from("/abs/%literal/%z"),
            ]
        );
    }

    #[test]
    fn match_blocks() {
        let config = "
Host alpha
Host beta
Host gamma

Match host alpha,beta
    User matched

Match !host gamma
    Port 3333

Match all
    HostName all.example.com

Match user someone
    Port 4444

Match exec \"true\"
    Port 5555
";
        let hosts = parse_hosts(config, &test_env());
        let alpha = find(&hosts, "alpha");
        assert_eq!(alpha.user.as_deref(), Some("matched"));
        assert_eq!(alpha.port, 3333);
        assert_eq!(alpha.host_name, "all.example.com");

        let gamma = find(&hosts, "gamma");
        assert_eq!(gamma.user, None);
        // !host gamma 不匹配 gamma；user/exec 条件视为不匹配
        assert_eq!(gamma.port, DEFAULT_PORT);
        assert_eq!(gamma.host_name, "all.example.com");
    }

    #[test]
    fn final_and_canonical_blocks_never_override_initial_identity() {
        let config = "
Match final
    User final-user
    Port 9999
Match !canonical
    IdentityFile ~/.ssh/wrong-key
Host robot
    HostName 127.0.0.1
    User robot-user
    Port 2222
    IdentityFile ~/.ssh/robot-key
";
        let hosts = parse_hosts(config, &test_env());
        let robot = find(&hosts, "robot");
        assert_eq!(robot.user.as_deref(), Some("robot-user"));
        assert_eq!(robot.port, 2222);
        assert_eq!(
            robot.identity_files,
            vec![PathBuf::from("/home/tester/.ssh/robot-key")]
        );
    }

    #[test]
    fn proxy_none_and_proxy_command_join() {
        let config = "
Host jumped
    ProxyJump bastion.example.com
Host direct
    ProxyJump none
Host cmd
    ProxyCommand ssh -W %h:%p bastion
Host cmd-none
    ProxyCommand none
";
        let hosts = parse_hosts(config, &test_env());
        assert_eq!(
            find(&hosts, "jumped").proxy_jump.as_deref(),
            Some("bastion.example.com")
        );
        assert_eq!(find(&hosts, "direct").proxy_jump, None);
        assert_eq!(
            find(&hosts, "cmd").proxy_command.as_deref(),
            Some("ssh -W %h:%p bastion")
        );
        assert_eq!(find(&hosts, "cmd-none").proxy_command, None);
    }

    #[test]
    fn crlf_and_case_insensitive_keywords() {
        let config = "HOST win\r\n  hostname 10.0.0.1\r\n  PORT 22022\r\n  user Administrator\r\n";
        let hosts = parse_hosts(config, &test_env());
        let win = find(&hosts, "win");
        assert_eq!(win.host_name, "10.0.0.1");
        assert_eq!(win.port, 22022);
        assert_eq!(win.user.as_deref(), Some("Administrator"));
    }

    #[test]
    fn invalid_port_is_ignored() {
        let config = "
Host bad
    Port notanumber
    Port 2020
";
        let hosts = parse_hosts(config, &test_env());
        assert_eq!(find(&hosts, "bad").port, 2020);
    }

    #[test]
    fn include_with_glob_and_nested_block_semantics() {
        let root = temp_dir("include");
        let ssh_dir = root.join(".ssh");
        let conf_d = ssh_dir.join("conf.d");
        std::fs::create_dir_all(&conf_d).expect("创建 conf.d 失败");

        std::fs::write(
            conf_d.join("10-robots.conf"),
            "Host robot-a\n    HostName 10.0.0.2\n",
        )
        .expect("写入失败");
        std::fs::write(
            conf_d.join("20-servers.conf"),
            "Host server-b\n    HostName 10.0.0.3\n    Port 2222\n",
        )
        .expect("写入失败");
        // 隐藏文件不应被 * 匹配
        std::fs::write(conf_d.join(".hidden.conf"), "Host hidden\n").expect("写入失败");
        // 块内 Include：被包含文件只含指令时并入当前块
        std::fs::write(ssh_dir.join("common-user"), "User common\n").expect("写入失败");

        let config = "
Include conf.d/*.conf
Include ~/.ssh/missing-*.conf

Host with-include
    Include common-user
    HostName 10.0.0.4
";
        let env = ParseEnv {
            ssh_dir: ssh_dir.clone(),
            home: root.clone(),
            local_user: "tester".into(),
        };
        let hosts = parse_hosts(config, &env);
        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, vec!["robot-a", "server-b", "with-include"]);
        assert_eq!(find(&hosts, "robot-a").host_name, "10.0.0.2");
        assert_eq!(find(&hosts, "server-b").port, 2222);
        assert_eq!(find(&hosts, "with-include").user.as_deref(), Some("common"));
        assert_eq!(find(&hosts, "with-include").host_name, "10.0.0.4");
        // 非 Include 块的主机不受块内 Include 影响
        assert_eq!(find(&hosts, "robot-a").user, None);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn include_depth_is_bounded() {
        let root = temp_dir("include-loop");
        let ssh_dir = root.join(".ssh");
        std::fs::create_dir_all(&ssh_dir).expect("创建 .ssh 失败");
        // 自包含循环：必须在深度上限处停止而不是栈溢出
        std::fs::write(ssh_dir.join("loop"), "Host looped\nInclude loop\n").expect("写入失败");

        let env = ParseEnv {
            ssh_dir: ssh_dir.clone(),
            home: root.clone(),
            local_user: String::new(),
        };
        let hosts = parse_hosts("Include loop\n", &env);
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].alias, "looped");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn default_identity_files_only_existing() {
        let root = temp_dir("identities");
        std::fs::write(root.join("id_ed25519"), "key").expect("写入失败");
        std::fs::write(root.join("id_rsa"), "key").expect("写入失败");
        std::fs::create_dir_all(root.join("id_ecdsa")).expect("创建目录失败");

        assert_eq!(
            default_identity_files_in(&root),
            vec![root.join("id_rsa"), root.join("id_ed25519")]
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tilde_expansion() {
        let home = Path::new("/home/tester");
        assert_eq!(expand_tilde("~", home), PathBuf::from("/home/tester"));
        assert_eq!(
            expand_tilde("~/.ssh/id_ed25519", home),
            PathBuf::from("/home/tester/.ssh/id_ed25519")
        );
        assert_eq!(expand_tilde("/abs/path", home), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("~other/x", home), PathBuf::from("~other/x"));
    }
}

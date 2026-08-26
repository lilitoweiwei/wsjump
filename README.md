# wsjump

wsjump 管理一批「小工作区」：每个工作区就是 `w/` 目录下的一个普通文件夹，命名形如 `20260730-wsjump`。它解决两个问题：

- **找**：工作区一多，靠眼睛在目录里翻很慢。wsjump 按 slug 和标签做子串匹配，一条命令（或一个交互式选择器）就能定位目标。
- **跳**：`cd` 进一个嵌套很深的路径很啰嗦。wsjump 返回目标的绝对路径，配合 shell 集成可以一键跳转。

## 两种交互形态

wsjump 是 Cargo workspace，产出两个可执行文件，各司其职：

| 二进制 | 形态 | 用途 |
|---|---|---|
| `wsjump` | 非交互 CLI | 查询并打印路径，服务自动化 / 脚本 |
| `wsjump-tui` | 交互式 TUI | 在终端里弹出一个行内候选列表，选中即跳转（经 shell 函数） |

日常交互你几乎不会手敲 `wsjump-tui`，而是通过 shell 函数 `ju` 触发它（见「shell 集成」）。

## 快速上手

1. 编译（见「安装 → 编译」）。
2. 把 `wsjump`、`wsjump-tui`（Windows 下为 `wsjump.exe`、`wsjump-tui.exe`）复制到一个稳定的目录（如 `G:/ws/bin`），并在**同一目录**写好 `wsjump.toml`（见「安装 → 配置」）。
3. 把该目录加入 `PATH`。
4. 按「安装 → shell 集成」给你的 shell 加一行集成（Windows 用 PowerShell profile，Linux 用 `~/.bashrc`）。
5. 重开终端，输入 `ju` 即可交互式跳转；输入 `wsjump help` 查看查询指令。

## 安装

### 编译

需要 Rust 工具链（edition 2021），以及一个 C 编译器（`rusqlite` 的 `bundled` 特性会把 SQLite 的 C 源码编进二进制；Windows 上即 MSVC 构建工具）。

```powershell
cargo build --release
```

产物在 `target/release/` 下：`wsjump.exe` 与 `wsjump-tui.exe`。

### 部署

把两个 exe 复制到一个**稳定的目录**（建议是某个已在 `PATH` 里的 `bin/`，例如 `G:/ws/bin`）。随后在该目录写 `wsjump.toml` 配置文件。

### 配置（wsjump.toml）

放在可执行文件同目录下的 TOML 文件。最小内容：

```toml
# w/ 目录的绝对路径
workspaces_dir = "G:/ws/w"

# 生成 / 解析日期所用的固定 UTC 偏移（小时），东八区 = 8
utc_offset_hours = 8

# --- 以下均可选 ---

# 数据库文件位置；缺省为可执行文件同目录下的 wsjump.db
# db_file = "G:/ws/bin/wsjump.db"

# 无 --sort 时的默认排序：recency（缺省）| id
# default_sort = "recency"
```

### 加入 PATH

确保 `bin/` 目录在你的用户环境变量 `PATH` 中，这样 `wsjump` / `wsjump-tui` 才能按名直接调用。

### shell 集成

`wsjump-tui init [<shell>]` 会输出一段集成片段（含安装说明与 `ju` 函数定义），`<shell>` 取 `powershell`（缺省）或 `bash`。片段同时是安装指南：注释部分在 eval 时是无害的，`ju` 函数定义则会被生效。安装后得到 `ju` 函数，可以交互式进行目录跳转，具体使用方法请执行 `ju --help` 来查阅。

**Windows / PowerShell**：把输出加进你的 PowerShell profile（`notepad $PROFILE` 打开）：

```powershell
Invoke-Expression (& wsjump-tui init | Out-String)
```

**Linux / bash**：先把 `wsjump`、`wsjump-tui` 和 `wsjump.toml` 放进一个已在 `PATH` 里的用户目录（例如 `~/bin`），再把输出加进 `~/.bashrc`：

```bash
eval "$(wsjump-tui init bash)"
```

升级 wsjump-tui 后重新执行上面的 INSTALL 行即可让函数保持同步；卸载即从 profile / `~/.bashrc` 里删掉那一行。

## 工作区结构约定

- 所有工作区**平铺**在配置的 `workspaces_dir`（即 `w/`）下，没有嵌套。
- 目录名形如 `<YYYYMMDD>` 或 `<YYYYMMDD>-<slug>`：
  - 前 8 位必须是**合法日历日期**（`20261340` 这类非法日期视为非工作区）。
  - 第 9 个字符若存在必须是 `-`；其后（可为空）即 slug。slug 内部允许再含 `-`（只在第 9 位切一刀）。
  - 合法示例：`20260730`（slug 空）、`20260730-`（slug 空）、`20260730-wsjump`、`20260730-a-b-c`（slug = `a-b-c`）。
- 不符合规则的名字会被 warn 到 stderr 并跳过，不收录为工作区。`w/` 下非目录的文件一律忽略。

## wsmeta.txt 写法

每个工作区可以有一个可选的 `wsmeta.txt`，用来登记标签和描述。它决定了 wsjump 能「搜到什么」。

- **位置**：`<workspace>/wsmeta.txt`，不存在则标签、描述均为空。
- **标签行**：**只有第一个「首个非空白字符是 `:`」的行**是标签行。按空白拆分成多个 token，逐个去掉前导 `:` 后收入标签。以下几种写法等价，都表示 `ai, tool, job` 三个标签：

  ```
  :ai tool job
  :ai :tool :job
  :ai tool :job
  ```

- **描述**：其余所有行（含空行、含之后再次出现的 `:` 开头行）一律作为描述，原样保留。描述仅用于展示，不参与查询。

示例：

```
:tool :rust
用于快速跳转到指定小工作区的小工具
```

## 索引机制

wsjump 用一个 SQLite 数据库（默认是和二进制文件同目录的 `wsjump.db`）充当**索引+访问历史**，加速查询并支撑「最近访问」排序。

- **自动 sync**：每次进程启动都会先扫一遍 `w/` 并与数据库对账（新建、更新、删除）。它只重读「大小或修改时间发生变化」的 `wsmeta.txt`，所以稳态下几乎零成本；通常你无需手动干预。
- **最近访问（recency）排序**：数据库为每个工作区记录 `last_access`（初始为创建日零点）。「跳转 / 新建 / `touch`」都会记一次访问，使该工作区在默认排序中居前。
- **`wsjump sync`**：强制全量重读所有工作区元数据。只有当磁盘上的文件明明是对的、索引里的结果却显得陈旧（签名碰撞或外部改动）时才需要它。带路径 `wsjump sync <path>` 只重读那一个工作区。
- **数据库不可用时的降级**：开库 / 迁移 / sync 失败只 warn 到 stderr，然后退回「扫盘 + 内存过滤 + 按 id 排序」，命令照常成功——数据库只是加速与状态载体，绝不能阻止你到达工作区。
- **数据库可重建**：它本质是一份缓存 + 少量状态，删掉后下次 sync 会重建索引（代价是丢失访问历史，所有工作区退回按创建日排序）。

## 使用

具体指令以内联帮助为准，运行 `wsjump` （或者交互式tui的 `ju --help`） 查看完整说明。

- **查询**：一串空格分隔的 token，token 之间是 AND 关系。`:开头` 的 token 只匹配标签；其余 token 匹配 slug + 标签。匹配是**大小写不敏感的子串**。空查询匹配全部。
- **不参与匹配**：日期前缀与描述（所以找东西时不用打日期）。
- **排序**：`--sort=recency`（最近访问优先，缺省）或 `--sort=id`（按目录名字典序）。
- **退出码**：`0` 成功（含 `ls` 空结果）；`1` 运行期错误（如 `top` 无匹配、配置缺失）；`2` 用法错误（未知子命令 / 选项）。

## 开发

### 技术栈

- 语言：Rust（edition 2021）。
- 仓库：Cargo workspace，三个 crate（双二进制）：

  ```
  wsjump/
    crates/
      wsjump-core/    lib：全部业务逻辑（CLI 与 TUI 共享）
      wsjump-cli/     bin：产物 wsjump(.exe)，argv 分发 + 子命令
      wsjump-tui/     bin：产物 wsjump-tui(.exe)，交互选择器 + shell init
  ```

### 编译与测试

```powershell
cargo build --release   # 编译
cargo test              # 测试（core / cli 的单元测试；tui 的分页与选择逻辑以 App 单元测试覆盖）
```

### 平台

当前以 **Windows / PowerShell** 为主平台；bash（Linux）集成已可用；zsh / fish 集成在待办中。

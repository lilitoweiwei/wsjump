# wsjump v3

wsjump 管理一批小工作区：工作区可以平铺在 `w/` 下，也可以按分类放在 `c/` 下。目录结构直接服务于文件管理器和保存对话框，wsjump 提供名称、标签检索和终端跳转。

详细设计见同级文档目录中的 [v3 设计文档](../doc/20261103-v3-design.md)。

## 目录约定

配置中的 `root_dir` 是包含 `w`、`c` 的大工作区根目录：

```text
G:/ws/
├─ w/
│  └─ scratch/
└─ c/
   └─ _ai/
      ├─ _opencode/
      │  └─ my-plugin/
      └─ demo/
```

- `w/` 下的直接子目录是工作区，不要求日期前缀。
- `c/` 的分类层级中，`_标签` 是分类目录，普通目录是工作区。
- 分类标签从祖先目录继承：`my-plugin` 的分类标签是 `ai`、`opencode`，`demo` 的分类标签是 `ai`。
- 找到工作区后不再递归，工作区内部的 `_cache` 等目录不会参与分类。
- `c/` 下也可以直接放工作区，此时没有继承标签。`w/` 下的 `_` 没有分类含义。
- 工作区名称非空、不含空白字符，必须是单个普通目录名。分类标签也必须非空、不含空白字符，`_` 和 `_two words` 不是合法分类。
- 文件忽略，目录符号链接不作为扫描入口；不合法目录会告警并跳过其子树。
- 根目录必须存在。`w`、`c` 可以暂不存在，创建工作区时会自动建立相应容器。

工作区使用根目录相对路径作为唯一键，例如 `c/_ai/demo` 和 `c/_nvim/demo` 可以同名。路径预期基本冻结；移动或重命名工作区、修改祖先分类名称，会被视为旧工作区删除、新工作区出现，访问记录不自动跟随。

## 安装与配置

### 编译与部署

需要 Rust 工具链（edition 2021）和 C 编译器。SQLite 使用 `rusqlite` 的 bundled 特性；Windows 使用 MSVC 构建工具。

```powershell
cargo build --release
```

产物是 `target/release/wsjump.exe` 和 `target/release/wsjump-tui.exe`。将两个程序放在已加入 `PATH` 的稳定目录中，并在程序同目录创建 `wsjump.toml`：

```toml
root_dir = "G:/ws"

# 可选，默认与可执行文件同目录下的 wsjump.db
# db_file = "G:/ws/bin/wsjump.db"

# 可选：recency（默认）或 name
# default_sort = "recency"
```

`root_dir` 必须是绝对路径。配置不接受未知项；旧的 `workspaces_dir`、`utc_offset_hours` 和 `id` 排序不再支持。

配置和默认的 `wsjump.db` 都放在**安装目录**里，即 `wsjump.toml` 所在目录。Windows 下安装目录就是可执行文件所在目录；Linux 下程序会依次尝试「`argv[0]` 的目录 → `PATH` 中指向本源代码树的目录 → 真实二进制所在目录」。因此在 `~/bin` 里放两个指向 `target/release/` 的符号链接、再把 `wsjump.toml` 放在 `~/bin`，同样能正常找到配置。判定细节见 `crates/wsjump-core/src/config.rs` 的 `install_dir` 注释。

### PowerShell 集成

在 PowerShell profile 中加入：

```powershell
Invoke-Expression (& wsjump-tui init | Out-String)
```

重开终端后使用 `ju`：

```powershell
ju                              # 交互式选择并跳转
ju new scratch                  # 创建 w/scratch 并跳转
ju new my-plugin _ai/_opencode   # 创建 c/_ai/_opencode/my-plugin 并跳转
ju touch                        # 记录当前工作区的访问
ju sync                         # 强制同步全部工作区
ju --help                       # 查看 shell 集成帮助
```

`new`、`touch`、`sync` 也可缩写为 `n`、`t`、`s`。集成会把程序返回的绝对路径交给 `Set-Location`。程序的 UI、告警、同步进度写入 stderr，stdout 留给路径结果。

### Linux（bash）集成

把 `wsjump`、`wsjump-tui` 和 `wsjump.toml` 放进一个已在 `PATH` 里的目录（例如 `~/bin`），再把集成片段加进 `~/.bashrc`：

```bash
eval "$(wsjump-tui init bash)"
```

`init` 不带参数时输出 PowerShell 片段，Linux 上必须显式写 `bash`。重开终端（或 `source ~/.bashrc`）后 `ju` 的用法与上面一致，选中的路径交给 `cd`。升级程序后重新执行上面这一行（或重新 `source`）即可让函数保持同步；卸载就是从 `~/.bashrc` 里删掉它。

### 从旧版升级

v3 不保留旧版配置和数据库兼容逻辑。更新配置后，使用新的 `db_file`，或在程序退出后移除旧 `wsjump.db` 及其 `-wal`、`-shm` 文件，再由 v3 重建。重建会丢失旧访问历史。

数据库 schema 必须为 3。遇到其他版本时程序会告警，降级为扫盘查询、按名称排序，直到换用或重建数据库。已有日期前缀目录无需立即重命名，它们按完整普通名称识别和搜索。

## wsmeta.txt

每个工作区可以有一个可选的 `wsmeta.txt`：

```text
:opencode rust tool
OpenCode 插件开发
```

- 只有第一个「首个非空白字符为 `:`」的行是标签行。
- 按空白字符拆分标签，每个 token 去掉一个可选前导 `:`。`:ai tool` 与 `:ai :tool` 等价。
- 其余行，包括空行和之后再次以 `:` 开头的行，都作为描述。
- 有效标签是分类标签与文件标签的并集；按分类从外到内、然后文件标签的顺序去重，保留原始文本。
- 文件不存在时元信息为空。读取失败时告警并使用空元信息，分类标签仍然保留。

例如上述文件放在 `c/_ai/_opencode/my-plugin` 中，最终标签为 `ai`、`opencode`、`rust`、`tool`。删除文件只清除文件贡献的标签和描述。

## 命令

```text
wsjump new <名称> [分类路径]
wsjump ls [--sort=name|recency] [查询...]
wsjump top [--sort=name|recency] [查询...]
wsjump tags [--sort=name|recency]
wsjump touch [路径]
wsjump sync [路径]
wsjump help
```

### 创建

```powershell
wsjump new scratch
wsjump new my-plugin _ai/_opencode
```

名称必填，最多两个位置参数，每次只创建一个工作区。省略分类路径时放在 `w`；提供时相对于 `c`，每个分量必须是 `_标签`。缺失分类自动建立，已有目标报冲突，不覆盖。部分 IO 失败前建立的空分类目录可以保留。

### 检索与排序

- 空格分隔的查询 token 之间为 AND。
- `:ai` 只匹配有效标签；普通 token 匹配完整名称和有效标签。
- 匹配为大小写不敏感的子串；描述和路径键不参与查询。
- `recency` 最近访问优先，同时间以唯一路径键降序打破并列。
- `name` 按完整名称字典序升序，同名以唯一路径键升序打破并列。
- `ls` 输出每个匹配工作区的绝对路径；`top` 输出第一项。
- `tags` 输出空格分隔的标签集合；名称排序下标签为字典序，最近访问排序下保留工作区访问顺序中的首次出现顺序。
- `--sort=name` 和 `--sort name` 均可；查询名称以 `-` 开头时使用 `--` 结束选项解析。

TUI 使用同一个查询接口，候选项仅显示名称、标签、描述：

```text
demo  :ai :tool  描述……
demo  :nvim      描述……
```

↑/↓ 选择，Enter 跳转，Esc 退出，Ctrl+S 切换排序，Shift+←/→ 横向滚动。同名且标签、描述相同的项可能显示相同，但内部路径仍然独立。

## 索引与访问记录

SQLite 保存元信息缓存和最近访问时间，每个进程启动时自动同步一次：

- 发现过程枚举 `w` 和 `c` 的分类目录，并检查每个工作区 `wsmeta.txt` 的修改时间、大小。
- 仅新工作区或签名变化时重读文件、合并标签；稳定态不读取元信息正文、不更新工作区记录。
- 分类改名或移动会产生新路径键，自然重新加载元信息。
- 根目录变更会强制刷新缓存中的绝对路径。
- 发现过程必须完整成功才执行对账，读取分支失败不会把未扫描到的工作区当成已删除。
- 首次入库用目录创建时间初始化访问时间；无法取得时使用 `0`。创建、`touch`、TUI 选择记录当前 Unix 秒，普通同步保留已有访问时间。
- `touch` 可从工作区内部任意目录执行；带路径时也可指向内部文件。
- `sync` 强制重读全部元信息；`sync <路径>` 只同步所属工作区，也接受最后一个分量所指工作区已删除的路径。多个祖先都已删除时使用全量同步。
- 数据库不可用时继续扫盘查询，按名称排序，不记录访问。删除并重建数据库会丢失访问历史。

退出码：`0` 成功（包括 `ls` 空结果），`1` 运行期错误，`2` 用法错误。

## 开发

仓库由三个 crate 组成：

```text
crates/wsjump-core/   共享目录规则、发现、元信息、索引和创建逻辑
crates/wsjump-cli/    非交互命令，产物 wsjump
crates/wsjump-tui/    交互选择器及 PowerShell 集成，产物 wsjump-tui
```

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release
```

测试覆盖分类继承、工作区边界、创建和冲突、路径归属、增量同步、分类移动、访问记录、数据库降级、Unicode 检索一致性和同名工作区分页。当前主要平台是 Windows / PowerShell。

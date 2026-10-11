# worktree 夹具

`crates/git` 的 porcelain 解析夹具，覆盖 `git worktree list --porcelain -z` 的边界形状。

## 来源

真机抓取（Git 2.55.0，Linux），命令：

```
git -C <repo> worktree list --porcelain -z > <fixture>.porcelain-z
```

抓取后把前缀 `/tmp/opencode/wt` 归一化为 `/repo`，其余字节（含 NUL、空格、中文、引号）
**原样保留**，以便可移植且可对照。

## 覆盖

| 夹具 | 内容 |
| --- | --- |
| `single.porcelain-z` | 只有一个主工作树（分支 `main`） |
| `multi.porcelain-z` | 主工作树 + 路径含引号且 detached + 路径含空格 / 中文且 detached + `locked <原因>` + 无原因 `locked` + `prunable` |
| `bare.porcelain-z` | bare 仓库主工作树（**无 HEAD 行**）+ 其链接 worktree |

## 已确认的形状（#17 D5）

- 字段以 `\0` 结尾，记录之间多一个 `\0`（`\0\0`）；
- `locked` / `prunable` 的原因与键**同一 token**（`locked hold for review`）；
- 路径**原样**输出，含空格 / 中文 / 引号都不做 C 式转义；
- bare 主工作树只有 `worktree` + `bare` 两行。

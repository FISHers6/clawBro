---
name: add-agent
description: 引导向 agent_roster 追加新 Agent，配置名称、触发 mention、Backend、Persona 和工作目录。
---

# 添加 Agent 到 Roster

## 关于本 Skill

向 `~/.quickai/config.toml` 的 `[[agent_roster]]` 追加一个新 Agent。
每个 Agent 可以：
- 有独立的名称和 @mention 触发词
- 绑定不同的 Backend（native / claude-code / codex 等）
- 有独立的工作目录和 Persona 个性
- 在群聊中被 @mention 直接切换

---

## Phase 0：前提确认

```bash
[ -f ~/.quickai/config.toml ] && echo "✓ config.toml 存在" || echo "⚠ 请先运行 /setup"

echo "--- 当前已有 Agents ---"
grep -A 6 '\[\[agent_roster\]\]' ~/.quickai/config.toml 2>/dev/null || echo "（暂无 agent_roster）"

echo "--- 可用 Backends ---"
grep -A 1 '\[\[backend\]\]' ~/.quickai/config.toml 2>/dev/null | grep '^id' | sed 's/id *= */  /' || echo "（无 backend 配置）"
```

---

## Phase 1：收集 Agent 基本信息

### 1.1 Agent 名称

询问：Agent 的名称是什么？

命名规则：
- 小写字母、数字、连字符
- 在系统内唯一
- 示例：`claude`、`codex`、`researcher`、`rex`、`coder`

### 1.2 触发 Mention 列表

询问：用户在群聊中用什么 @mention 触发这个 Agent？（可以有多个别名）

示例：
- `["@claude"]`
- `["@代码助手", "@coder"]`
- `["@rex", "@Rex"]`

> 注意：群聊的 @mention 触发受 `require_mention_in_groups` 控制。

### 1.3 选择 Backend

询问：这个 Agent 使用哪个 Backend？

显示可选列表：
```bash
echo "可用 Backends："
grep 'id = ' ~/.quickai/config.toml | grep -A0 'backend' | awk '{print "  " $3}' || true

# 以及内置选项
echo "  native-main  （内置 quickai-rust-agent，默认）"
echo "  claude-code  （如果已通过 /add-acp-backend 添加）"
echo "  codex        （如果已通过 /add-acp-backend 添加）"
```

如果用户想用的 Backend 不存在，提示：
```
Backend "<name>" 不存在，请先运行 /add-acp-backend 添加它，然后再回来配置这个 Agent。
```

---

## Phase 2：收集可选配置

### 2.1 工作目录（可选）

询问：这个 Agent 的工作目录是什么？（Agent 运行时的"当前目录"，影响文件操作的相对路径）

```
示例：
  /Users/xxx/projects/my-app     — 代码助手专注于某个项目
  /Users/xxx/documents           — 文档助手处理文档
  （留空 = 使用 Gateway 全局 default_workspace）
```

### 2.2 Persona 目录（可选）

询问：是否为这个 Agent 配置个性（Persona）？

```
Persona 目录下可以放：
  - SOUL.md        — 核心性格和行为准则
  - IDENTITY.md    — Agent 的身份定义（包含 MBTI、名字、emoji 等）
  - soul-injection.md  — 注入到系统提示词的额外内容

示例目录：~/.quickai/personas/rex

如果不需要独立个性，留空（将使用全局 shared memory）。
```

如果用户想创建 Persona，询问目录路径，并提示：
```bash
mkdir -p <persona_dir>
# 然后可以手动创建 SOUL.md 和 IDENTITY.md
```

### 2.3 是否设为默认 Agent（可选）

询问：是否设为某个 Channel 的默认 Agent（当没有被 @mention 时使用）？

如果是，询问：
- 作用范围（`channel:lark`、`channel:dingtalk`、`scope:group:xxx`）

---

## Phase 3：生成配置

根据收集的信息生成 `[[agent_roster]]` 段：

```toml
[[agent_roster]]
name       = "<name>"
mentions   = [<mention-list>]
backend_id = "<backend-id>"
<如有 workspace_dir>workspace_dir = "<路径>"
<如有 persona_dir>persona_dir  = "<路径>"
```

如需默认 binding，额外生成：
```toml
[[binding]]
kind    = "channel"
channel = "<channel>"
agent   = "<name>"
```

---

## Phase 4：写入配置

```bash
# 备份
cp ~/.quickai/config.toml ~/.quickai/config.toml.bak.$(date +%Y%m%d%H%M%S)

# 追加 agent_roster 配置
cat >> ~/.quickai/config.toml << 'TOMLEOF'

[[agent_roster]]
name       = "<name>"
mentions   = [<mentions>]
backend_id = "<backend-id>"
<如有>workspace_dir = "<路径>"
<如有>persona_dir  = "<路径>"
TOMLEOF

<如有 binding>cat >> ~/.quickai/config.toml << 'TOMLEOF'

[[binding]]
kind    = "channel"
channel = "<channel>"
agent   = "<name>"
TOMLEOF

echo "✓ Agent '<name>' 已添加到 roster"
```

---

## Phase 5：验证

```bash
echo "--- 更新后的 agent_roster ---"
grep -A 8 '\[\[agent_roster\]\]' ~/.quickai/config.toml

echo ""
echo "--- 验证配置语法（重启 Gateway）---"
source ~/.quickai/.env && quickai-gateway &
GATEWAY_PID=$!
sleep 2
PORT=$(cat ~/.quickai/gateway.port 2>/dev/null || echo "8080")
curl -s http://127.0.0.1:$PORT/health
kill $GATEWAY_PID
```

---

## Phase 6：完成确认

```
✓ Agent '<name>' 已成功添加！

触发方式：在群聊中 @<mention> + 消息
Backend：<backend-id>
工作目录：<workspace_dir | 全局默认>
Persona：<persona_dir | 无>

如需开启 Team 模式（Lead + Specialists），请运行：/add-team-mode
```

---

## 配置示例（多 Agent roster）

```toml
# 内置 Agent（代码助手）
[[agent_roster]]
name          = "coder"
mentions      = ["@coder", "@代码助手"]
backend_id    = "native-main"
workspace_dir = "/Users/xxx/projects"

# Claude Code Agent（需要先 /add-acp-backend）
[[agent_roster]]
name          = "claude"
mentions      = ["@claude", "@Claude"]
backend_id    = "claude-code"
workspace_dir = "/Users/xxx/projects"

# Rex — 带 Persona 的研究助手
[[agent_roster]]
name          = "rex"
mentions      = ["@rex", "@Rex"]
backend_id    = "native-main"
persona_dir   = "/Users/xxx/.quickai/personas/rex"

# 绑定：默认用 coder（无 mention 时）
[[binding]]
kind    = "channel"
channel = "lark"
agent   = "coder"
```

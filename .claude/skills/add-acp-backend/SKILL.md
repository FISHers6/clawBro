---
name: add-acp-backend
description: 引导添加外部 ACP Agent Backend（如 claude-code、codex、qwen、goose 等），配置到 config.toml 的 [[backend]] 中。
---

# 添加外部 ACP Backend

## 关于本 Skill

ClawBro Gateway 内置了 `clawbro-rust-agent` 作为默认 Backend（无需安装）。
本 Skill 引导你添加**外部 ACP-compatible Agent**，如：
- `claude-code`（Anthropic Claude Code CLI）
- `codex`（OpenAI Codex CLI）
- `qwen-code`（阿里云通义千问 CLI）
- `goose`（Block 的 AI 助手）
- 任何支持 ACP 协议的 Agent

---

## Phase 0：前提确认

```bash
[ -f ~/.clawbro/config.toml ] && echo "✓ config.toml 存在" || echo "⚠ 请先运行 /setup"
```

---

## Phase 1：选择 Backend 类型

询问用户要添加哪种 Backend：

```
请选择要添加的外部 ACP Backend：

1. claude-code    — Anthropic Claude Code CLI（需要 Node.js + npm）
2. codex          — OpenAI Codex CLI（需要 Node.js + npm）
3. qwen-code      — 通义千问代码助手（需要 Node.js + npm）
4. goose          — Block.xyz Goose AI（需要独立安装）
5. 其他 ACP CLI  — 任意支持 ACP stdio 传输的 CLI 工具

```

根据选择进入对应分支。

---

## Phase 1A：claude-code

### 检查是否已安装

```bash
which claude || npx --yes @anthropic-ai/claude-code --version 2>/dev/null || echo "未安装"
```

### 安装指引

```bash
# 方式 1：全局安装（推荐）
npm install -g @anthropic-ai/claude-code

# 方式 2：每次通过 npx 调用（无需安装）
# Gateway 会在 backend.launch 中使用 npx 调用
```

### 收集参数

询问：
- **Backend ID**（默认：`claude-code`，用于 agent_roster 中的 `backend_id`）
- **工作目录**（claude-code 默认用当前目录，可以在 agent_roster 中单独覆盖）
- **是否使用 npx 启动**（如果未全局安装）

### 生成配置段

**全局安装**：
```toml
[[backend]]
id     = "claude-code"
family = "acp"

[backend.launch]
type    = "command"
command = "claude"
args    = ["--acp-server"]
```

**npx 启动**：
```toml
[[backend]]
id     = "claude-code"
family = "acp"

[backend.launch]
type    = "command"
command = "npx"
args    = ["-y", "@anthropic-ai/claude-code", "--acp-server"]
```

---

## Phase 1B：codex

### 检查是否已安装

```bash
which codex || npx --yes @openai/codex --version 2>/dev/null || echo "未安装"
```

### 安装指引

```bash
npm install -g @openai/codex
```

### 收集参数

- **Backend ID**（默认：`codex`）
- **是否使用 npx 启动**

### 生成配置段

```toml
[[backend]]
id     = "codex"
family = "acp"

[backend.launch]
type    = "command"
command = "codex"
args    = ["--acp"]
env     = { OPENAI_API_KEY = "${OPENAI_API_KEY}" }
```

---

## Phase 1C：qwen-code

### 安装指引

```bash
npm install -g @alibaba/qwen-code
```

### 生成配置段

```toml
[[backend]]
id     = "qwen-code"
family = "acp"

[backend.launch]
type    = "command"
command = "qwen"
args    = ["--acp-server"]
env     = { DASHSCOPE_API_KEY = "${DASHSCOPE_API_KEY}" }
```

---

## Phase 1D：goose

### 安装指引

```
Goose 需要从官网下载安装：
  https://block.github.io/goose/docs/installation

或通过 pipx：
  pipx install goose-ai
```

### 检查安装

```bash
which goose && goose --version || echo "未安装"
```

### 生成配置段

```toml
[[backend]]
id     = "goose"
family = "acp"

[backend.launch]
type    = "command"
command = "goose"
args    = ["agent"]
```

---

## Phase 1E：自定义 ACP CLI

### 收集参数

询问：
- **Backend ID**（自定义名称，如 `my-agent`）
- **可执行命令**（如 `my-agent`、`/usr/local/bin/my-agent`）
- **启动参数**（如 `["--acp", "--stdio"]`）
- **需要的环境变量**（如 `MY_API_KEY`）
- **工作目录**（可选）

### 生成配置段

```toml
[[backend]]
id     = "<backend-id>"
family = "acp"

[backend.launch]
type    = "command"
command = "<命令>"
args    = [<参数列表>]
<如有 env>env = { <KEY> = "${<KEY>}" }
<如有 cwd>cwd = "<工作目录>"
```

---

## Phase 2：关联到 Agent

询问：是否要将新 Backend 关联到某个 Agent（或新建一个 Agent）？

**选项 A：新建 Agent 使用此 Backend**

```bash
# 收集 Agent 参数
# 然后追加 [[agent_roster]] 到 config.toml
```

生成：
```toml
[[agent_roster]]
name       = "<agent-name>"
mentions   = ["@<mention>"]
backend_id = "<backend-id>"
<如有>workspace_dir = "<目录>"
```

**选项 B：更新已有 Agent 的 Backend**

```bash
# 显示当前 agent_roster
grep -A 5 '\[\[agent_roster\]\]' ~/.clawbro/config.toml
```

提示用户手动编辑或告知 AI 哪个 agent 需要更新。

**选项 C：只添加 Backend，不关联 Agent（手动配置）**

跳过 Agent 关联步骤。

---

## Phase 3：写入配置

```bash
# 备份
cp ~/.clawbro/config.toml ~/.clawbro/config.toml.bak.$(date +%Y%m%d%H%M%S)

# 追加 backend 配置
cat >> ~/.clawbro/config.toml << 'TOMLEOF'

<生成的 [[backend]] 配置段>
TOMLEOF

# 如有新 agent，追加
<如有>cat >> ~/.clawbro/config.toml << 'TOMLEOF'

<生成的 [[agent_roster]] 配置段>
TOMLEOF

echo "✓ Backend 配置已写入"
```

---

## Phase 4：验证

### 4.1 检查 CLI 是否可以启动

```bash
# 测试 ACP backend 能否正常启动（仅检查能否执行，不做完整握手）
<command> <args...> --help 2>&1 | head -5 || echo "⚠ 命令无法执行，请检查安装"
```

### 4.2 重启 Gateway 验证

```bash
source ~/.clawbro/.env && clawbro-gateway &
GATEWAY_PID=$!
sleep 2
PORT=$(cat ~/.clawbro/gateway.port 2>/dev/null || echo "8080")

# 检查 backend 状态
curl -s http://127.0.0.1:$PORT/diagnostics/backends 2>/dev/null | python3 -m json.tool || \
  curl -s http://127.0.0.1:$PORT/health

kill $GATEWAY_PID
```

---

## Phase 5：完成确认

```
✓ Backend 已添加！

Backend ID：<id>
命令：<command> <args>
关联 Agent：<agent-name | 未关联>

使用时，在消息中 @<mention> 就会使用此 Backend。
或者在 [[agent_roster]] 中将 backend_id 设为 "<id>"。

其他可用操作：
  /add-agent      — 向 roster 追加更多 Agent
  /doctor         — 诊断 Backend 是否正常工作
```

---

## 配置参考（所有 Backend 汇总）

```toml
# ─── 内置 Backend（无需安装）───────────────────────
[[backend]]
id     = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "bundled_command"

# ─── claude-code（全局安装）────────────────────────
[[backend]]
id     = "claude-code"
family = "acp"

[backend.launch]
type    = "command"
command = "claude"
args    = ["--acp-server"]

# ─── claude-code（npx，无需安装）───────────────────
[[backend]]
id     = "claude-code-npx"
family = "acp"

[backend.launch]
type    = "command"
command = "npx"
args    = ["-y", "@anthropic-ai/claude-code", "--acp-server"]

# ─── codex ─────────────────────────────────────────
[[backend]]
id     = "codex"
family = "acp"

[backend.launch]
type    = "command"
command = "npx"
args    = ["-y", "@openai/codex", "--acp"]
env     = { OPENAI_API_KEY = "${OPENAI_API_KEY}" }
```

---
name: setup
description: 从零引导配置 quickai-gateway，包括 API Key、默认 Agent、运行模式（Solo/Multi-agent/Team）、Channel（Lark/DingTalk）。无需 fork 或重新编译。
---

# QuickAI Gateway 初始化引导

## 关于本 Skill

本 Skill 通过对话引导你完成 `~/.quickai/config.toml` 的创建和配置。
所有变更均为配置文件修改，**不需要 fork 项目，不需要重新编译**。

默认使用内置的 `quickai-rust-agent` 作为 AI 执行核心，无需安装任何额外 CLI 工具。

其他扩展 Skill：
- `/add-lark` — 添加飞书（Lark）Channel
- `/add-dingtalk` — 添加钉钉（DingTalk）Channel
- `/add-acp-backend` — 添加外部 ACP Agent（claude-code / codex / qwen 等）
- `/add-agent` — 向 agent_roster 追加新 Agent
- `/add-team-mode` — 为指定群组配置 Team 模式
- `/doctor` — 诊断和修复配置问题

---

## Phase 0：Pre-flight 检查

在开始前，先确认运行环境是否就绪。

### 0.1 检查 Binary 是否存在

运行以下命令，确认两个 binary 可以找到：

```bash
which quickai-gateway || ls ./target/release/quickai-gateway 2>/dev/null || ls ./target/debug/quickai-gateway 2>/dev/null
which quickai-rust-agent || ls ./target/release/quickai-rust-agent 2>/dev/null || ls ./target/debug/quickai-rust-agent 2>/dev/null
```

**如果找不到 binary**，需要先编译：

```bash
cargo build -p qai-server --bin quickai-gateway
cargo build -p quickai-rust-agent
```

编译完成后，建议把两个 binary 复制到 PATH：

```bash
cp target/debug/quickai-gateway ~/.local/bin/
cp target/debug/quickai-rust-agent ~/.local/bin/
# 确认 ~/.local/bin 在 PATH 中：
echo $PATH | tr ':' '\n' | grep -q "$HOME/.local/bin" && echo "✓ PATH OK" || echo "⚠ 请在 shell profile 中添加: export PATH=\"\$HOME/.local/bin:\$PATH\""
```

### 0.2 创建运行时目录

```bash
mkdir -p ~/.quickai/{sessions,shared,skills,personas}
echo "✓ 目录已创建"
```

### 0.3 检查是否已有配置

```bash
[ -f ~/.quickai/config.toml ] && echo "⚠ 已存在 config.toml，本次将覆盖" || echo "✓ 干净环境，将创建新配置"
```

> 如果已有配置且只想修改部分内容，请直接告诉我要修改什么，我会定点更新。

---

## Phase 1：API Key 配置

quickai-rust-agent 启动时从环境变量读取 API Key。

### 1.1 询问用户使用哪个 Provider

请告诉我你想使用哪个 AI Provider：

| 选项 | Provider | 环境变量 | 推荐模型 |
|------|----------|---------|---------|
| 1 | **Anthropic（Claude）** | `ANTHROPIC_API_KEY` | claude-sonnet-4-6 |
| 2 | **OpenAI（GPT）** | `OPENAI_API_KEY` | gpt-4o |
| 3 | **DeepSeek** | `OPENAI_API_KEY` + `OPENAI_API_BASE` | deepseek-chat |
| 4 | **其他 OpenAI 兼容端点** | `OPENAI_API_KEY` + `OPENAI_API_BASE` | 按服务商 |

### 1.2 收集 API Key

请提供你的 API Key（输入后我会写入配置，不会在任何地方展示原文）。

### 1.3 写入环境变量

将 API Key 写入 `~/.quickai/.env`（gateway 启动时可 source 这个文件）：

根据用户选择，写入对应内容：

**Anthropic**：
```bash
cat > ~/.quickai/.env << 'EOF'
export ANTHROPIC_API_KEY=<用户填写的key>
EOF
```

**OpenAI**：
```bash
cat > ~/.quickai/.env << 'EOF'
export OPENAI_API_KEY=<用户填写的key>
EOF
```

**DeepSeek**：
```bash
cat > ~/.quickai/.env << 'EOF'
export OPENAI_API_KEY=<用户填写的key>
export OPENAI_API_BASE=https://api.deepseek.com
export QUICKAI_MODEL=deepseek-chat
EOF
```

**其他 OpenAI 兼容**：
```bash
cat > ~/.quickai/.env << 'EOF'
export OPENAI_API_KEY=<用户填写的key>
export OPENAI_API_BASE=<用户填写的base url>
export QUICKAI_MODEL=<用户填写的model>
EOF
```

### 1.4 提示 shell 集成（可选）

询问用户：是否要把 `source ~/.quickai/.env` 加到 shell profile？

如果同意，检测 shell 并追加：
```bash
SHELL_PROFILE=""
case "$SHELL" in
  */zsh)  SHELL_PROFILE="$HOME/.zshrc" ;;
  */bash) SHELL_PROFILE="$HOME/.bashrc" ;;
esac
if [ -n "$SHELL_PROFILE" ]; then
  grep -q "quickai/.env" "$SHELL_PROFILE" 2>/dev/null \
    && echo "✓ 已存在，跳过" \
    || echo 'source ~/.quickai/.env' >> "$SHELL_PROFILE" && echo "✓ 已写入 $SHELL_PROFILE"
fi
```

---

## Phase 2：选择运行模式

询问用户要配置哪种模式：

```
请选择你的使用场景：

1. Solo（最简单）
   - 一个 AI Agent
   - 适合个人使用、快速上手
   - WebSocket 访问

2. Multi-agent（多 Agent 切换）
   - 多个命名 Agent，通过 @mention 切换
   - 不同 Agent 可以有不同 persona、workspace、backend
   - 适合区分"代码助手"/"写作助手"/"研究助手"

3. Team（多 Agent 协作）
   - Lead Agent 负责拆解任务、验收
   - Specialist Agent 并行执行子任务
   - 适合复杂工程任务、需要多专家协作的场景
   - 需要先配好多 Agent roster，再开启 Team 模式

建议顺序：先跑通 Solo，再升级到 Multi-agent，最后按需开 Team。
```

根据用户选择进入对应配置分支。

---

## Phase 3A：Solo 模式配置

### 3A.1 收集基本参数

询问：
- **工作目录**（默认可空，Agent 将在当前目录工作）：`/Users/xxx/work`
- **WebSocket Token**（用于 `/ws` 端点鉴权，可留空 = 开放模式）
- **监听端口**（默认 8080，0 = 随机端口）

### 3A.2 生成 config.toml

写入 `~/.quickai/config.toml`：

```toml
[gateway]
host = "127.0.0.1"
port = <用户选择的端口>
require_mention_in_groups = false
<如果有 default_workspace>default_workspace = "<用户填写的目录>"

[auth]
<如果有 ws_token>ws_token = "<用户填写的 token>"

[agent]
backend_id = "native-main"

[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "bundled_command"

[session]
dir = "/Users/<username>/.quickai/sessions"

[memory]
shared_dir = "/Users/<username>/.quickai/shared"
distill_every_n = 20
distiller_binary = "quickai-rust-agent"

[skills]
dir = "/Users/<username>/.quickai/skills"
```

---

## Phase 3B：Multi-agent 模式配置

### 3B.1 收集 Agent 信息

询问需要几个 Agent，并对每个 Agent 收集：
- **Agent 名称**（如 `claude`、`codex`、`researcher`）
- **触发 mention**（如 `@claude`、@代码助手）
- **Backend**（先都用 `native-main`，后续可用 `/add-acp-backend` 添加其他）
- **Workspace 目录**（可选）
- **Persona 目录**（可选，存放 SOUL.md/IDENTITY.md 等）

### 3B.2 是否需要 Binding（默认路由）

询问：某个 scope（如 Lark 群）默认走哪个 Agent？

如果需要，收集：
- scope 格式（如 `group:lark:chat-xxx`）
- 默认 Agent 名称

### 3B.3 生成 config.toml

```toml
[gateway]
host = "127.0.0.1"
port = <端口>
require_mention_in_groups = true

[auth]
<如有>ws_token = "<token>"

[[backend]]
id = "native-main"
family = "quick_ai_native"

[backend.launch]
type = "bundled_command"

[[agent_roster]]
name = "<agent1_name>"
mentions = ["<@mention1>"]
backend_id = "native-main"
<如有>persona_dir = "<路径>"
<如有>workspace_dir = "<路径>"

[[agent_roster]]
name = "<agent2_name>"
mentions = ["<@mention2>"]
backend_id = "native-main"
<如有>persona_dir = "<路径>"
<如有>workspace_dir = "<路径>"

<如有 binding>
[[binding]]
kind = "scope"
agent = "<默认agent名>"
scope = "<scope>"
channel = "<channel>"

[session]
dir = "/Users/<username>/.quickai/sessions"

[memory]
shared_dir = "/Users/<username>/.quickai/shared"
distill_every_n = 20
distiller_binary = "quickai-rust-agent"

[skills]
dir = "/Users/<username>/.quickai/skills"
```

---

## Phase 3C：Team 模式配置

Team 模式在 Multi-agent 基础上增加编排配置。

### 3C.1 先完成 Multi-agent 配置（3B 步骤）

至少需要：
- 1 个 Lead Agent（`front_bot`）
- 1+ 个 Specialist Agent（`roster`）

### 3C.2 选择 Team 作用范围

询问：Team 模式绑定到哪里？

**选项 A：群组（Lark/DingTalk group）**
- 需要 `[[group]]` + `scope = "group:lark:chat-xxx"`
- 群内消息驱动 Lead，Lead 分配给 Specialists

**选项 B：单聊个人工作台（DM scope）**
- 需要 `[[team_scope]]` + 精确 scope（如 `user:ou_xxxx`）
- 单聊也能享受 Team 编排能力

### 3C.3 收集 Team 参数

- **front_bot**：Lead Agent 名称（必须在 agent_roster 中）
- **team.roster**：Specialist Agent 名称列表
- **public_updates**：
  - `minimal` — 只发 Lead 显式回复（安静，推荐）
  - `normal` — 加上关键事件（blocked/failed/done）
  - `verbose` — 所有里程碑（调试用）
- **max_parallel**：最大并行任务数（默认 3）
- **auto_promote**（可选）：是否开启关键词自动升级到 Team 模式

### 3C.4 生成 Team 配置段

在 Multi-agent 配置基础上追加：

```toml
<Group 模式>
[[group]]
scope = "<group scope>"
name = "<group name>"

[group.mode]
interaction = "team"
front_bot = "<lead agent 名>"
channel = "<lark|dingtalk|ws>"
auto_promote = <true|false>

[group.team]
roster = ["<specialist1>", "<specialist2>"]
public_updates = "<minimal|normal|verbose>"
max_parallel = <N>
```

```toml
<Team Scope 模式（单聊）>
[[team_scope]]
scope = "<精确 scope>"
name = "<name>"

[team_scope.mode]
interaction = "team"
front_bot = "<lead agent 名>"
channel = "<channel>"

[team_scope.team]
roster = ["<specialist1>"]
public_updates = "minimal"
max_parallel = 2
```

---

## Phase 4：Channel 配置（可选）

询问是否需要接入 IM Channel：

```
是否接入消息 Channel？（Gateway 默认只有 WebSocket，可以接入 IM）

1. 飞书（Lark/Feishu）      → 运行 /add-lark
2. 钉钉（DingTalk）          → 运行 /add-dingtalk
3. 暂时不接，只用 WebSocket   → 跳过
```

如果用户选择接入 Channel，直接调用对应 skill：
- 选 1：提示用户"请运行 /add-lark 继续"
- 选 2：提示用户"请运行 /add-dingtalk 继续"

---

## Phase 5：写入配置文件

将上述所有配置合并，写入 `~/.quickai/config.toml`：

```bash
# 备份现有配置（如果存在）
[ -f ~/.quickai/config.toml ] && cp ~/.quickai/config.toml ~/.quickai/config.toml.bak.$(date +%Y%m%d%H%M%S) && echo "✓ 旧配置已备份"

# 写入新配置
cat > ~/.quickai/config.toml << 'TOMLEOF'
<根据 Phase 3 生成的完整 TOML 内容>
TOMLEOF
echo "✓ 配置已写入 ~/.quickai/config.toml"
```

---

## Phase 6：验证

### 6.1 先加载 env（当前 shell session）

```bash
source ~/.quickai/.env
echo "✓ 环境变量已加载"
```

### 6.2 语法校验

启动 gateway 做拓扑校验（不实际监听，遇错即退）：

```bash
quickai-gateway --validate-only 2>&1 | head -20
```

> 注意：如果 `--validate-only` 不支持，可以先启动 gateway 后立刻看启动日志。

### 6.3 启动 gateway 并验证

```bash
quickai-gateway &
GATEWAY_PID=$!
sleep 2

# 读取端口
PORT=$(cat ~/.quickai/gateway.port 2>/dev/null || echo "8080")
echo "Gateway 监听在 :$PORT"

# 健康检查
curl -s http://127.0.0.1:$PORT/health | python3 -m json.tool 2>/dev/null || curl -s http://127.0.0.1:$PORT/health
echo ""
curl -s http://127.0.0.1:$PORT/doctor | python3 -m json.tool 2>/dev/null || curl -s http://127.0.0.1:$PORT/doctor
echo ""

# 显示 Backend 状态
curl -s http://127.0.0.1:$PORT/diagnostics/backends 2>/dev/null

kill $GATEWAY_PID 2>/dev/null
```

### 6.4 快速功能测试（可选）

如果健康检查通过，可以发送一条测试消息验证 AI 是否工作：

```bash
source ~/.quickai/.env
quickai-gateway &
GATEWAY_PID=$!
sleep 2

PORT=$(cat ~/.quickai/gateway.port 2>/dev/null || echo "8080")
TOKEN=$(grep 'ws_token' ~/.quickai/config.toml 2>/dev/null | sed 's/.*= *"//' | sed 's/".*//')

# 如有 token，构造 auth header
AUTH_HEADER=""
[ -n "$TOKEN" ] && AUTH_HEADER="-H \"Authorization: Bearer $TOKEN\""

echo "正在通过 WebSocket 发送测试消息..."
echo '{"id":"test-1","session_key":{"channel":"ws","scope":"user:setup-test"},"content":{"type":"Text","text":"你好，请用一句话回复我"},"sender":"setup-test","channel":"ws","timestamp":"2026-01-01T00:00:00Z","thread_ts":null,"target_agent":null,"source":"human"}' | \
  websocat $AUTH_HEADER ws://127.0.0.1:$PORT/ws 2>/dev/null | head -5 || \
  echo "(websocat 未安装，跳过 WS 测试)"

kill $GATEWAY_PID 2>/dev/null
```

---

## Phase 7：完成与后续步骤

### 7.1 打印配置摘要

```
╔════════════════════════════════════════╗
║     QuickAI Gateway 配置完成           ║
╠════════════════════════════════════════╣
║ 配置文件: ~/.quickai/config.toml        ║
║ 运行模式: <solo|multi-agent|team>       ║
║ 默认 Backend: quickai-rust-agent        ║
║ Provider: <anthropic|openai|deepseek>  ║
║ Channel: <none|lark|dingtalk>          ║
╚════════════════════════════════════════╝
```

### 7.2 启动命令

```bash
# 每次启动前加载 env
source ~/.quickai/.env && quickai-gateway
```

### 7.3 后续扩展

```
可运行的其他 Skill：
  /add-lark           — 接入飞书
  /add-dingtalk       — 接入钉钉
  /add-acp-backend    — 添加 claude-code / codex / qwen 等外部 ACP Agent
  /add-agent          — 向 roster 追加新 Agent
  /add-team-mode      — 为群组开启 Team 模式
  /doctor             — 诊断配置问题
```

---

## 回滚

如果配置有误，恢复备份：

```bash
BACKUP=$(ls -t ~/.quickai/config.toml.bak.* 2>/dev/null | head -1)
[ -n "$BACKUP" ] && cp "$BACKUP" ~/.quickai/config.toml && echo "✓ 已恢复 $BACKUP" || echo "无备份可恢复"
```

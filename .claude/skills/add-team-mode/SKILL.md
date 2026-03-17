---
name: add-team-mode
description: 引导为群组或单聊启用 Team 模式（Lead + Specialists 协作），配置 [[group]] 或 [[team_scope]] 段。
---

# 启用 Team 模式

## 关于本 Skill

Team 模式让 QuickAI Gateway 支持多 Agent 协作：
- **Lead Agent**：接收用户消息，拆解任务，分配给 Specialists，验收结果
- **Specialist Agents**：并行执行子任务，汇报完成状态
- 适合复杂工程任务（大型代码修改、多步骤研究、跨文件重构等）

---

## Phase 0：前提确认

### 0.1 检查 Roster

```bash
echo "--- 当前 agent_roster ---"
grep -A 6 '\[\[agent_roster\]\]' ~/.quickai/config.toml 2>/dev/null || echo "⚠ 无 agent_roster"
```

**Team 模式要求**：
- 至少 **1 个 Lead Agent**（承担编排角色）
- 至少 **1 个 Specialist Agent**（执行子任务）

如果 roster 不够，提示：
```
当前 roster 中 Agent 数量不足以配置 Team 模式。
Team 模式最少需要 2 个 Agent（1 Lead + 1 Specialist）。
请先运行 /add-agent 添加更多 Agent。
```

### 0.2 检查是否已有 Team 配置

```bash
grep -q '\[\[group\]\]\|\[\[team_scope\]\]' ~/.quickai/config.toml 2>/dev/null && \
  echo "⚠ 已有 Team 配置，本次将追加新配置" || \
  echo "✓ 将新建 Team 配置"
```

---

## Phase 1：选择作用范围类型

询问用户：

```
Team 模式可以绑定到：

1. 群组（Group）— 绑定到飞书/钉钉群
   - 群里的消息触发 Lead Agent
   - Lead 在后台分配给 Specialists
   - 支持 auto_promote（关键词自动升级）

2. 单聊 Scope — 绑定到某个用户或 WebSocket session
   - 个人工作台也能享受 Team 编排
   - 适合开发者通过 WS 使用

3. 两者都配置
```

---

## Phase 1A：群组（Group）模式

### 1A.1 获取群 ID

**飞书群**：
```
飞书群的 scope 格式为：group:lark:<chat_id>
chat_id 格式：oc_xxxxxxxxxxxxxxxxxx

获取方式：
  - 飞书开发者后台 → 消息测试 → 选择群聊 → 查看 chat_id
  - 或让机器人发一条消息，在事件 payload 中查看 chat_id
```

**钉钉群**：
```
钉钉群的 scope 格式为：group:dingtalk:<conversationId>
conversationId 可以在收到的消息 payload 中找到。
```

**WebSocket 测试群**：
```
如果用 WebSocket 测试，可以使用任意字符串：
  group:ws:test-group-001
```

询问用户：群组的 scope 字符串是什么？

### 1A.2 收集 Team 参数

询问：
- **群组名称**（人类可读的描述，如 "研发组"、"产品群"）
- **Lead Agent 名称**（从 roster 中选择）
- **Specialist Agents**（从 roster 中选择，可多选）
- **Channel 类型**（`lark` / `dingtalk` / `ws`）

**关于 public_updates（通知详细程度）**：
```
选择群里看到的更新粒度：

  minimal  — 只有 Lead 明确回复用户时才发消息（最安静，推荐）
  normal   — 加上任务完成/阻塞/失败的关键事件
  verbose  — 所有里程碑都通知（调试时用）
```

**关于 max_parallel（最大并行任务数）**：
```
同时允许几个 Specialist 并发执行？（默认 3）
建议：
  - 2：保守，资源消耗低
  - 3：默认值，适合大多数场景
  - 5+：激进，适合大型任务但 API 成本高
```

**关于 auto_promote（自动升级到 Team 模式）**：
```
当 Lead 收到包含特定关键词的消息时，自动切换到 Team 模式？
（关键词如："实现"、"重构"、"开发"、"设计方案"）

false = 总是由用户明确请求 Team 模式（更可控）
true  = AI 自动判断是否需要 Team（更智能）
```

### 1A.3 生成配置段

```toml
[[group]]
scope = "<group-scope>"
name  = "<group-name>"

[group.mode]
interaction  = "team"
front_bot    = "<lead-agent-name>"
channel      = "<lark|dingtalk|ws>"
auto_promote = <true|false>

[group.team]
roster         = [<specialist-name-list>]
public_updates = "<minimal|normal|verbose>"
max_parallel   = <N>
```

---

## Phase 1B：单聊 Scope 模式

### 1B.1 获取 Scope

**飞书用户**：
```
飞书单聊 scope 格式：user:lark:<open_id>
open_id 格式：ou_xxxxxxxxxxxxxxxxxx

获取方式：在飞书事件 payload 的 sender.open_id 中找到
```

**WebSocket**：
```
WS scope 格式：user:ws:<任意字符串>
示例：user:ws:dev-local
```

询问用户：要绑定的 scope 字符串？

### 1B.2 收集参数

与群组类似，但通常配置更轻量：
- **Lead Agent**
- **Specialist Agents**
- **Channel 类型**
- **public_updates**（单聊推荐 `minimal`）
- **max_parallel**（默认 2）

### 1B.3 生成配置段

```toml
[[team_scope]]
scope = "<scope>"
name  = "<name>"

[team_scope.mode]
interaction = "team"
front_bot   = "<lead-agent-name>"
channel     = "<channel>"

[team_scope.team]
roster         = [<specialist-list>]
public_updates = "minimal"
max_parallel   = 2
```

---

## Phase 2：写入配置

```bash
# 备份
cp ~/.quickai/config.toml ~/.quickai/config.toml.bak.$(date +%Y%m%d%H%M%S)

# 追加 Team 配置
cat >> ~/.quickai/config.toml << 'TOMLEOF'

<生成的 [[group]] 或 [[team_scope]] 段>
TOMLEOF

echo "✓ Team 模式配置已写入"
```

---

## Phase 3：验证

### 3.1 检查配置

```bash
echo "--- 新增的 Team 配置 ---"
grep -A 12 '\[\[group\]\]\|\[\[team_scope\]\]' ~/.quickai/config.toml | tail -20
```

### 3.2 重启 Gateway 验证

```bash
source ~/.quickai/.env && quickai-gateway &
GATEWAY_PID=$!
sleep 2
PORT=$(cat ~/.quickai/gateway.port 2>/dev/null || echo "8080")

# 健康检查
curl -s http://127.0.0.1:$PORT/health | python3 -m json.tool 2>/dev/null

kill $GATEWAY_PID
```

### 3.3 功能测试建议

```
功能测试步骤：

1. 向 Lead Agent 发送一个需要多步骤的任务
   示例："请帮我分析这个项目的代码结构，并提出3个改进建议"

2. 观察 Lead 是否：
   - 拆解为子任务分配给 Specialists
   - 等待 Specialists 完成后汇总结果
   - 返回最终回复

3. 如果 verbose 模式，群里应该能看到进度更新
```

---

## Phase 4：完成确认

**群组模式**：
```
✓ Team 模式已为群组启用！

群组：<group-name>
Scope：<scope>
Lead：<lead-agent>
Specialists：<specialist-list>
并行上限：<max_parallel>
更新模式：<public_updates>

在群里向 @<lead-mention> 发送复杂任务，Team 就会开始协作。
```

**单聊模式**：
```
✓ Team 模式已为单聊 Scope 启用！

Scope：<scope>
Lead：<lead-agent>
Specialists：<specialist-list>

通过 WS 或飞书单聊发送复杂任务即可触发 Team 协作。
```

---

## 配置参考

```toml
# ─── 飞书群的 Team 模式 ────────────────────────────
[[group]]
scope = "group:lark:oc_xxxxxxxxxxxxxxxxxx"
name  = "研发群"

[group.mode]
interaction  = "team"
front_bot    = "lead"
channel      = "lark"
auto_promote = false

[group.team]
roster         = ["coder", "researcher", "reviewer"]
public_updates = "minimal"
max_parallel   = 3

# ─── WS 单聊的 Team 模式（本地测试）──────────────
[[team_scope]]
scope = "user:ws:dev-local"
name  = "本地开发工作台"

[team_scope.mode]
interaction = "team"
front_bot   = "lead"
channel     = "ws"

[team_scope.team]
roster         = ["coder", "reviewer"]
public_updates = "normal"
max_parallel   = 2
```

---

## 常见问题

**Q: Lead 分配了任务但 Specialists 不回复**
- 确认 Specialist agents 在 roster 中存在
- 检查 Backend 是否正常：`/doctor`

**Q: 任务一直在 pending 状态**
- 可能是 Backend 启动失败，查看 Gateway 日志
- 检查 API Key 是否有效

**Q: auto_promote = true 但没有自动触发 Team 模式**
- 检查消息内容是否包含触发关键词
- 关键词在 `mode_selector.rs` 中定义（默认包含"实现"、"开发"、"重构"等）

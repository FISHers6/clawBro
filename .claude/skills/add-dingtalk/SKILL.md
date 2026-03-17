---
name: add-dingtalk
description: 引导配置钉钉（DingTalk）Channel，包括企业应用创建、机器人配置、Webhook 接入和连通性验证。
---

# 添加钉钉（DingTalk）Channel

## 关于本 Skill

引导你完成钉钉 Channel 的接入配置。完成后，QuickAI Gateway 可以：
- 接收钉钉群消息（机器人 @mention）
- 接收钉钉单聊消息
- 回复群消息和单聊消息

---

## Phase 0：前提确认

### 0.1 确认 Gateway 已初始化

```bash
[ -f ~/.quickai/config.toml ] && echo "✓ config.toml 存在" || echo "⚠ 请先运行 /setup"
```

### 0.2 检查是否已有钉钉配置

```bash
grep -q '\[channels.dingtalk\]' ~/.quickai/config.toml 2>/dev/null && echo "⚠ 已有 DingTalk 配置，本次将更新" || echo "✓ 将新增 DingTalk 配置"
```

---

## Phase 1：钉钉应用准备

### 1.1 应用类型说明

```
钉钉接入有两种路径，请选择：

路径 A：企业内部应用（推荐，功能完整）
  - 需要企业钉钉管理员权限
  - 可以主动发送单聊/群消息
  - 支持完整的消息事件订阅

路径 B：自定义机器人（简单，仅群消息）
  - 任何人都可以创建
  - 只能在特定群里接收 @mention 消息
  - 通过群 Webhook 发送消息（不支持主动发起）
  - 适合快速测试

建议先用路径 B 测试，正式接入用路径 A。
```

询问用户选择哪条路径。

---

## Phase 1A：企业内部应用（路径 A）

### 1A.1 创建钉钉企业应用

```
1. 打开钉钉开放平台：https://open.dingtalk.com/
2. 进入「应用开发」→「企业内部开发」→「创建应用」
3. 选择「H5 微应用」或「小程序」中的「机器人」能力
4. 进入「基础信息」，获取：
   - AppKey（即 client_id）
   - AppSecret（即 client_secret）
5. 在「消息与事件接收」中配置：
   - 消息接收模式：HTTP 模式
   - 请求网址：http://<your-server>:<port>/channels/dingtalk/event
6. 订阅事件：
   - 用户发送消息  → message:biz:robot:interactive:1.0.0
7. 在「权限管理」中开通：
   - 消息通知 → 群会话 → 发送群消息
   - 消息通知 → 个人 → 发送单聊消息
```

### 1A.2 收集凭证

询问用户提供：
- **AppKey（client_id）**（格式：`dingxxxxxxxxxxxxxxxx`）
- **AppSecret（client_secret）**（64 位字符串）
- **AgentId**（应用的 AgentId，在「基础信息」页面）

---

## Phase 1B：自定义机器人（路径 B）

### 1B.1 创建群机器人

```
1. 打开目标钉钉群 → 右上角「...」→「智能群助手」
2. 点击「添加机器人」→「自定义」
3. 填写机器人名称
4. 安全设置：
   - 建议选择「自定义关键词」（如设置 "ai" 或 "@quickai"）
   - 或选择「加签」（更安全，gateway 需要配置 signing_secret）
5. 记录生成的 Webhook URL（格式：https://oapi.dingtalk.com/robot/send?access_token=xxxx）
```

### 1B.2 收集配置

询问用户提供：
- **Webhook URL**（完整 URL，包含 access_token）
- **Signing Secret**（如果选了「加签」安全方式，格式：`SECxxxxxxx`）

> 路径 B 只需配置 outbound webhook，无法接收消息事件（自定义机器人是单向的）。
> 要接收群 @mention 消息，需要使用路径 A 的企业应用。

---

## Phase 2：Bot 信息配置

### 2.1 Bot 名称（用于 @mention 识别）

询问：你的钉钉机器人名称是什么？（用于识别群里的 @mention）

### 2.2 默认 Agent 绑定（可选）

询问：收到钉钉消息时，默认路由给哪个 Agent？

```bash
grep -A 3 '\[\[agent_roster\]\]' ~/.quickai/config.toml 2>/dev/null || echo "（无 agent_roster 配置，将使用默认 backend）"
```

---

## Phase 3：写入配置

### 3.1 路径 A 配置段

```toml
[channels.dingtalk]
client_id     = "<AppKey>"
client_secret = "<AppSecret>"
agent_id      = <AgentId>
<如有>bot_name = "<Bot 名称>"
```

### 3.2 路径 B 配置段

```toml
[channels.dingtalk]
webhook_url   = "<群 Webhook URL>"
<如有>signing_secret = "<SECxxxxxxx>"
bot_name      = "<Bot 名称>"
```

### 3.3 追加到 config.toml

```bash
# 备份
cp ~/.quickai/config.toml ~/.quickai/config.toml.bak.$(date +%Y%m%d%H%M%S)

# 追加配置
cat >> ~/.quickai/config.toml << 'TOMLEOF'

[channels.dingtalk]
<根据路径 A 或 B 生成的配置>
TOMLEOF
echo "✓ DingTalk 配置已写入"
```

### 3.4 追加默认 Agent binding（如需要）

```bash
cat >> ~/.quickai/config.toml << 'TOMLEOF'

[[binding]]
kind    = "channel"
channel = "dingtalk"
agent   = "<默认 Agent 名>"
TOMLEOF
```

---

## Phase 4：验证配置

### 4.1 启动 Gateway

```bash
source ~/.quickai/.env && quickai-gateway &
GATEWAY_PID=$!
sleep 2
PORT=$(cat ~/.quickai/gateway.port 2>/dev/null || echo "8080")
echo "Gateway 监听在 :$PORT"
```

### 4.2 Webhook 地址

```
钉钉企业应用 Webhook 地址：
  http://<你的服务器IP>:$PORT/channels/dingtalk/event

请在钉钉开放平台「消息与事件接收」→「请求网址」中填写此地址。
```

### 4.3 连通性测试

```bash
# 测试 Gateway 是否正确响应钉钉验证请求
curl -s http://127.0.0.1:$PORT/health | grep -q '"status":"ok"' && echo "✓ Gateway 运行正常"

# 查看最近日志确认钉钉 channel 初始化
sleep 1
```

### 4.4 发送测试消息

路径 A — 在钉钉群中 @机器人 发送测试消息

路径 B — 可通过 Webhook 主动发送测试消息：
```bash
ACCESS_TOKEN="<从 webhook url 中提取>"
curl -s -X POST "https://oapi.dingtalk.com/robot/send?access_token=$ACCESS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"msgtype":"text","text":{"content":"QuickAI Gateway 连通性测试"}}'
```

---

## Phase 5：完成确认

```
✓ 钉钉 Channel 配置完成！

接入方式：<企业内部应用 | 自定义机器人>
Webhook 地址：http://<server>:<port>/channels/dingtalk/event

使用方式：
  - 群聊：@机器人名称 + 消息内容
  - 单聊：直接发送消息（路径 A 才支持单聊）

如需配置群组 Team 模式，请运行：/add-team-mode
如遇到问题，请运行：/doctor
```

---

## 配置参考（完整配置段）

**路径 A（企业应用）**：
```toml
[channels.dingtalk]
client_id     = "dingxxxxxxxxxxxxxxxxxxxx"
client_secret = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
agent_id      = 123456789
bot_name      = "QuickAI"
```

**路径 B（自定义机器人）**：
```toml
[channels.dingtalk]
webhook_url    = "https://oapi.dingtalk.com/robot/send?access_token=xxxxxxxx"
signing_secret = "SECxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
bot_name       = "QuickAI"
```

---

## 常见问题

**Q: 企业应用无法收到消息**
- 确认 Webhook URL 已在开放平台配置且验证通过
- 确认应用已发布，且机器人已添加到对应群聊
- 检查 Gateway 日志

**Q: 自定义机器人只能发消息，无法收消息**
- 这是钉钉自定义机器人的限制（单向），需要路径 A 才能双向

**Q: 签名验证失败**
- 确认 `signing_secret` 以 `SEC` 开头
- 检查服务器时钟是否准确（签名包含时间戳，误差 > 1 小时会失败）：`date`

---
name: add-lark
description: 引导配置飞书（Lark/Feishu）Channel，包括 App 创建、Webhook 配置、config.toml 写入和连通性验证。
---

# 添加飞书（Lark / Feishu）Channel

## 关于本 Skill

引导你完成飞书 Channel 的接入配置。完成后，ClawBro Gateway 可以：
- 接收飞书群消息 / 单聊消息
- 回复飞书消息
- 在群里被 @mention 触发

---

## Phase 0：前提确认

### 0.1 确认 Gateway 已初始化

```bash
[ -f ~/.clawbro/config.toml ] && echo "✓ config.toml 存在" || echo "⚠ 请先运行 /setup"
```

如果 config.toml 不存在，请先运行 `/setup`。

### 0.2 检查是否已有飞书配置

```bash
grep -q '\[channels.lark\]' ~/.clawbro/config.toml 2>/dev/null && echo "⚠ 已有 Lark 配置，本次将更新" || echo "✓ 将新增 Lark 配置"
```

---

## Phase 1：飞书应用准备

### 1.1 告知用户需要创建飞书自建应用

```
需要一个飞书自建应用（Internal App）来获取 App ID 和 App Secret。

如果你还没有飞书应用，请按以下步骤操作：

1. 打开飞书开发者后台：https://open.feishu.cn/app
2. 点击「创建企业自建应用」
3. 填写应用名称（如 ClawBro）和描述
4. 进入「凭证与基础信息」页面，找到 App ID 和 App Secret

需要开通的权限（API 范围）：
  ✓ 读取用户发给机器人的单聊消息  → im:message.receive_v1 (event)
  ✓ 接收群消息（需机器人在群内）  → im:message (event)
  ✓ 发送消息                    → im:message:send_as_bot
  ✓ 获取 open_id 等用户信息      → contact:user.base:readonly

订阅事件：
  ✓ 接收消息  → im.message.receive_v1

Webhook URL（等 Gateway 启动后填写）：
  http://<your-server>:<port>/channels/lark/event
```

### 1.2 收集 App 凭证

询问用户提供：

- **App ID**（格式：`cli_xxxxxxxxxxxx`）
- **App Secret**（40 位字母数字字符串）
- **Verification Token**（飞书事件订阅页面中的「Verification Token」，用于验证 Webhook 请求合法性）

> Verification Token 在飞书开发者后台「事件订阅」→「加密策略」处获取。

### 1.3 可选：加密配置

询问是否启用飞书事件加密（Encrypt Key）：

```
飞书支持对 Webhook 消息进行加密（可选）。

如果你在飞书后台「事件订阅」→「加密策略」中设置了 Encrypt Key，
请提供（否则留空，gateway 将不使用加密）。
```

---

## Phase 2：收集运行参数

### 2.1 Bot 名称（用于 @mention 识别）

询问：你的飞书机器人名称是什么？（用于识别群里的 @mention，不填则无法接受群消息触发）

常见格式：
- `ClawBro`
- `AI助手`
- `智能体`

### 2.2 Bot Open ID（可选，用于精确识别 @mention）

```bash
# 如果知道 Bot 的 open_id，可以直接配置（更精确）
# 格式：ou_xxxxxxxxxx
# 获取方式：飞书后台「开发配置」→「权限管理」→「获取 Bot OpenID」
```

询问：是否有 Bot 的 open_id？（没有可以跳过，gateway 会用 bot 名称进行模糊匹配）

### 2.3 默认 Agent 绑定（可选）

询问：收到飞书消息时，默认路由给哪个 Agent？

```bash
# 查看已有 agent_roster
grep -A 3 '\[\[agent_roster\]\]' ~/.clawbro/config.toml 2>/dev/null || echo "（无 agent_roster 配置，将使用 [agent] backend_id）"
```

---

## Phase 3：写入配置

### 3.1 生成 Lark Channel 配置段

根据收集的信息生成：

```toml
[channels.lark]
app_id              = "<App ID>"
app_secret          = "<App Secret>"
verification_token  = "<Verification Token>"
<如有>encrypt_key  = "<Encrypt Key>"
<如有>bot_name     = "<Bot 名称>"
<如有>bot_open_id  = "<Bot open_id>"
```

### 3.2 追加到 config.toml

**注意**：如果已有 `[channels.lark]` 段，先移除再追加。

```bash
# 备份
cp ~/.clawbro/config.toml ~/.clawbro/config.toml.bak.$(date +%Y%m%d%H%M%S)

# 检测并移除旧 lark 配置段（如有）
# 然后追加新配置
```

执行写入操作（追加到文件末尾）：

```bash
cat >> ~/.clawbro/config.toml << 'TOMLEOF'

[channels.lark]
app_id             = "<App ID>"
app_secret         = "<App Secret>"
verification_token = "<Verification Token>"
<如有 encrypt_key>encrypt_key        = "<Encrypt Key>"
<如有 bot_name>bot_name           = "<Bot 名称>"
TOMLEOF
echo "✓ Lark 配置已写入"
```

### 3.3 如有默认 Agent 绑定，追加 binding

```bash
cat >> ~/.clawbro/config.toml << 'TOMLEOF'

[[binding]]
kind    = "channel"
channel = "lark"
agent   = "<默认 Agent 名>"
TOMLEOF
```

---

## Phase 4：验证配置

### 4.1 启动 gateway

```bash
source ~/.clawbro/.env && clawbro-gateway &
GATEWAY_PID=$!
sleep 2

PORT=$(cat ~/.clawbro/gateway.port 2>/dev/null || grep 'port' ~/.clawbro/config.toml | head -1 | grep -o '[0-9]*' || echo "8080")
echo "Gateway 监听在 :$PORT"
```

### 4.2 飞书 Webhook 地址

```
你的飞书 Webhook 地址为：
  http://<你的服务器IP>:$PORT/channels/lark/event

请在飞书开发者后台「事件订阅」→「请求网址配置」中填写此地址。
飞书会向此地址发送一次验证请求，Gateway 会自动响应 challenge。
```

询问用户：
1. 服务器是否有公网 IP？
2. 如果是本地测试，是否需要使用 ngrok / frp 暴露内网端口？

如果需要 ngrok 帮助：
```bash
# 安装 ngrok（如未安装）
# brew install ngrok/ngrok/ngrok

# 暴露本地端口
ngrok http $PORT
# 使用 ngrok 生成的 https URL 填写到飞书 Webhook 配置
```

### 4.3 Webhook 验证测试

```bash
# 检查 Gateway 是否正确响应 /channels/lark/event
curl -s -X POST http://127.0.0.1:$PORT/channels/lark/event \
  -H "Content-Type: application/json" \
  -d '{"challenge":"test-challenge-123","token":"<verification_token>","type":"url_verification"}' | \
  python3 -m json.tool 2>/dev/null

# 预期响应：{"challenge":"test-challenge-123"}
```

如果响应正确，提示用户在飞书开发者后台完成 Webhook URL 验证。

### 4.4 发布应用

```
验证通过后，请在飞书开发者后台：
1. 点击「版本管理与发布」
2. 创建新版本并提交审核（内部应用通常无需审核，直接发布）
3. 将机器人添加到目标群聊

添加机器人到群：
  打开飞书群 → 右上角「...」→「群机器人」→「添加机器人」→ 选择你的应用
```

---

## Phase 5：功能确认

### 5.1 结束语

```
✓ 飞书 Channel 配置完成！

Webhook 地址：http://<server>:<port>/channels/lark/event
验证状态：<已验证 / 待验证>

在飞书群中 @你的机器人 就可以和 ClawBro 对话了。

单聊：直接发送消息给机器人（无需 @）

如需配置群组 Team 模式，请运行：/add-team-mode
如遇到问题，请运行：/doctor
```

---

## 配置参考（完整 Lark 配置段）

```toml
[channels.lark]
# 必须字段
app_id             = "cli_xxxxxxxxxxxx"
app_secret         = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
verification_token = "xxxxxxxxxxxxxxxxxxxxxxxx"

# 可选字段
encrypt_key        = "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"  # 如果飞书后台配置了加密
bot_name           = "ClawBro"                           # 机器人名称（群消息 @mention 识别）
bot_open_id        = "ou_xxxxxxxxxxxxxxxxxxxxxxxxxx"    # 机器人 open_id（精确匹配）
```

---

## 常见问题

**Q: 收到 `invalid verification_token` 错误**
- 检查 `verification_token` 是否填写正确（注意不是 App Secret）

**Q: 群消息无响应**
- 确认机器人已被添加到群聊
- 确认 `require_mention_in_groups = true` 时需要 @mention 机器人

**Q: 单聊无响应**
- 确认已开通 `im:message.receive_v1` 权限并订阅事件
- 检查 Gateway 日志：`tail -f ~/.clawbro/gateway.log`

**Q: Webhook URL 验证失败**
- 确认 Gateway 已在运行：`curl http://127.0.0.1:<port>/health`
- 确认 Webhook URL 可从公网访问（考虑使用 ngrok）

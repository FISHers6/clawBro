---
name: doctor
description: 诊断 ClawBro Gateway 配置和运行问题，逐项检查并给出修复建议。
---

# ClawBro Gateway 诊断（Doctor）

## 关于本 Skill

自动诊断常见问题，包括：
- Binary 是否存在
- 配置文件语法和完整性
- API Key 是否设置
- Channel 配置是否正确
- Backend 是否可以启动
- Gateway 运行时健康状态

---

## Phase 0：快速状态检查

先运行一个综合检查，快速看清楚问题所在：

```bash
echo "===== ClawBro Gateway Doctor ====="
echo ""

# 1. Binary 检查
echo "[1] Binary 检查"
which clawbro-gateway 2>/dev/null && echo "  ✓ clawbro-gateway: $(which clawbro-gateway)" || echo "  ✗ clawbro-gateway 未找到（请检查 PATH 或重新编译）"
which clawbro-rust-agent 2>/dev/null && echo "  ✓ clawbro-rust-agent: $(which clawbro-rust-agent)" || echo "  ✗ clawbro-rust-agent 未找到"
echo ""

# 2. 配置文件检查
echo "[2] 配置文件检查"
[ -f ~/.clawbro/config.toml ] && echo "  ✓ ~/.clawbro/config.toml 存在" || echo "  ✗ ~/.clawbro/config.toml 不存在（运行 /setup 创建）"
echo ""

# 3. 环境变量检查
echo "[3] API Key 检查"
[ -n "$ANTHROPIC_API_KEY" ] && echo "  ✓ ANTHROPIC_API_KEY 已设置" || echo "  - ANTHROPIC_API_KEY 未设置"
[ -n "$OPENAI_API_KEY" ] && echo "  ✓ OPENAI_API_KEY 已设置" || echo "  - OPENAI_API_KEY 未设置"
[ -f ~/.clawbro/.env ] && echo "  ✓ ~/.clawbro/.env 存在" || echo "  - ~/.clawbro/.env 不存在"
echo ""

# 4. 目录检查
echo "[4] 运行时目录检查"
for dir in ~/.clawbro ~/.clawbro/sessions ~/.clawbro/shared ~/.clawbro/skills ~/.clawbro/personas; do
  [ -d "$dir" ] && echo "  ✓ $dir" || echo "  ✗ $dir 不存在（运行: mkdir -p $dir）"
done
echo ""

# 5. Gateway 进程检查
echo "[5] Gateway 进程检查"
pgrep -l clawbro-gateway 2>/dev/null && echo "  ✓ Gateway 正在运行" || echo "  - Gateway 未运行"
echo ""

echo "=============================="
```

---

## Phase 1：配置文件深度诊断

### 1.1 读取配置

```bash
if [ ! -f ~/.clawbro/config.toml ]; then
  echo "✗ 配置文件不存在，请先运行 /setup"
  exit 1
fi

echo "--- 配置文件内容 ---"
cat ~/.clawbro/config.toml
echo "---"
```

### 1.2 逐项检查关键字段

**检查 [gateway] 段**：
```bash
echo "[gateway] 检查"
grep -q '\[gateway\]' ~/.clawbro/config.toml && echo "  ✓ [gateway] 段存在" || echo "  ✗ 缺少 [gateway] 段"
grep 'port' ~/.clawbro/config.toml | head -1 | grep -q '[0-9]' && echo "  ✓ port 已配置" || echo "  - port 未配置（将使用默认值 8080）"
```

**检查 Backend 配置**：
```bash
echo "[backend] 检查"
BACKEND_COUNT=$(grep -c '\[\[backend\]\]' ~/.clawbro/config.toml 2>/dev/null || echo "0")
echo "  已配置 $BACKEND_COUNT 个 backend"
if [ "$BACKEND_COUNT" -eq 0 ]; then
  echo "  ✗ 未配置任何 backend（运行 /setup 或手动添加 [[backend]]）"
fi
grep -A 3 '\[\[backend\]\]' ~/.clawbro/config.toml
```

**检查 Agent 配置**：
```bash
echo "[agent / agent_roster] 检查"
grep -q '\[agent\]' ~/.clawbro/config.toml && echo "  ✓ [agent] solo 配置存在" || true
ROSTER_COUNT=$(grep -c '\[\[agent_roster\]\]' ~/.clawbro/config.toml 2>/dev/null || echo "0")
echo "  已配置 $ROSTER_COUNT 个 agent_roster 条目"
```

**检查 Channel 配置**：
```bash
echo "[channels] 检查"
grep -q '\[channels.lark\]' ~/.clawbro/config.toml && echo "  ✓ Lark channel 已配置" || echo "  - Lark channel 未配置"
grep -q '\[channels.dingtalk\]' ~/.clawbro/config.toml && echo "  ✓ DingTalk channel 已配置" || echo "  - DingTalk channel 未配置"
```

---

## Phase 2：运行时诊断

### 2.1 如果 Gateway 未运行，启动并检查

```bash
# 先加载 env
[ -f ~/.clawbro/.env ] && source ~/.clawbro/.env

# 检查是否已运行
if pgrep clawbro-gateway > /dev/null; then
  echo "✓ Gateway 已在运行"
  PORT=$(cat ~/.clawbro/gateway.port 2>/dev/null || echo "8080")
else
  echo "正在临时启动 Gateway 进行诊断..."
  clawbro-gateway > /tmp/clawbro-doctor.log 2>&1 &
  GATEWAY_PID=$!
  sleep 3

  if ! kill -0 $GATEWAY_PID 2>/dev/null; then
    echo "✗ Gateway 启动失败，查看日志："
    cat /tmp/clawbro-doctor.log
    echo ""
    echo "常见原因："
    echo "  1. config.toml 语法错误"
    echo "  2. 端口被占用（尝试修改 port 配置）"
    echo "  3. API Key 未设置"
    exit 1
  fi
  PORT=$(cat ~/.clawbro/gateway.port 2>/dev/null || echo "8080")
fi

echo "Gateway 监听在 :$PORT"
```

### 2.2 HTTP 健康检查

```bash
echo ""
echo "--- /health ---"
curl -s "http://127.0.0.1:$PORT/health" | python3 -m json.tool 2>/dev/null || curl -s "http://127.0.0.1:$PORT/health"

echo ""
echo "--- /doctor ---"
curl -s "http://127.0.0.1:$PORT/doctor" | python3 -m json.tool 2>/dev/null || curl -s "http://127.0.0.1:$PORT/doctor"

echo ""
echo "--- /diagnostics/backends ---"
curl -s "http://127.0.0.1:$PORT/diagnostics/backends" 2>/dev/null | python3 -m json.tool 2>/dev/null || echo "(端点不存在或无响应)"
```

### 2.3 查看最近日志

```bash
echo ""
echo "--- 最近日志（最后 30 行）---"
[ -f ~/.clawbro/gateway.log ] && tail -30 ~/.clawbro/gateway.log || \
  [ -f /tmp/clawbro-doctor.log ] && tail -30 /tmp/clawbro-doctor.log || \
  echo "（无日志文件，请检查 Gateway 是否已将日志输出到文件）"
```

### 2.4 清理临时进程

```bash
[ -n "$GATEWAY_PID" ] && kill $GATEWAY_PID 2>/dev/null && echo "诊断完成，已停止临时 Gateway 进程"
```

---

## Phase 3：常见问题诊断树

根据检查结果，识别问题类型并给出针对性建议。

### 问题：Gateway 无法启动

```
可能原因（按概率排序）：
  1. config.toml 语法错误
     检查：cat ~/.clawbro/config.toml | python3 -c "import sys; import tomllib; tomllib.load(sys.stdin.buffer)" 2>&1
     修复：手动编辑修正 TOML 语法

  2. 端口已被占用
     检查：lsof -i :<port> 或 ss -tlnp | grep <port>
     修复：修改 ~/.clawbro/config.toml 中的 port 值

  3. clawbro-rust-agent binary 不在 PATH
     检查：which clawbro-rust-agent
     修复：cp target/release/clawbro-rust-agent ~/.local/bin/

  4. API Key 未设置
     检查：echo $ANTHROPIC_API_KEY
     修复：source ~/.clawbro/.env
```

### 问题：收不到 IM 消息

```
飞书：
  1. Webhook URL 是否在飞书后台配置并通过验证？
     测试：curl -X POST http://127.0.0.1:<port>/channels/lark/event \
               -d '{"challenge":"test","token":"<token>","type":"url_verification"}'
     期望：{"challenge":"test"}

  2. 服务器是否可公网访问？
     测试：curl https://ifconfig.me（查看公网 IP）
     考虑：使用 ngrok 做内网穿透

  3. 飞书应用是否已发布？机器人是否在群里？

钉钉：
  1. 企业应用：检查 client_id / client_secret 是否正确
  2. 自定义机器人：注意自定义机器人无法接收消息（单向）
  3. 签名验证：检查服务器时钟是否准确（date）
```

### 问题：AI 不回复消息

```
1. 检查 API Key 是否有效
   Anthropic：curl -H "x-api-key: $ANTHROPIC_API_KEY" https://api.anthropic.com/v1/models
   OpenAI：curl -H "Authorization: Bearer $OPENAI_API_KEY" https://api.openai.com/v1/models

2. 检查 Backend 是否可以启动
   native-main：which clawbro-rust-agent && clawbro-rust-agent --help 2>&1 | head -5
   claude-code：claude --version
   codex：codex --version

3. 查看 Gateway 日志中的 ERROR 行
   grep -i error ~/.clawbro/gateway.log | tail -20

4. 检查会话文件
   ls ~/.clawbro/sessions/
```

### 问题：Team 模式不工作

```
1. 检查 [[group]] 配置中的 scope 是否匹配实际群 ID
   飞书群 ID 在消息 payload 的 chat_id 字段中

2. 检查 front_bot 和 roster 中的 agent 名称是否都在 [[agent_roster]] 中存在
   grep 'name = ' ~/.clawbro/config.toml | grep -v '#'

3. 确认 channel 字段与实际使用的 channel 一致（lark/dingtalk/ws）

4. 查看 team 目录（如果存在）：
   ls ~/.clawbro/sessions/team/*/
```

---

## Phase 4：修复建议汇总

根据上述诊断，给出具体的修复步骤列表：

```
诊断报告
========

状态：<整体状态 OK / 有问题>

发现的问题：
  1. [类型] 问题描述 → 修复命令/操作

可选的下一步：
  /setup        — 重新完整配置
  /add-lark     — 重新配置飞书
  /add-dingtalk — 重新配置钉钉
```

---

## 快速修复命令参考

```bash
# 重新加载 env
source ~/.clawbro/.env

# 修复目录权限
mkdir -p ~/.clawbro/{sessions,shared,skills,personas}
chmod 700 ~/.clawbro

# 查找并杀掉已有 gateway 进程
pkill clawbro-gateway

# 测试 API Key（Anthropic）
curl -s -o /dev/null -w "%{http_code}" \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "anthropic-version: 2023-06-01" \
  https://api.anthropic.com/v1/models

# 回滚配置
BACKUP=$(ls -t ~/.clawbro/config.toml.bak.* 2>/dev/null | head -1)
[ -n "$BACKUP" ] && cp "$BACKUP" ~/.clawbro/config.toml && echo "已恢复 $BACKUP"

# 检查端口占用
lsof -i :8080 2>/dev/null || ss -tlnp | grep 8080
```

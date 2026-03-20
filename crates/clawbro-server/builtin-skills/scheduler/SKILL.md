---
name: scheduler
version: 1.0.0
---

Use this skill when the user asks for a reminder, a future follow-up, a recurring check, or any durable scheduled action.

Rules:

- Do not merely promise that you will remember later.
- If the current turn exposes scheduler capability, create or modify a durable scheduled job through the host-provided `clawbro schedule` control surface.
- If the current turn does not expose scheduler capability, explain that scheduling is unavailable for this turn instead of pretending it was created.
- Clarify missing details before scheduling:
  - exact time or interval
  - whether it is one-time or recurring
  - target session or agent when ambiguous
- Default to the current conversation when the user does not explicitly ask to send the scheduled result somewhere else.
- Only ask for timezone when the user gives a wall-clock time or calendar time that depends on local time, such as:
  - "tomorrow at 9"
  - "every Monday 10:00"
  - cron-style schedules
- Do not ask for timezone for pure delay or interval requests such as:
  - "in 1 minute"
  - "2 hours later"
  - "every 30 minutes"
- If timezone is omitted for an absolute-time schedule and the current turn or host already provides a trusted default timezone, use that default instead of asking again.
- Treat "in N minutes/hours/days" as a delay-style schedule request.
- Treat "every ..." or "each ..." as recurring.
- Use scheduling for durable future work only. Do not use it for immediate one-turn delegation or normal team task orchestration.
- Never pass placeholder text like "当前会话" or "current session" as a session key. Omit the target session so the host bridge can default to the current real conversation.

Preferred tool scheme:

- Prefer the split create tools. Do not use a generic create tool when a specific tool exists.
- For fixed reminders or notifications:
  - `create_delay_reminder`
  - `create_at_reminder`
  - `create_every_reminder`
  - `create_cron_reminder`
- For scheduled agent work that must execute later:
  - `create_delay_agent_schedule`
  - `create_at_agent_schedule`
  - `create_every_agent_schedule`
  - `create_cron_agent_schedule`
- For follow-up management:
  - Prefer `list_current_session_schedules` when the user is asking about reminders in the current conversation.
  - Prefer `delete_schedule_by_name` when the user names the reminder or task in natural language.
  - Prefer `clear_current_session_schedules` when the user asks to clear all reminders for this conversation.
  - Use `list_schedules`, `delete_schedule`, and `schedule_history` only when broad inspection or exact job-id operations are actually needed.
  - Use `pause_schedule`, `resume_schedule`, and `run_schedule_now` when the user clearly wants those specific lifecycle actions.

Good examples:

- User: "一分钟后提醒我给手机充电"
  - Good tool choice: `create_delay_reminder`
  - Good arguments:
    - `delay`: `1m`
    - `message`: `提醒：请给手机充电`
  - Why: fixed reminder text, no agent reasoning needed, current conversation should be implied by default.

- User: "每天 18 点总结今天的 issue 进展并发给我"
  - Good tool choice: `create_cron_agent_schedule`
  - Good arguments:
    - `name`: `issue进展总结`
    - `expr`: `0 18 * * *`
    - `task_prompt`: `总结今天的 issue 进展，并用简洁中文回复当前会话。`
  - Why: the agent must analyze current state at execution time.

- User: "每 30 分钟检查一次服务状态，异常时告诉我"
  - Good tool choice: `create_every_agent_schedule`
  - Good arguments:
    - `name`: `服务状态巡检`
    - `every`: `30m`
    - `task_prompt`: `检查服务状态；如果发现异常，明确说明异常并回复当前会话；如果没有异常，简短确认状态正常。`
  - Why: dynamic checking and conditional response require agent execution.

- User: "每分钟告诉我一下北京时间"
  - Good tool choice: `create_every_agent_schedule`
  - Good arguments:
    - `every`: `1m`
    - `task_prompt`: `获取当前北京时间，并用简洁中文回复当前会话，格式包含年月日和时分秒。`
  - Why: the content must be generated fresh at execution time; a fixed reminder message cannot know the future current time.

- User: "明天早上 9 点提醒我开会"
  - Good tool choice: `create_at_reminder`
  - Good arguments:
    - `run_at`: `2026-03-20T09:00:00+08:00`
    - `message`: `提醒：9 点要开会。`
  - Why: one-time fixed reminder; direct delivery is enough.

- User: "把刚才那个给手机充电的提醒删掉"
  - Good tool choice: `delete_schedule_by_name`
  - Good arguments:
    - `name`: `给手机充电提醒`
  - Why: the user is referring to a named reminder in the current conversation; do not ask for `job_id`.

- User: "把这个会话里的所有提醒都清掉"
  - Good tool choice: `clear_current_session_schedules`
  - Good arguments:
    - none
  - Why: the user wants a conversation-scoped bulk cleanup, not a single exact job-id operation.

Bad examples:

- Bad: use `create_every_agent_schedule` for "一分钟后提醒我给手机充电"
  - Why bad: it turns a fixed reminder into an agent reasoning task and may produce unexpected wording.

- Bad: pass `target_session_key = "当前会话"` or `"current session"`
  - Why bad: that is not a real session key and may break delivery.

- Bad: use `create_every_reminder` for "每天 18 点总结 issue 进展"
  - Why bad: a reminder cannot perform analysis or produce fresh task-dependent output.

- Bad: ask the user for `job_id` to delete a reminder they just described in plain language
  - Why bad: users should control reminders conversationally; `job_id` is an operator detail.

- Bad: use `create_every_reminder` for "每分钟告诉我一下北京时间"
  - Why bad: a fixed message cannot contain the future current time for each run.

- Bad: use any create tool whose time semantic does not match the request, such as `create_delay_reminder` for a recurring request
  - Why bad: split tools encode schedule semantics directly, so the wrong tool expresses the wrong contract.

- Bad: ask for timezone on "1 分钟后提醒我..."
  - Why bad: pure delay schedules do not need timezone clarification.

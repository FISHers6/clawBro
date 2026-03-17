use crate::cli::args::LangArg;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language { Zh, En, Ja, Ko }

impl Language {
    pub fn from_arg(arg: Option<&LangArg>) -> Self {
        match arg {
            Some(LangArg::En) => Language::En,
            Some(LangArg::Ja) => Language::Ja,
            Some(LangArg::Ko) => Language::Ko,
            _ => Language::Zh,
        }
    }
}

pub struct Messages {
    pub welcome: &'static str,
    pub select_provider: &'static str,
    pub enter_api_key: &'static str,
    pub enter_api_key_hint: &'static str,
    pub enter_api_base: &'static str,
    pub enter_model: &'static str,
    pub select_mode: &'static str,
    pub mode_solo: &'static str,
    pub mode_multi: &'static str,
    pub mode_team: &'static str,
    pub enter_port: &'static str,
    pub enter_workspace: &'static str,
    pub enter_ws_token: &'static str,
    pub enter_ws_token_hint: &'static str,
    pub select_channel: &'static str,
    pub channel_none: &'static str,
    pub channel_lark: &'static str,
    pub channel_dingtalk: &'static str,
    pub enter_lark_app_id: &'static str,
    pub enter_lark_app_secret: &'static str,
    pub enter_lark_verify_token: &'static str,
    pub enter_lark_bot_name: &'static str,
    pub enter_dingtalk_client_id: &'static str,
    pub enter_dingtalk_client_secret: &'static str,
    pub enter_dingtalk_agent_id: &'static str,
    pub enter_dingtalk_bot_name: &'static str,
    pub confirm_write: &'static str,
    pub written_config: &'static str,
    pub written_env: &'static str,
    pub backed_up: &'static str,
    pub done: &'static str,
    pub next_steps: &'static str,
}

impl Messages {
    pub fn for_lang(lang: Language) -> &'static Self {
        match lang {
            Language::Zh => &ZH,
            Language::En => &EN,
            Language::Ja => &JA,
            Language::Ko => &KO,
        }
    }
}

static ZH: Messages = Messages {
    welcome: "════════════════════════════════════\n  QuickAI Gateway 初始化向导\n════════════════════════════════════",
    select_provider: "选择 AI Provider",
    enter_api_key: "请输入 API Key",
    enter_api_key_hint: "（输入内容不会显示）",
    enter_api_base: "自定义 API Base URL（可选，留空使用默认）",
    enter_model: "模型名称（留空使用默认值）",
    select_mode: "选择运行模式",
    mode_solo: "Solo — 单 Agent，适合个人使用",
    mode_multi: "Multi-agent — 多 Agent，通过 @mention 切换",
    mode_team: "Team — Lead + Specialists 编排协作",
    enter_port: "Gateway 监听端口（0 = 随机，默认 8080）",
    enter_workspace: "默认工作目录（留空 = 不设置）",
    enter_ws_token: "WebSocket 认证 Token（留空 = 开放模式，无需鉴权）",
    enter_ws_token_hint: "建议生产环境设置 Token 保护 /ws 端点",
    select_channel: "接入 IM Channel（可选）",
    channel_none: "暂不接入，只用 WebSocket",
    channel_lark: "飞书 / Feishu / Lark",
    channel_dingtalk: "钉钉 / DingTalk",
    enter_lark_app_id: "飞书 App ID（格式: cli_xxxx）",
    enter_lark_app_secret: "飞书 App Secret",
    enter_lark_verify_token: "飞书 Verification Token",
    enter_lark_bot_name: "Bot 名称（群消息 @mention 识别，可留空）",
    enter_dingtalk_client_id: "钉钉 AppKey / client_id",
    enter_dingtalk_client_secret: "钉钉 AppSecret / client_secret",
    enter_dingtalk_agent_id: "AgentId（数字，钉钉开放平台基础信息页，可留空）",
    enter_dingtalk_bot_name: "Bot 名称（可留空）",
    confirm_write: "确认写入配置文件？",
    written_config: "✓ 配置已写入 ~/.quickai/config.toml",
    written_env: "✓ API Key 已写入 ~/.quickai/.env",
    backed_up: "✓ 旧配置已备份",
    done: "🎉 初始化完成！",
    next_steps: "启动 Gateway：\n  source ~/.quickai/.env && quickai serve\n\n其他命令：\n  quickai doctor      — 诊断问题\n  quickai status      — 查看配置\n  quickai auth list   — 查看 API Key\n  quickai setup       — 重新配置",
};

static EN: Messages = Messages {
    welcome: "════════════════════════════════════\n  QuickAI Gateway Setup Wizard\n════════════════════════════════════",
    select_provider: "Select AI Provider",
    enter_api_key: "Enter API Key",
    enter_api_key_hint: "(input is hidden)",
    enter_api_base: "Custom API Base URL (optional, leave empty for default)",
    enter_model: "Model name (leave empty for default)",
    select_mode: "Select operation mode",
    mode_solo: "Solo — single agent, great for personal use",
    mode_multi: "Multi-agent — multiple agents, switch via @mention",
    mode_team: "Team — Lead + Specialists for complex tasks",
    enter_port: "Gateway port (0 = random, default 8080)",
    enter_workspace: "Default workspace directory (leave empty to skip)",
    enter_ws_token: "WebSocket auth token (leave empty = open mode, no auth)",
    enter_ws_token_hint: "Recommended to set a token in production",
    select_channel: "Connect an IM Channel (optional)",
    channel_none: "Skip — WebSocket only",
    channel_lark: "Feishu / Lark",
    channel_dingtalk: "DingTalk",
    enter_lark_app_id: "Lark App ID (format: cli_xxxx)",
    enter_lark_app_secret: "Lark App Secret",
    enter_lark_verify_token: "Lark Verification Token",
    enter_lark_bot_name: "Bot name (for @mention in groups, optional)",
    enter_dingtalk_client_id: "DingTalk AppKey / client_id",
    enter_dingtalk_client_secret: "DingTalk AppSecret / client_secret",
    enter_dingtalk_agent_id: "AgentId (number, from DingTalk app basic info, optional)",
    enter_dingtalk_bot_name: "Bot name (optional)",
    confirm_write: "Write configuration files?",
    written_config: "✓ Config written to ~/.quickai/config.toml",
    written_env: "✓ API Key written to ~/.quickai/.env",
    backed_up: "✓ Old config backed up",
    done: "Setup complete!",
    next_steps: "Start Gateway:\n  source ~/.quickai/.env && quickai serve\n\nOther commands:\n  quickai doctor      — diagnose issues\n  quickai status      — show config\n  quickai auth list   — view API keys\n  quickai setup       — reconfigure",
};

static JA: Messages = Messages {
    welcome: "════════════════════════════════════\n  QuickAI Gateway セットアップ\n════════════════════════════════════",
    select_provider: "AIプロバイダーを選択",
    enter_api_key: "APIキーを入力",
    enter_api_key_hint: "（入力内容は非表示）",
    enter_api_base: "カスタムAPI Base URL（任意）",
    enter_model: "モデル名（空欄でデフォルト）",
    select_mode: "動作モードを選択",
    mode_solo: "Solo — シングルエージェント",
    mode_multi: "Multi-agent — 複数エージェント（@mentionで切替）",
    mode_team: "Team — リード＋スペシャリスト編成",
    enter_port: "Gatewayポート（0=ランダム、デフォルト8080）",
    enter_workspace: "デフォルト作業ディレクトリ（任意）",
    enter_ws_token: "WebSocket認証トークン（空欄=認証なし）",
    enter_ws_token_hint: "本番環境では設定を推奨",
    select_channel: "IMチャンネル接続（任意）",
    channel_none: "スキップ — WebSocketのみ",
    channel_lark: "Feishu / Lark",
    channel_dingtalk: "DingTalk",
    enter_lark_app_id: "Lark App ID（形式: cli_xxxx）",
    enter_lark_app_secret: "Lark App Secret",
    enter_lark_verify_token: "Lark Verification Token",
    enter_lark_bot_name: "Bot名（@mention識別用、任意）",
    enter_dingtalk_client_id: "DingTalk AppKey / client_id",
    enter_dingtalk_client_secret: "DingTalk AppSecret / client_secret",
    enter_dingtalk_agent_id: "AgentId（数字、任意）",
    enter_dingtalk_bot_name: "Bot名（任意）",
    confirm_write: "設定ファイルを書き込みますか？",
    written_config: "✓ ~/.quickai/config.toml に設定を書き込みました",
    written_env: "✓ APIキーを ~/.quickai/.env に書き込みました",
    backed_up: "✓ 旧設定をバックアップしました",
    done: "セットアップ完了！",
    next_steps: "起動：\n  source ~/.quickai/.env && quickai serve\n\nその他：\n  quickai doctor      — 問題診断\n  quickai status      — 設定確認",
};

static KO: Messages = Messages {
    welcome: "════════════════════════════════════\n  QuickAI Gateway 설정 마법사\n════════════════════════════════════",
    select_provider: "AI 공급자 선택",
    enter_api_key: "API 키 입력",
    enter_api_key_hint: "（입력 내용 비표시）",
    enter_api_base: "커스텀 API Base URL（선택사항）",
    enter_model: "모델 이름（기본값 사용 시 빈칸）",
    select_mode: "실행 모드 선택",
    mode_solo: "Solo — 단일 에이전트",
    mode_multi: "Multi-agent — 다중 에이전트（@mention 전환）",
    mode_team: "Team — Lead + Specialists 협업",
    enter_port: "Gateway 포트（0=랜덤, 기본 8080）",
    enter_workspace: "기본 작업 디렉토리（선택사항）",
    enter_ws_token: "WebSocket 인증 토큰（빈칸=인증 없음）",
    enter_ws_token_hint: "프로덕션 환경에서는 설정 권장",
    select_channel: "IM 채널 연결（선택사항）",
    channel_none: "건너뛰기 — WebSocket만",
    channel_lark: "Feishu / Lark",
    channel_dingtalk: "DingTalk",
    enter_lark_app_id: "Lark App ID（형식: cli_xxxx）",
    enter_lark_app_secret: "Lark App Secret",
    enter_lark_verify_token: "Lark Verification Token",
    enter_lark_bot_name: "봇 이름（선택사항）",
    enter_dingtalk_client_id: "DingTalk AppKey / client_id",
    enter_dingtalk_client_secret: "DingTalk AppSecret / client_secret",
    enter_dingtalk_agent_id: "AgentId（숫자, 선택사항）",
    enter_dingtalk_bot_name: "봇 이름（선택사항）",
    confirm_write: "설정 파일을 저장하시겠습니까?",
    written_config: "✓ ~/.quickai/config.toml 저장 완료",
    written_env: "✓ API 키가 ~/.quickai/.env에 저장되었습니다",
    backed_up: "✓ 이전 설정이 백업되었습니다",
    done: "설정 완료！",
    next_steps: "시작：\n  source ~/.quickai/.env && quickai serve\n\n기타：\n  quickai doctor      — 문제 진단\n  quickai status      — 설정 확인",
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_languages_non_empty() {
        for lang in [Language::Zh, Language::En, Language::Ja, Language::Ko] {
            let m = Messages::for_lang(lang);
            assert!(!m.welcome.is_empty());
            assert!(!m.select_provider.is_empty());
            assert!(!m.done.is_empty());
        }
    }
    #[test]
    fn from_arg_defaults_to_zh() {
        assert_eq!(Language::from_arg(None), Language::Zh);
        assert_eq!(Language::from_arg(Some(&LangArg::En)), Language::En);
    }
}

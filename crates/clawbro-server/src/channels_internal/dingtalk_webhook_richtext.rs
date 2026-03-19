use crate::channels_internal::dingtalk_webhook_types::DingTalkWebhookRichTextNode;

const IMAGE_PLACEHOLDER: &str = "[image]";
const FILE_PLACEHOLDER: &str = "[file]";
const MAX_RICHTEXT_IMAGES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichTextImageTask {
    pub download_code: String,
    pub placeholder_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RichTextRender {
    pub text: String,
    pub images: Vec<RichTextImageTask>,
}

pub fn parse_richtext_nodes(nodes: &[DingTalkWebhookRichTextNode]) -> RichTextRender {
    let mut parts = Vec::new();
    let mut images = Vec::new();
    let mut placeholder_index = 0usize;

    for node in nodes {
        if let Some(text) = node.text.as_deref().map(str::trim).filter(|text| !text.is_empty()) {
            parts.push(text.to_string());
            continue;
        }

        match node.node_type.as_deref() {
            Some("picture") | Some("image") => {
                let download_code = node
                    .download_code
                    .as_deref()
                    .or(node.picture_download_code.as_deref())
                    .map(str::trim)
                    .filter(|code| !code.is_empty());
                parts.push(IMAGE_PLACEHOLDER.to_string());
                if let Some(download_code) = download_code.filter(|_| images.len() < MAX_RICHTEXT_IMAGES) {
                    images.push(RichTextImageTask {
                        download_code: download_code.to_string(),
                        placeholder_index,
                    });
                }
                placeholder_index += 1;
            }
            Some("file") => {
                parts.push(
                    node.file_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(|name| format!("{FILE_PLACEHOLDER}:{name}"))
                        .unwrap_or_else(|| FILE_PLACEHOLDER.to_string()),
                );
            }
            _ => {}
        }
    }

    RichTextRender {
        text: parts.join(" ").trim().to_string(),
        images,
    }
}

pub async fn resolve_richtext_image_urls(
    client: &reqwest::Client,
    access_token: Option<&str>,
    robot_code: Option<&str>,
    images: &[RichTextImageTask],
) -> Vec<Option<String>> {
    let access_token = access_token
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let robot_code = robot_code
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter(|value| *value != "normal");

    if access_token.is_none() || robot_code.is_none() || images.is_empty() {
        return vec![None; images.len()];
    }

    let access_token = access_token.unwrap();
    let robot_code = robot_code.unwrap();
    let mut out = Vec::with_capacity(images.len());
    for image in images {
        let result = resolve_image_url(client, access_token, robot_code, &image.download_code)
            .await
            .ok()
            .filter(|url| !url.trim().is_empty());
        out.push(result);
    }
    out
}

async fn resolve_image_url(
    client: &reqwest::Client,
    access_token: &str,
    robot_code: &str,
    download_code: &str,
) -> anyhow::Result<String> {
    let response: serde_json::Value = client
        .post("https://api.dingtalk.com/v1.0/robot/messageFiles/download")
        .header("x-acs-dingtalk-access-token", access_token)
        .json(&serde_json::json!({
            "downloadCode": download_code,
            "robotCode": robot_code,
        }))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(response["downloadUrl"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string())
}

pub fn inject_resolved_image_urls(rendered_text: &str, urls: &[Option<String>]) -> String {
    let mut output = String::new();
    let mut remaining = rendered_text;
    let mut index = 0usize;
    while let Some(pos) = remaining.find(IMAGE_PLACEHOLDER) {
        output.push_str(&remaining[..pos]);
        let replacement = urls
            .get(index)
            .and_then(|url| url.as_deref())
            .map(|url| format!("[image: {url}]"))
            .unwrap_or_else(|| IMAGE_PLACEHOLDER.to_string());
        output.push_str(&replacement);
        remaining = &remaining[pos + IMAGE_PLACEHOLDER.len()..];
        index += 1;
    }
    output.push_str(remaining);
    output.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels_internal::dingtalk_webhook_types::DingTalkWebhookRichTextNode;

    #[test]
    fn parse_richtext_nodes_extracts_text_files_and_image_placeholders() {
        let render = parse_richtext_nodes(&[
            DingTalkWebhookRichTextNode {
                text: Some("hello".to_string()),
                node_type: None,
                download_code: None,
                picture_download_code: None,
                file_name: None,
                content_type: None,
                width: None,
                height: None,
            },
            DingTalkWebhookRichTextNode {
                text: None,
                node_type: Some("picture".to_string()),
                download_code: Some("dc-1".to_string()),
                picture_download_code: None,
                file_name: None,
                content_type: None,
                width: None,
                height: None,
            },
            DingTalkWebhookRichTextNode {
                text: None,
                node_type: Some("file".to_string()),
                download_code: None,
                picture_download_code: None,
                file_name: Some("note.txt".to_string()),
                content_type: None,
                width: None,
                height: None,
            },
        ]);
        assert_eq!(render.text, "hello [image] [file]:note.txt");
        assert_eq!(render.images.len(), 1);
        assert_eq!(render.images[0].download_code, "dc-1");
    }

    #[test]
    fn inject_resolved_image_urls_replaces_placeholders_in_order() {
        let text = inject_resolved_image_urls(
            "hello [image] world [image]",
            &[Some("https://a".into()), None],
        );
        assert_eq!(text, "hello [image: https://a] world [image]");
    }
}

/// SSE 帧解析器。
///
/// MiniMax 走标准 OpenAI 兼容路径，流式响应是 `text/event-stream`，
/// 每帧形如 `data: {json}\n\n`。**不能**用 `"data:"` 做切分点——正文里
/// 出现字面量 `data:` 会把 JSON 劈坏。正确做法是按空行（帧边界）切。
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// 喂入一段原始文本，返回本次能够完整解析出的 data 负载。
    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buffer.push_str(chunk);
        // 先归一化换行。chunk 边界可能把 \r 和 \n 拆到两片里，
        // 所以每次都重新规范化整个 buffer，而不是只处理新来的片段。
        if self.buffer.contains("\r\n") {
            self.buffer = self.buffer.replace("\r\n", "\n");
        }

        let mut payloads = Vec::new();
        while let Some(idx) = self.buffer.find("\n\n") {
            let frame: String = self.buffer.drain(..idx + 2).collect();
            let frame = frame.trim_end_matches('\n');

            // 一个帧里可能有多个 data: 行（规范允许），这里只取第一个非空的。
            for line in frame.lines() {
                let Some(payload) = line.strip_prefix("data:") else {
                    continue;
                };
                let payload = payload.trim();
                if payload.is_empty() || payload == "[DONE]" {
                    continue;
                }
                payloads.push(payload.to_string());
                break;
            }
        }

        payloads
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

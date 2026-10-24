use serde::Serialize;
use serde_json::json;

use super::super::tools::{arg_i64, arg_str, Tool, ToolDefinition, ToolRegistry};

/// 行情数据源。
///
/// 腾讯财经为主源：无需 Referer、无需 API key、无需注册，字段最全。
/// 两个源返回的都是 **GBK**，reqwest 的 `text()` 在没有 charset 头时会
/// fallback 到 UTF-8，中文名会乱码，所以统一用 encoding_rs 手动解码。
struct MarketSource {
    client: reqwest::Client,
}

impl MarketSource {
    fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
        }
    }

    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, String> {
        let response = self
            .client
            .get(url)
            .header("User-Agent", "Mozilla/5.0")
            .send()
            .await
            .map_err(|err| format!("请求行情失败：{err}"))?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|err| format!("读取行情失败：{err}"))?;

        if !status.is_success() {
            return Err(format!("行情接口返回 {status}"));
        }

        Ok(bytes.to_vec())
    }

    /// 文本接口：GBK（腾讯的行情/搜索接口是这个编码）。
    async fn get_gbk(&self, url: &str) -> Result<String, String> {
        let bytes = self.get_bytes(url).await?;
        let (text, _, _) = encoding_rs::GBK.decode(&bytes);
        Ok(text.into_owned())
    }

    /// JSON 接口：优先严格 UTF-8，失败再退回 GBK。
    ///
    /// 腾讯这几个接口的编码并不统一：`day/query` 和 `newfqkline/get` 返回 UTF-8，
    /// 而 `qt.gtimg.cn` 是 GBK。用 GBK 去解 UTF-8 会把中文吞掉、JSON 结构也可能
    /// 被破坏；反过来用 UTF-8 解 GBK 会直接报错。先试严格 UTF-8 最稳。
    async fn get_json_text(&self, url: &str) -> Result<String, String> {
        let bytes = self.get_bytes(url).await?;
        let (text, _, had_errors) = encoding_rs::UTF_8.decode(&bytes);
        if !had_errors {
            return Ok(text.into_owned());
        }
        let (text, _, _) = encoding_rs::GBK.decode(&bytes);
        Ok(text.into_owned())
    }
}

/// 一只股票的实时行情快照。
#[derive(Debug, Clone, Serialize)]
pub struct Quote {
    pub code: String,
    pub name: String,
    pub price: f64,
    pub prev_close: f64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub change: f64,
    pub change_pct: f64,
    /// 成交量，单位：手
    pub volume: f64,
    /// 成交额，单位：元
    pub amount: f64,
    /// 换手率，%
    pub turnover: f64,
    /// 振幅，%
    pub amplitude: f64,
    /// 量比
    pub volume_ratio: f64,
    /// 市盈率(TTM)
    pub pe: f64,
    /// 市净率
    pub pb: f64,
    /// 流通市值，单位：亿元
    pub float_mcap: f64,
    /// 总市值，单位：亿元
    pub total_mcap: f64,
    pub limit_up: f64,
    pub limit_down: f64,
    /// 行情时间戳
    pub time: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KlinePoint {
    pub date: String,
    pub open: f64,
    pub close: f64,
    pub high: f64,
    pub low: f64,
    pub volume: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct KlineSeries {
    pub code: String,
    pub period: String,
    pub items: Vec<KlinePoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IntradayPoint {
    pub time: String,
    pub price: f64,
    /// 累计成交量，单位：手
    pub volume: f64,
    /// 累计成交额，单位：元
    pub amount: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct IntradaySeries {
    pub code: String,
    pub name: String,
    /// 昨收价，分时图的基准线
    pub prev_close: f64,
    pub price: f64,
    pub change_pct: f64,
    pub date: String,
    pub items: Vec<IntradayPoint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StockMatch {
    pub code: String,
    pub name: String,
    pub pinyin: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub keyword: String,
    pub matches: Vec<StockMatch>,
}

/* ------------------------------------------------------------------ *
 * 数据获取
 * ------------------------------------------------------------------ */

/// 批量拉实时行情。`codes` 形如 `sh600000`，支持逗号分隔批量。
pub async fn fetch_quotes(codes: &[String]) -> Result<Vec<Quote>, String> {
    if codes.is_empty() {
        return Ok(Vec::new());
    }

    let source = MarketSource::new();
    let url = format!("https://qt.gtimg.cn/q={}", codes.join(","));
    let body = source.get_gbk(&url).await?;

    let mut quotes = Vec::new();
    for line in body.lines() {
        let Some(start) = line.find('"') else {
            continue;
        };
        let Some(end) = line.rfind('"') else {
            continue;
        };
        if end <= start {
            continue;
        }

        // 市场前缀在行的 key 上：`v_sz300750="..."`。
        // 行情字段里的 code 是纯数字，光看它判断不出是沪是深。
        let market = if line.starts_with("v_sz") {
            "sz"
        } else if line.starts_with("v_sh") {
            "sh"
        } else if line.starts_with("v_bj") {
            "bj"
        } else {
            ""
        };

        if let Some(quote) = parse_tencent_quote(&line[start + 1..end], market) {
            quotes.push(quote);
        }
    }

    Ok(quotes)
}

/// 拉前复权K线。`period` 取 day / week / month。
pub async fn fetch_kline(code: &str, period: &str, count: i64) -> Result<KlineSeries, String> {
    let code = normalize_code(code);
    if code.is_empty() {
        return Err("股票代码不能为空".to_string());
    }

    let period = match period {
        "week" | "weekly" | "周" | "周线" => "week",
        "month" | "monthly" | "月" | "月线" => "month",
        _ => "day",
    };
    let count = count.clamp(5, 320);

    let source = MarketSource::new();
    // 腾讯把 fqkline/get 下线了（501 Not Implemented），改走 newfqkline/get。
    // 备用源 kline/kline 是不复权的，但 key 不同（day vs qfqday），
    // 下面的解析两种都认，所以可以直接当降级用。
    let urls = [
        format!(
            "https://web.ifzq.gtimg.cn/appstock/app/newfqkline/get?param={code},{period},,,{count},qfq"
        ),
        format!("https://web.ifzq.gtimg.cn/appstock/app/kline/kline?param={code},{period},,,{count}"),
    ];

    let mut items: Vec<KlinePoint> = Vec::new();
    let mut last_error = String::new();

    for url in urls {
        let body = match source.get_json_text(&url).await {
            Ok(body) => body,
            Err(err) => {
                last_error = err;
                continue;
            }
        };

        let Ok(root) = serde_json::from_str::<serde_json::Value>(&body) else {
            last_error = "K线数据解析失败".to_string();
            continue;
        };

        let Some(node) = root.get("data").and_then(|d| d.get(&code)) else {
            last_error = format!("没有查到 {code} 的K线");
            continue;
        };

        // 前复权数据的 key 带 qfq 前缀，复权与否 key 不同，两种都试。
        let rows = node
            .get(&format!("qfq{period}"))
            .or_else(|| node.get(period))
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();

        items.clear();
        for row in rows {
            let Some(cells) = row.as_array() else {
                continue;
            };
            if cells.len() < 6 {
                continue;
            }

            let get = |index: usize| cells[index].as_str().unwrap_or("").to_string();
            let parse = |value: &str| value.parse::<f64>().unwrap_or(0.0);

            // 腾讯的行顺序是 [日期, 开, 收, 高, 低, 成交量]，注意不是 OHLC。
            items.push(KlinePoint {
                date: get(0),
                open: parse(&get(1)),
                close: parse(&get(2)),
                high: parse(&get(3)),
                low: parse(&get(4)),
                volume: parse(&get(5)),
            });
        }

        if !items.is_empty() {
            break;
        }

        last_error = format!("没有查到 {code} 的K线");
    }

    if items.is_empty() {
        return Err(if last_error.is_empty() {
            format!("没有查到 {code} 的K线")
        } else {
            last_error
        });
    }

    Ok(KlineSeries {
        code,
        period: period.to_string(),
        items,
    })
}

/// 拉今日分时。
pub async fn fetch_intraday(code: &str) -> Result<IntradaySeries, String> {
    let code = normalize_code(code);
    if code.is_empty() {
        return Err("股票代码不能为空".to_string());
    }

    let source = MarketSource::new();
    // 腾讯把 minute/query 下线了（501 Not Implemented），改走 day/query：
    // 它返回最近 5 个交易日的分时，data[0] 就是最新一天。
    let url = format!("https://web.ifzq.gtimg.cn/appstock/app/day/query?code={code}");
    let body = source.get_json_text(&url).await?;

    let root: serde_json::Value =
        serde_json::from_str(&body).map_err(|_| "分时数据解析失败".to_string())?;

    let Some(node) = root.get("data").and_then(|d| d.get(&code)) else {
        return Err(format!("没有查到 {code} 的分时数据"));
    };

    // qt 是这一路的行情快照。腾讯有两种形态：
    // 按代码索引的对象 { "sh600519": [...] }，或者直接一个数组。
    let quote = node.get("qt").and_then(|q| {
        q.get(&code)
            .and_then(|v| v.as_array())
            .or_else(|| q.as_array())
    });
    let quote_str = |index: usize| -> String {
        quote
            .and_then(|arr| arr.get(index))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let quote_num = |index: usize| -> f64 {
        quote_str(index).parse::<f64>().unwrap_or(0.0)
    };

    // day/query 的 data 是「按天分组」的数组，第一组就是当天。
    let Some(day) = node
        .get("data")
        .and_then(|d| d.as_array())
        .and_then(|days| days.first())
    else {
        return Err(format!("没有查到 {code} 的分时数据"));
    };

    let date = day
        .get("date")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // 昨收直接给在当天那组里（prec），比从 qt 里翻字段可靠。
    let prev_close = day
        .get("prec")
        .and_then(|v| v.as_str())
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or_else(|| quote_num(4));
    let price = quote_num(3);
    let change_pct = quote_num(32);
    let name = quote_str(1);

    // 分点挂在 day["data"]，不是更深一层。
    let Some(rows) = day.get("data").and_then(|value| value.as_array()) else {
        return Err(format!("没有查到 {code} 的分时数据"));
    };

    // 每项形如 "0930 9.66 1037 1001742.00"：时间 / 现价 / 累计量(手) / 累计额(元)
    let mut items = Vec::new();
    for row in rows {
        let Some(line) = row.as_str() else {
            continue;
        };
        let cells: Vec<&str> = line.split_whitespace().collect();
        if cells.len() < 4 {
            continue;
        }
        items.push(IntradayPoint {
            time: cells[0].to_string(),
            price: cells[1].parse::<f64>().unwrap_or(0.0),
            volume: cells[2].parse::<f64>().unwrap_or(0.0),
            amount: cells[3].parse::<f64>().unwrap_or(0.0),
        });
    }

    if items.is_empty() {
        return Err(format!("没有查到 {code} 的分时数据"));
    }

    Ok(IntradaySeries {
        code,
        name,
        prev_close,
        price,
        change_pct,
        date,
        items,
    })
}

/// 按名称、代码或拼音搜索股票。
pub async fn search_stocks(keyword: &str) -> Result<SearchResult, String> {
    let keyword = keyword.trim();
    if keyword.is_empty() {
        return Err("搜索关键词不能为空".to_string());
    }

    let source = MarketSource::new();
    let url = format!(
        "https://smartbox.gtimg.cn/s3/?q={}&t=all",
        urlencode(keyword)
    );
    let body = source.get_gbk(&url).await?;

    // 响应形如：v_hint="sh~600519~贵州茅台~gzmt~GP-A;sz~000858~五粮液~wly~GP-A";
    let mut matches = Vec::new();
    for entry in body.split(';') {
        let entry = entry.trim();
        let Some(start) = entry.find('"') else {
            continue;
        };
        let Some(end) = entry.rfind('"') else {
            continue;
        };
        if end <= start {
            continue;
        }

        let parts: Vec<&str> = entry[start + 1..end].split('~').collect();
        if parts.len() < 4 {
            continue;
        }

        let market = parts[0].trim();
        let code = parts[1].trim();
        if code.is_empty() || !matches!(market, "sh" | "sz" | "bj") {
            continue;
        }

        matches.push(StockMatch {
            code: format!("{market}{code}"),
            name: decode_unicode_escapes(parts[2].trim()),
            pinyin: parts[3].trim().to_string(),
        });
    }

    Ok(SearchResult {
        keyword: keyword.to_string(),
        matches,
    })
}

fn field<'a>(parts: &[&'a str], index: usize) -> &'a str {
    parts.get(index).copied().unwrap_or("")
}

fn num(parts: &[&str], index: usize) -> f64 {
    field(parts, index).parse::<f64>().unwrap_or(0.0)
}

/// 解析腾讯行情的一行 `~` 分隔数据。
///
/// 字段位置（实测自 `qt.gtimg.cn`）：
/// `[1]`名称 `[2]`代码 `[3]`现价 `[4]`昨收 `[5]`今开 `[6]`成交量(手)
/// `[30]`时间 `[31]`涨跌 `[32]`涨跌幅% `[33]`最高 `[34]`最低
/// `[37]`成交额(万) `[38]`换手率 `[39]`市盈率 `[43]`振幅
/// `[44]`流通市值(亿) `[45]`总市值(亿) `[46]`市净率
/// `[47]`涨停价 `[48]`跌停价 `[49]`量比
///
/// `market` 是市场前缀（sh/sz/bj），从响应行的 key 上取。
/// 字段里的 code 是纯数字，猜不出沪深，必须靠这个前缀。
fn parse_tencent_quote(payload: &str, market: &str) -> Option<Quote> {
    let parts: Vec<&str> = payload.split('~').collect();
    if parts.len() < 50 {
        return None;
    }

    let name = field(&parts, 1).trim().to_string();
    if name.is_empty() {
        return None;
    }

    let raw_code = field(&parts, 2).trim();
    let code = match market {
        "sh" | "sz" | "bj" => format!("{market}{raw_code}"),
        _ => normalize_code(raw_code),
    };

    let raw_time = field(&parts, 30).trim().to_string();
    let time = if raw_time.len() == 14 {
        format!(
            "{}-{}-{} {}:{}:{}",
            &raw_time[0..4],
            &raw_time[4..6],
            &raw_time[6..8],
            &raw_time[8..10],
            &raw_time[10..12],
            &raw_time[12..14]
        )
    } else {
        raw_time
    };

    Some(Quote {
        code,
        name,
        price: num(&parts, 3),
        prev_close: num(&parts, 4),
        open: num(&parts, 5),
        high: num(&parts, 33),
        low: num(&parts, 34),
        change: num(&parts, 31),
        change_pct: num(&parts, 32),
        volume: num(&parts, 36),
        // 腾讯这里给的是万元，换算成元。
        amount: num(&parts, 37) * 10_000.0,
        turnover: num(&parts, 38),
        amplitude: num(&parts, 43),
        volume_ratio: num(&parts, 49),
        pe: num(&parts, 39),
        pb: num(&parts, 46),
        float_mcap: num(&parts, 44),
        total_mcap: num(&parts, 45),
        limit_up: num(&parts, 47),
        limit_down: num(&parts, 48),
        time,
    })
}

/// 把用户/模型给的代码补上市场前缀。
pub fn normalize_code(raw: &str) -> String {
    let raw = raw.trim().to_lowercase();
    if raw.is_empty() {
        return String::new();
    }
    if raw.starts_with("sh") || raw.starts_with("sz") || raw.starts_with("bj") {
        return raw;
    }
    // 6 开头是沪市，0/3 开头是深市，其余按深市处理。
    if raw.starts_with('6') {
        format!("sh{raw}")
    } else {
        format!("sz{raw}")
    }
}

/* ------------------------------------------------------------------ *
 * 工具实现
 * ------------------------------------------------------------------ */

pub struct SearchStockTool;

#[async_trait::async_trait]
impl Tool for SearchStockTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "search_stock",
            "按股票名称、股票代码或拼音首字母搜索股票，返回候选列表。查具体行情前先用这个工具确认代码。",
            json!({
                "type": "object",
                "properties": {
                    "keyword": {
                        "type": "string",
                        "description": "股票名称、代码或拼音首字母，例如 茅台 / 600519 / gzmt / 平安银行"
                    }
                },
                "required": ["keyword"]
            }),
        )
    }

    async fn call(&self, args: serde_json::Value) -> Result<String, String> {
        let keyword = arg_str(&args, "keyword", "");
        let result = search_stocks(&keyword).await?;

        if result.matches.is_empty() {
            return Ok(json!({
                "keyword": result.keyword,
                "matches": [],
                "hint": "没搜到，换个名称或直接用股票代码试试"
            })
            .to_string());
        }

        Ok(serde_json::to_string(&result).unwrap_or_default())
    }
}

pub struct GetQuoteTool;

#[async_trait::async_trait]
impl Tool for GetQuoteTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_quote",
            "查询一只股票的实时行情：最新价、涨跌幅、开高低、成交量额、换手率、市盈率、市值等。代码形如 sh600000。",
            json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "带市场前缀的股票代码，例如 sh600000、sz000001"
                    }
                },
                "required": ["code"]
            }),
        )
    }

    async fn call(&self, args: serde_json::Value) -> Result<String, String> {
        let code = normalize_code(&arg_str(&args, "code", ""));
        if code.is_empty() {
            return Err("股票代码不能为空".to_string());
        }

        let quotes = fetch_quotes(std::slice::from_ref(&code)).await?;
        let Some(quote) = quotes.into_iter().next() else {
            return Err(format!("没有查到 {code} 的行情"));
        };

        Ok(serde_json::to_string(&quote).unwrap_or_default())
    }
}

pub struct GetKlineTool;

#[async_trait::async_trait]
impl Tool for GetKlineTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_kline",
            "查询股票历史K线（前复权）。用于看走势、算均线、回答「最近一个月涨了多少」这类问题。",
            json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "带市场前缀的股票代码，例如 sh600000"
                    },
                    "period": {
                        "type": "string",
                        "description": "周期：day(日线) / week(周线) / month(月线)，默认 day"
                    },
                    "count": {
                        "type": "integer",
                        "description": "返回的K线根数，默认 60，最大 320"
                    }
                },
                "required": ["code"]
            }),
        )
    }

    async fn call(&self, args: serde_json::Value) -> Result<String, String> {
        let code = normalize_code(&arg_str(&args, "code", ""));
        let period = arg_str(&args, "period", "day");
        let count = arg_i64(&args, "count", 60);

        let series = fetch_kline(&code, &period, count).await?;
        Ok(serde_json::to_string(&series).unwrap_or_default())
    }
}

pub struct GetIntradayTool;

#[async_trait::async_trait]
impl Tool for GetIntradayTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_intraday",
            "查询股票今日的分时走势（逐分钟价格）。用于回答「今天盘中是什么走势」这类问题。",
            json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "带市场前缀的股票代码，例如 sh600000"
                    }
                },
                "required": ["code"]
            }),
        )
    }

    async fn call(&self, args: serde_json::Value) -> Result<String, String> {
        let code = normalize_code(&arg_str(&args, "code", ""));
        let series = fetch_intraday(&code).await?;
        Ok(serde_json::to_string(&series).unwrap_or_default())
    }
}

/// 极简 URL 编码，够用于中文关键字。
fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 解码 `\uXXXX` 形式的 Unicode 转义。
///
/// 腾讯的搜索接口把中文写成转义（`\u8d35\u5dde\u8305\u53f0`），
/// 直接显示会是满屏反斜杠。只在整串都是 ASCII 时处理，避免误伤 GBK 字节。
fn decode_unicode_escapes(input: &str) -> String {
    let bytes = input.as_bytes();
    if bytes.iter().any(|byte| *byte >= 0x80) {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 5 < bytes.len()
            && bytes[index + 1] == b'u'
            && bytes[index + 2..index + 6].iter().all(|b| b.is_ascii_hexdigit())
        {
            let hex = &input[index + 2..index + 6];
            if let Ok(code) = u32::from_str_radix(hex, 16) {
                if let Some(ch) = char::from_u32(code) {
                    out.push(ch);
                    index += 6;
                    continue;
                }
            }
        }
        out.push(bytes[index] as char);
        index += 1;
    }
    out
}

/// 心跳场景单独用的行情工具定义。
///
/// 主工具表里已经注册过 get_quote，但心跳那一轮想要的是「能顺手查一下」的
/// 轻量版本，所以这里给一个更简单的描述。
pub fn quote_definition() -> ToolDefinition {
    ToolDefinition::new(
        "get_quote",
        "查询一只股票的实时行情：最新价、涨跌幅、成交量等。代码形如 sh600000。",
        json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "带市场前缀的股票代码，例如 sh600000"
                }
            },
            "required": ["code"]
        }),
    )
}

/// 把股票工具装进注册表。
pub fn build_stock_tools_into(registry: &mut ToolRegistry) {
    registry.register(Box::new(SearchStockTool));
    registry.register(Box::new(GetQuoteTool));
    registry.register(Box::new(GetKlineTool));
    registry.register(Box::new(GetIntradayTool));
}

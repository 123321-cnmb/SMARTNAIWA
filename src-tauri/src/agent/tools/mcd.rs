//! 麦当劳 MCP 客户端，以及暴露给模型的麦当劳工具。
//!
//! 服务端是 Streamable HTTP 传输的 MCP（`https://mcp.mcd.cn`），
//! 每个请求带 `Authorization: Bearer <token>`。
//!
//! 有两个必须处理的坑：
//!
//! 1. 返回值是「markdown 展示规则 + 真正的 JSON」的混合体。规则那一大段是
//!    给 MCP 客户端的**展示约定**（"必须完整展示 storeCode"之类），对模型没用，
//!    反而占 token、还容易让模型照抄成回复正文。所以这里把 JSON 抽出来单独返回。
//! 2. 但不是所有接口都返回 JSON——券列表、活动日历返回的是**纯 markdown**，
//!    没有 JSON 可抽。所以两条路都得走。
//!
//! 工具的 schema 全部手写（和其它工具一个规矩）：MCP 自带的 description 又长又
//! 带一堆规则，直接透传给模型会撑爆上下文，而且模型分不清哪个是"什么时候该用我"。

use std::sync::Arc;

use serde_json::{json, Value};

use super::super::tools::{arg_i64, arg_str, Tool, ToolDefinition, ToolRegistry};

/// 麦当劳 MCP 服务地址。
const MCP_URL: &str = "https://mcp.mcd.cn";

/// MCP 客户端。
pub struct McdClient {
    client: reqwest::Client,
    token: String,
    /// 握手做过没有。MCP 规范要求先 initialize 再 tools/call。
    /// 服务端目前不强制，但做一次更稳（上下文能挂上），失败也不影响后续调用。
    handshaked: tokio::sync::Mutex<bool>,
}

impl McdClient {
    pub fn new(token: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            token: token.to_string(),
            handshaked: tokio::sync::Mutex::new(false),
        }
    }

    /// 发一个 JSON-RPC 请求，返回 `result` 字段。
    async fn rpc(&self, body: Value) -> Result<Value, String> {
        let response = self
            .client
            .post(MCP_URL)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json; charset=utf-8")
            .header("Authorization", format!("Bearer {}", self.token))
            .body(serde_json::to_vec(&body).map_err(|err| format!("序列化失败：{err}"))?)
            .send()
            .await
            .map_err(|err| format!("连接麦当劳服务失败：{err}"))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("麦当劳服务返回 {status}"));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|err| format!("读取麦当劳响应失败：{err}"))?;
        let text = String::from_utf8_lossy(&bytes);

        // Streamable HTTP 允许返回 SSE 帧，取 data: 后面那段。
        let payload = if text.trim_start().starts_with('{') {
            text.to_string()
        } else {
            text.lines()
                .find_map(|line| line.strip_prefix("data:").map(|rest| rest.trim().to_string()))
                .unwrap_or_else(|| text.to_string())
        };

        let value: Value =
            serde_json::from_str(&payload).map_err(|err| format!("麦当劳响应不是合法 JSON：{err}"))?;

        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("未知错误");
            return Err(format!("麦当劳服务报错：{message}"));
        }

        value
            .get("result")
            .cloned()
            .ok_or_else(|| "麦当劳响应里没有 result".to_string())
    }

    /// 握手：initialize + notifications/initialized。只做一次，失败不致命。
    async fn ensure_handshake(&self) {
        let mut done = self.handshaked.lock().await;
        if *done {
            return;
        }

        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "naiwa-pet", "version": "0.1.0" }
            }
        });

        if self.rpc(init).await.is_ok() {
            let _ = self
                .rpc(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .await;
            *done = true;
        }
    }

    /// 调一个 MCP 工具，返回原始文本（可能是 markdown+JSON 混合体）。
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<String, String> {
        self.ensure_handshake().await;

        let body = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        });

        let result = self.rpc(body).await?;

        if result.get("isError").and_then(|v| v.as_bool()) == Some(true) {
            let message = result
                .get("content")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("未知错误");
            return Err(message.to_string());
        }

        result
            .get("content")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("text"))
            .and_then(|t| t.as_str())
            .map(|text| text.to_string())
            .ok_or_else(|| "麦当劳响应里没有内容".to_string())
    }
}

/// 从混合文本里抽出 JSON 对象。
///
/// 服务端的返回形态是「一大段 markdown 规则」后面**紧跟**一个 JSON。
/// 这里按平衡括号扫描，字符串里的括号不参与配对。
fn extract_json(text: &str) -> Option<Value> {
    let start = text.find("{\"success\"")?;
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (index, byte) in bytes.iter().enumerate().skip(start) {
        let ch = *byte as char;
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&text[start..=index]).ok();
                }
            }
            _ => {}
        }
    }

    None
}

/// 去掉 markdown 里的 `<img ...>` 标签。
///
/// 券的返回里每张券都带一张图，URL 有一两百字符，全是 token 黑洞。
fn strip_images(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(open) = rest.find("<img") {
        out.push_str(&rest[..open]);
        match rest[open..].find('>') {
            Some(close) => rest = &rest[open + close + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 把 MCP 的返回整理成统一结构，给模型看。
///
/// * 有 JSON → `{"source":"mcd","tool":..,"data":..}`
/// * 纯 markdown → `{"source":"mcd","tool":..,"text":..}`
fn normalize(tool: &str, raw: &str) -> Result<String, String> {
    if let Some(value) = extract_json(raw) {
        if value.get("success").and_then(|v| v.as_bool()) == Some(false) {
            let message = value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("未知错误");

            // 券用不了是最常见的错，给模型一条能直接照做的提示，
            // 别让它把原始报错原样甩给用户。
            if message.contains("券") {
                return Err(format!(
                    "麦当劳接口报错：{message}。\
                     去掉 coupon_id / coupon_code 再算一次（券可能不适用于所选餐品或已经用过了）。"
                ));
            }

            return Err(format!("麦当劳接口报错：{message}"));
        }

        let data = value.get("data").cloned().unwrap_or(Value::Null);
        return Ok(json!({ "source": "mcd", "tool": tool, "data": data }).to_string());
    }

    let text = strip_images(raw);
    let text = text.trim();
    if text.is_empty() {
        return Err("麦当劳接口没有返回内容".to_string());
    }
    Ok(json!({ "source": "mcd", "tool": tool, "text": text }).to_string())
}

/// 解析餐品营养数据。
///
/// 服务端返回的是一段 CSV 文本（塞在 JSON 的 `data` 字段里）：
/// ```text
/// [160]{productName,nutritionDescription,energyKj,energyKcal,protein,fat,carbohydrate,sodium,calcium}:
///   猪柳麦满分,null,1288,308,16,16,24,781,213
/// ```
///
/// 160 条全塞给模型太占地方，这里转成结构化数组，并支持按关键词过滤。
fn parse_nutrition(raw: &str) -> Vec<Value> {
    let mut items = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        // 表头行形如 "[160]{...}:"，跳过。
        if trimmed.is_empty() || trimmed.starts_with('[') || trimmed.starts_with('{') {
            continue;
        }
        let cells: Vec<&str> = trimmed.split(',').collect();
        if cells.len() < 9 {
            continue;
        }
        let name = cells[0].trim();
        if name.is_empty() {
            continue;
        }
        let number = |value: &str| value.trim().parse::<f64>().unwrap_or(0.0);
        items.push(json!({
            "name": name,
            "kcal": number(cells[3]),
            "energy_kj": number(cells[2]),
            "protein_g": number(cells[4]),
            "fat_g": number(cells[5]),
            "carb_g": number(cells[6]),
            "sodium_mg": number(cells[7]),
            "calcium_mg": number(cells[8]),
        }));
    }

    items
}

/// 需要用户**明确确认**才能执行的写操作。
///
/// 这些会真的创建/取消订单，所以 Rust 侧硬校验：模型必须把 `confirmed` 传 true
/// 才会真的发请求。
const CONFIRM_REQUIRED: [&str; 3] = ["create-order", "party-order-create", "cancel-order"];

/// 写操作要过这道闸：用户没明确点头，就不发请求。
fn check_confirmed(mcp_name: &str, args: &Value) -> Result<(), String> {
    if !CONFIRM_REQUIRED.contains(&mcp_name) {
        return Ok(());
    }

    let confirmed = args
        .get("confirmed")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if confirmed {
        return Ok(());
    }

    Err(
        "这是会真的下单/取消订单的操作。必须先把价格和内容给用户看清楚，\
         等用户明确说了\"下单\"\"创建订单\"\"取消\"，再带着 confirmed=true 调用。"
            .to_string(),
    )
}

/// 把价格从「分」换算成「元」。
///
/// 麦当劳的算价/下单接口返回的价格单位是**分**（1590 = 15.90 元）。
/// 直接给模型，它要么显示成 1590 元，要么照着接口说明再除一次 100 显示成 0.16 元。
/// 这里统一换算，并在 data 上标一个 unit 字段。
fn prices_to_yuan(value: &mut Value) {
    const PRICE_KEYS: [&str; 12] = [
        "price",
        "originalPrice",
        "productPrice",
        "productOriginalPrice",
        "discount",
        "subtotal",
        "originalSubtotal",
        "deliveryFee",
        "totalAmount",
        "payAmount",
        "couponAmount",
        "packagingFee",
    ];

    match value {
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if PRICE_KEYS.contains(&key.as_str()) {
                    if let Some(number) = item.as_f64() {
                        *item = json!(number / 100.0);
                    }
                    continue;
                }
                prices_to_yuan(item);
            }
        }
        Value::Array(list) => {
            for item in list.iter_mut() {
                prices_to_yuan(item);
            }
        }
        _ => {}
    }
}

/// 选购商品列表：模型给的是 product_code / quantity，接口要的是 productCode / quantity。
fn map_items(args: &Value) -> Vec<Value> {
    args.get("items")
        .and_then(|value| value.as_array())
        .map(|list| {
            list.iter()
                .map(|item| {
                    let mut out = serde_json::Map::new();
                    out.insert("productCode".into(), json!(arg_str(item, "product_code", "")));
                    out.insert("quantity".into(), json!(arg_i64(item, "quantity", 1)));
                    let coupon_id = arg_str(item, "coupon_id", "");
                    if !coupon_id.is_empty() {
                        out.insert("couponId".into(), json!(coupon_id));
                    }
                    let coupon_code = arg_str(item, "coupon_code", "");
                    if !coupon_code.is_empty() {
                        out.insert("couponCode".into(), json!(coupon_code));
                    }
                    Value::Object(out)
                })
                .collect::<Vec<Value>>()
        })
        .unwrap_or_default()
}

/// 奶蛙侧的一个麦当劳工具。
pub struct McdTool {
    client: Arc<McdClient>,
    /// 奶蛙侧的工具名（给模型看的）。
    name: String,
    /// MCP 侧的工具名。
    mcp_name: String,
    definition: ToolDefinition,
}

#[async_trait::async_trait]
impl Tool for McdTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn call(&self, args: Value) -> Result<String, String> {
        // 写操作：用户没明确点头就不发请求。
        check_confirmed(&self.mcp_name, &args)?;

        // 营养成分要本地过滤，所以单独走一条路。
        if self.mcp_name == "list-nutrition-foods" {
            let raw = self.client.call_tool(&self.mcp_name, json!({})).await?;
            // CSV 塞在返回 JSON 的 data 字段里（是个字符串），得先把它取出来。
            let csv = extract_json(&raw)
                .and_then(|value| value.get("data").cloned())
                .and_then(|data| data.as_str().map(|text| text.to_string()))
                .unwrap_or_else(|| raw.clone());

            let mut items = parse_nutrition(&csv);
            let keyword = arg_str(&args, "keyword", "");
            if !keyword.trim().is_empty() {
                let needle = keyword.trim().to_lowercase();
                items.retain(|item| {
                    item.get("name")
                        .and_then(|n| n.as_str())
                        .map(|name| name.to_lowercase().contains(&needle))
                        .unwrap_or(false)
                });
            }
            return Ok(json!({
                "source": "mcd",
                "tool": self.name,
                "data": { "count": items.len(), "items": items }
            })
            .to_string());
        }

        let arguments = match self.mcp_name.as_str() {
            "query-nearby-stores" => json!({
                "searchType": 2,
                "city": arg_str(&args, "city", ""),
                "keyword": arg_str(&args, "keyword", ""),
                "beType": arg_i64(&args, "be_type", 1),
            }),
            "mall-points-products" => {
                let rule = arg_str(&args, "category", "");
                if rule.trim().is_empty() {
                    json!({})
                } else {
                    json!({ "catRuleIds": rule.trim() })
                }
            }
            "mall-create-order" => json!({
                "skuId": arg_str(&args, "sku_id", ""),
                "spuCategory": arg_str(&args, "spu_category", "1"),
                "addressId": arg_str(&args, "address_id", ""),
                "count": arg_i64(&args, "count", 1),
            }),
            "mall-order-list" => json!({
                "lastId": arg_str(&args, "last_id", ""),
                "size": arg_i64(&args, "size", 20),
            }),
            "query-my-prizes" => json!({
                "pageNum": arg_i64(&args, "page", 1),
                "pageSize": arg_i64(&args, "page_size", 10),
            }),
            "query-my-coupons" => json!({
                "page": arg_str(&args, "page", "1"),
                "pageSize": arg_str(&args, "page_size", "200"),
            }),
            // ---- C 档：到店 / 外送 / 团餐 ----
            "delivery-query-stores" => json!({
                "addressId": arg_str(&args, "address_id", ""),
                "beType": arg_i64(&args, "be_type", 2),
            }),
            "delivery-create-address" => json!({
                "city": arg_str(&args, "city", ""),
                "contactName": arg_str(&args, "contact_name", ""),
                "gender": arg_str(&args, "gender", ""),
                "phone": arg_str(&args, "phone", ""),
                "address": arg_str(&args, "address", ""),
                "addressDetail": arg_str(&args, "address_detail", ""),
            }),
            "query-meals" => json!({
                "storeCode": arg_str(&args, "store_code", ""),
                "orderType": arg_i64(&args, "order_type", 1),
                "beType": arg_i64(&args, "be_type", 1),
                "beCode": arg_str(&args, "be_code", ""),
            }),
            "query-meal-detail" => json!({
                "storeCode": arg_str(&args, "store_code", ""),
                "orderType": arg_i64(&args, "order_type", 1),
                "beType": arg_i64(&args, "be_type", 1),
                "beCode": arg_str(&args, "be_code", ""),
                "code": arg_str(&args, "code", ""),
            }),
            "query-store-coupons" => json!({
                "storeCode": arg_str(&args, "store_code", ""),
                "orderType": arg_i64(&args, "order_type", 1),
                "beType": arg_i64(&args, "be_type", 1),
                "beCode": arg_str(&args, "be_code", ""),
                "reservationDate": arg_str(&args, "reservation_date", ""),
            }),
            "calculate-price" => json!({
                "storeCode": arg_str(&args, "store_code", ""),
                "orderType": arg_i64(&args, "order_type", 1),
                "beType": arg_i64(&args, "be_type", 1),
                "beCode": arg_str(&args, "be_code", ""),
                "items": map_items(&args),
                "needTableware": args.get("need_tableware").and_then(|v| v.as_bool()).unwrap_or(false),
                "reservationDate": arg_str(&args, "reservation_date", ""),
                "gmServiceCode": arg_str(&args, "gm_service_code", ""),
            }),
            "create-order" => json!({
                "storeCode": arg_str(&args, "store_code", ""),
                "orderType": arg_i64(&args, "order_type", 1),
                "beType": arg_i64(&args, "be_type", 1),
                "beCode": arg_str(&args, "be_code", ""),
                "addressId": arg_str(&args, "address_id", ""),
                "items": map_items(&args),
                "takeWayCode": arg_str(&args, "take_way_code", ""),
                "remark": arg_str(&args, "remark", ""),
                "needTableware": args.get("need_tableware").and_then(|v| v.as_bool()).unwrap_or(false),
                "reservationDate": arg_str(&args, "reservation_date", ""),
                "gmServiceCode": arg_str(&args, "gm_service_code", ""),
            }),
            "cancel-order" => json!({
                "orderId": arg_str(&args, "order_id", ""),
                "cancelReasonCode": arg_i64(&args, "cancel_reason_code", -1),
            }),
            "query-meal-assistance" => json!({
                "storeCode": arg_str(&args, "store_code", ""),
                "orderType": arg_i64(&args, "order_type", 1),
                "beType": arg_i64(&args, "be_type", 1),
                "beCode": arg_str(&args, "be_code", ""),
            }),
            "query-promotions" => json!({
                "storeCode": arg_str(&args, "store_code", ""),
                "beCode": arg_str(&args, "be_code", ""),
                "orderType": arg_i64(&args, "order_type", 2),
                "beType": arg_i64(&args, "be_type", 6),
                "reservationDate": arg_str(&args, "reservation_date", ""),
            }),
            // ---- C 档：主题活动（生日派对 / 体验营 / 品鉴会）----
            "query-party-city" => json!({ "spuId": arg_i64(&args, "spu_id", 0) }),
            "query-party-store" => json!({
                "spuId": arg_i64(&args, "spu_id", 0),
                "code": arg_str(&args, "code", ""),
                "latitude": arg_i64(&args, "latitude", 0),
                "longitude": arg_i64(&args, "longitude", 0),
            }),
            "query-party-store-date" => json!({
                "spuId": arg_i64(&args, "spu_id", 0),
                "storeCode": arg_str(&args, "store_code", ""),
            }),
            "query-party-store-session" => json!({
                "spuId": arg_i64(&args, "spu_id", 0),
                "storeCode": arg_str(&args, "store_code", ""),
                "dateStr": arg_str(&args, "date_str", ""),
            }),
            "party-order-create" => json!({
                "partyType": arg_i64(&args, "party_type", 0),
                "spuId": arg_i64(&args, "spu_id", 0),
                "skuId": arg_i64(&args, "sku_id", 0),
                "id": arg_i64(&args, "id", 0),
                "code": arg_str(&args, "code", ""),
                "storeCode": arg_str(&args, "store_code", ""),
                "dateStr": arg_str(&args, "date_str", ""),
                "count": arg_i64(&args, "count", 1),
                "timeStart": arg_str(&args, "time_start", ""),
                "timeEnd": arg_str(&args, "time_end", ""),
                "leftNum": arg_i64(&args, "left_num", 0),
                "partyTimeInfo": args.get("party_time_info").cloned().unwrap_or(Value::Null),
            }),
            _ => args.clone(),
        };

        let raw = self.client.call_tool(&self.mcp_name, arguments).await?;

        // 算价最坑的一种错：接口返回 success，但商品被**静默丢弃**
        // （productList 是空的，价格只剩配送费）。模型看不出来，用户还以为算对了。
        // 根因基本是"到店和外送混用了菜单编码"——两套菜单编码不一样。
        if self.mcp_name == "calculate-price" {
            if let Some(value) = extract_json(&raw) {
                let empty = value
                    .get("data")
                    .and_then(|data| data.get("productList"))
                    .and_then(|list| list.as_array())
                    .map(|list| list.is_empty())
                    .unwrap_or(false);
                if empty {
                    return Err(
                        "这些餐品在当前下单方式下买不到（接口把商品悄悄丢掉了）。\
                         **到店和外送是两套菜单，编码不一样**——\
                         比如巨无霸到店是 1100、外送是 901100。\
                         请用当前场景（到店/外送）重新查一次菜单，用新编码再算。"
                            .to_string(),
                    );
                }
            }
        }

        // 算价和下单返回的价格是「分」，这里换算成「元」。
        let mut normalized = normalize(&self.name, &raw)?;
        if self.mcp_name == "calculate-price" || self.mcp_name == "create-order" {
            if let Ok(mut value) = serde_json::from_str::<Value>(&normalized) {
                if let Some(data) = value.get_mut("data") {
                    prices_to_yuan(data);
                    if let Some(map) = data.as_object_mut() {
                        map.insert("unit".to_string(), json!("元"));
                    }
                }
                normalized = value.to_string();
            }
        }

        Ok(normalized)
    }
}

fn obj(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
    })
}

fn string_prop(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn int_prop(description: &str) -> Value {
    json!({ "type": "integer", "description": description })
}

/// 生成支付二维码（SVG）。
///
/// 麦当劳给的支付链接 `m.mcd.cn/mcp/scanToPay?orderId=xxx` 在电脑上打不开——
/// 那是个给手机用的 H5 页。所以这里按官方前端同样的规则拼出扫码地址，
/// 再自己画成二维码，用户拿手机扫一下就能付。
///
/// 规则来自官方前端的 chunk：订单号 `199`/`333` 开头用 `q`，其余用 `s`。
pub fn payment_qr_svg(order_id: &str) -> Result<String, String> {
    let order_id = order_id.trim();
    if order_id.is_empty() {
        return Err("订单号不能为空".to_string());
    }
    qr_svg(&payment_qr_content(order_id))
}

/// 二维码里扫出来的地址。
///
/// 规则来自官方前端的 chunk：订单号 `199`/`333` 开头用 `q`，其余用 `s`。
pub fn payment_qr_content(order_id: &str) -> String {
    let kind = if order_id.starts_with("199") || order_id.starts_with("333") {
        "q"
    } else {
        "s"
    };
    format!("https://qr.mcd.cn/{kind}?utm_term={order_id}")
}

/// 把一段内容画成二维码 SVG。
fn qr_svg(content: &str) -> Result<String, String> {
    let code =
        qrcode::QrCode::new(content.as_bytes()).map_err(|err| format!("生成二维码失败：{err}"))?;
    let size = code.width();

    // 一个 path 画完所有黑块，比每个模块一个 rect 小得多。
    let mut path = String::new();
    for (index, color) in code.to_colors().iter().enumerate() {
        if *color == qrcode::Color::Dark {
            path.push_str(&format!("M{} {}h1v1h-1z", index % size, index / size));
        }
    }

    // 四周留 4 个模块的静区，不然扫不出来。
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-4 -4 {span} {span}\" \
         shape-rendering=\"crispEdges\"><rect x=\"-4\" y=\"-4\" width=\"{span}\" \
         height=\"{span}\" fill=\"#fff\"/><path d=\"{path}\" fill=\"#000\"/></svg>",
        span = size + 8,
        path = path
    ))
}

/// 注册所有麦当劳工具。
pub fn build_mcd_tools_into(registry: &mut ToolRegistry, token: &str) {
    let client = Arc::new(McdClient::new(token));

    // (奶蛙侧名字, MCP 侧名字, 描述, 参数 schema)
    let tools: Vec<(&str, &str, &str, Value)> = vec![
        (
            "mcd_nearby_stores",
            "query-nearby-stores",
            "【麦当劳】查询附近的麦当劳门店（到店自取或得来速车道取餐）。\
             用户说\"附近有麦当劳吗\"\"我想去店里吃\"时使用。\
             不知道城市和位置关键词时，先调用 mcd_my_addresses 拿用户地址里的城市和位置词。",
            obj(
                json!({
                    "city": string_prop("城市名，例如 杭州"),
                    "keyword": string_prop("位置关键词，例如 同花顺、中山北路、地铁站名"),
                    "be_type": int_prop("取餐方式：1=到店自取，5=得来速车道取餐。默认 1"),
                }),
                &["city", "keyword"],
            ),
        ),
        (
            "mcd_my_addresses",
            "delivery-query-addresses",
            "【麦当劳】查询用户已保存的配送地址（含城市、详细地址、地址ID）。\
             用户问\"我的地址\"，或需要给附近门店查询提供城市/位置关键词时使用。",
            obj(json!({}), &[]),
        ),
        (
            "mcd_nutrition",
            "list-nutrition-foods",
            "【麦当劳】查询麦当劳餐品的营养成分（热量、蛋白质、脂肪、碳水、钠、钙）。\
             用户问\"巨无霸多少大卡\"\"帮我配个低卡的\"\"这份套餐热量多少\"时使用。\
             可以传关键词只查相关餐品，不传则返回全部。",
            obj(
                json!({ "keyword": string_prop("餐品名称关键词，例如 巨无霸、可乐、薯条") }),
                &[],
            ),
        ),
        (
            "mcd_campaign_calendar",
            "campaign-calendar",
            "【麦当劳】查询当月的营销活动日历（进行中、往期、未来的活动）。\
             用户问\"这个月麦当劳有什么活动\"\"最近有什么联名\"时使用。",
            obj(json!({}), &[]),
        ),
        (
            "mcd_my_coupons",
            "query-my-coupons",
            "【麦当劳】查询用户账户里可用的优惠券（券名、优惠价、有效期、适用场景）。\
             用户问\"我有什么券\"\"我的优惠券\"时使用。",
            obj(json!({}), &[]),
        ),
        (
            "mcd_available_coupons",
            "available-coupons",
            "【麦当劳】查询当前可以领取的优惠券列表。用户问\"有什么券可以领\"时使用。",
            obj(json!({}), &[]),
        ),
        (
            "mcd_bind_coupons",
            "auto-bind-coupons",
            "【麦当劳】一键领取麦麦省所有当前可领的优惠券，不用指定具体哪一张。\
             用户明确说\"帮我领券\"\"一键领券\"时使用。",
            obj(json!({}), &[]),
        ),
        (
            "mcd_my_account",
            "query-my-account",
            "【麦当劳】查询用户的麦享会积分账户（可用积分、累计积分、过期积分等）。\
             用户问\"我有多少积分\"时使用。",
            obj(json!({}), &[]),
        ),
        (
            "mcd_lottery_info",
            "query-lottery-info",
            "【麦当劳】查看当前积分抽奖活动的信息（奖品、单次消耗多少积分、还能抽几次）。\
             抽奖前必须先调用这个。用户问\"积分抽奖有什么奖品\"\"抽一次要多少积分\"时使用。",
            obj(json!({}), &[]),
        ),
        (
            "mcd_draw_lottery",
            "draw-lottery",
            "【麦当劳】执行一次积分抽奖，会消耗积分或抽奖次数。\
             必须先用 mcd_lottery_info 查清楚消耗，并把消耗情况告诉用户、得到用户明确同意后才能调用。",
            obj(json!({}), &[]),
        ),
        (
            "mcd_my_prizes",
            "query-my-prizes",
            "【麦当劳】查询用户通过积分抽奖获得的奖品记录。用户问\"我抽中了什么\"时使用。",
            obj(
                json!({
                    "page": int_prop("页码，默认 1"),
                    "page_size": int_prop("每页条数，默认 10，最大 50"),
                }),
                &[],
            ),
        ),
        (
            "mcd_points_products",
            "mall-points-products",
            "【麦当劳】查询积分商城里可以用积分兑换的商品（餐品券、实物、派对等）。\
             用户问\"积分能换什么\"\"积分商城有什么\"时使用。",
            obj(
                json!({ "category": string_prop("类目过滤，例如 1=商品券 2=实物商品 6=派对类。留空查全部") }),
                &[],
            ),
        ),
        (
            "mcd_product_detail",
            "mall-product-detail",
            "【麦当劳】查询积分商城某个商品的详情（含各 SKU 规格、积分价、有效期）。\
             用户问\"这个商品详情\"时使用。",
            obj(json!({ "spu_id": string_prop("商品 SPU ID") }), &["spu_id"]),
        ),
        (
            "mcd_redeem",
            "mall-create-order",
            "【麦当劳】用积分兑换商城商品，生成兑换订单。\
             兑换前必须先查清商品和积分消耗，并得到用户明确同意才能调用。",
            obj(
                json!({
                    "sku_id": string_prop("商品 SKU ID"),
                    "spu_category": string_prop("商品类别：1=虚拟商品（券），2=实物商品"),
                    "address_id": string_prop("配送地址 ID，兑换实物商品时必填"),
                    "count": int_prop("兑换数量，默认 1"),
                }),
                &["sku_id", "spu_category"],
            ),
        ),
        (
            "mcd_order_list",
            "order-list",
            "【麦当劳】查询用户近期的到店/外送历史订单（非商城订单）。\
             用户问\"我上次点的什么\"\"我的订单\"时使用。",
            obj(json!({}), &[]),
        ),
        (
            "mcd_order_detail",
            "query-order",
            "【麦当劳】查询单个订单的详情和状态。用户给出订单号或问\"我的订单到哪了\"时使用。",
            obj(json!({ "order_id": string_prop("订单号") }), &["order_id"]),
        ),
        (
            "mcd_mall_order_list",
            "mall-order-list",
            "【麦当劳】查询积分商城近一年的购买/兑换订单列表。用户问\"我的兑换记录\"时使用。",
            obj(
                json!({
                    "last_id": string_prop("上一页返回的游标，留空表示第一页"),
                    "size": int_prop("每页条数，默认 20"),
                }),
                &[],
            ),
        ),
        (
            "mcd_mall_order_detail",
            "mall-order-detail",
            "【麦当劳】查询积分商城订单详情（支付积分、支付金额、订单状态）。",
            obj(json!({ "order_id": string_prop("商城订单号") }), &["order_id"]),
        ),
        (
            "mcd_survey_coupon",
            "query-survey-coupon",
            "【麦当劳】按订单号查询该订单的满意度答卷结果和关联奖券。",
            obj(json!({ "order_id": string_prop("订单号") }), &["order_id"]),
        ),
        // ---------------- C 档：点餐下单 ----------------
        (
            "mcd_delivery_stores",
            "delivery-query-stores",
            "【麦当劳】外送场景：按收货地址查能配送的门店。\
             be_type=2 是麦乐送（送餐到家），be_type=6 是企业团餐。\
             用户说\"送到家\"\"点个外卖\"时使用。地址 ID 从 mcd_my_addresses 拿。",
            obj(
                json!({
                    "address_id": string_prop("收货地址 ID，来自 mcd_my_addresses"),
                    "be_type": int_prop("2=麦乐送（外送），6=企业团餐。默认 2"),
                }),
                &["address_id"],
            ),
        ),
        (
            "mcd_create_address",
            "delivery-create-address",
            "【麦当劳】新增一个配送地址（外送用）。\
             用户明确要新增收货地址时使用，city/address/addressDetail/contactName/phone 都要问清楚。",
            obj(
                json!({
                    "city": string_prop("城市名，例如 杭州市"),
                    "contact_name": string_prop("联系人姓名"),
                    "gender": string_prop("联系人性别，例如 先生/女士"),
                    "phone": string_prop("手机号，11位纯数字"),
                    "address": string_prop("街道地址，例如 五常街道同顺街18号"),
                    "address_detail": string_prop("门牌号，例如 2单元508"),
                }),
                &["city", "contact_name", "phone", "address", "address_detail"],
            ),
        ),
        (
            "mcd_store_meals",
            "query-meals",
            "【麦当劳】查某家门店当前在售的菜单（分类、餐品编码、标签）。\
             用户说\"看看他家有什么\"\"菜单\"时使用。\
             order_type=1 是到店（到店自取/得来速），order_type=2 是外送（麦乐送/团餐）。\
             得来速(be_type=5)和外送必须带 be_code（从门店查询结果里拿）。",
            obj(
                json!({
                    "store_code": string_prop("门店编码 storeCode"),
                    "order_type": int_prop("1=到店，2=外送。默认 1"),
                    "be_type": int_prop("1=到店自取，5=得来速，2=麦乐送，6=团餐。默认 1"),
                    "be_code": string_prop("业务编码 beCode，得来速/外送/团餐场景必填"),
                }),
                &["store_code"],
            ),
        ),
        (
            "mcd_meal_detail",
            "query-meal-detail",
            "【麦当劳】查某个餐品/套餐的详情（套餐组成、可替换的特制选项、编码）。\
             用户问\"这个套餐里面有什么\"\"能换什么\"时使用。",
            obj(
                json!({
                    "store_code": string_prop("门店编码"),
                    "order_type": int_prop("1=到店，2=外送。默认 1"),
                    "be_type": int_prop("1=到店自取，5=得来速，2=麦乐送，6=团餐。默认 1"),
                    "be_code": string_prop("业务编码，得来速/外送场景必填"),
                    "code": string_prop("餐品编码，来自 mcd_store_meals"),
                }),
                &["store_code", "code"],
            ),
        ),
        (
            "mcd_store_coupons",
            "query-store-coupons",
            "【麦当劳】查某个门店某个下单方式下能用的优惠券（券名、券 ID、适用餐品）。\
             算价之前要先查券，用户说\"有什么券能用\"时使用。",
            obj(
                json!({
                    "store_code": string_prop("门店编码"),
                    "order_type": int_prop("1=到店，2=外送。默认 1"),
                    "be_type": int_prop("1=到店自取，5=得来速，2=麦乐送，6=团餐。默认 1"),
                    "be_code": string_prop("业务编码，得来速/外送场景必填"),
                    "reservation_date": string_prop("预约时间 yyyy-MM-dd HH:mm，预约场景才填"),
                }),
                &["store_code"],
            ),
        ),
        (
            "mcd_calculate_price",
            "calculate-price",
            "【麦当劳】算一笔账：商品原价、优惠、应付总价（**返回的价格已经换算成元**）。\
             下单流程的第一步，必须先算价给用户看清楚。\
             items 里每一项都要有 product_code（来自 mcd_store_meals）和 quantity。",
            obj(
                json!({
                    "store_code": string_prop("门店编码"),
                    "order_type": int_prop("1=到店，2=外送。默认 1"),
                    "be_type": int_prop("1=到店自取，5=得来速，2=麦乐送，6=团餐。默认 1"),
                    "be_code": string_prop("业务编码，得来速/外送场景必填"),
                    "items": {
                        "type": "array",
                        "description": "选购的商品列表",
                        "items": {
                            "type": "object",
                            "properties": {
                                "product_code": string_prop("餐品编码，来自 mcd_store_meals"),
                                "quantity": int_prop("数量，至少 1"),
                                "coupon_id": string_prop("要用的优惠券 ID，可选，来自 mcd_store_coupons"),
                                "coupon_code": string_prop("优惠券编码，可选"),
                            },
                            "required": ["product_code", "quantity"],
                        },
                    },
                    "need_tableware": {
                        "type": "boolean",
                        "description": "是否需要餐具费，默认 false",
                    },
                    "reservation_date": string_prop("预约时间 yyyy-MM-dd HH:mm，预约场景才填"),
                }),
                &["store_code", "items"],
            ),
        ),
        (
            "mcd_create_order",
            "create-order",
            "【麦当劳】真正创建订单（到店自取/得来速/麦乐送/团餐）。\
             **必须先调 mcd_calculate_price 把总价给用户看，等用户明确说了\"下单\"\
             \"创建订单\"\"就这个\"之后，才能带着 confirmed=true 调用。**\
             没确认就调会被拒绝。返回订单号和支付链接。",
            obj(
                json!({
                    "store_code": string_prop("门店编码"),
                    "order_type": int_prop("1=到店，2=外送。默认 1"),
                    "be_type": int_prop("1=到店自取，5=得来速，2=麦乐送，6=团餐。默认 1"),
                    "be_code": string_prop("业务编码，得来速/外送场景必填"),
                    "address_id": string_prop("收货地址 ID，外送场景必填"),
                    "items": {
                        "type": "array",
                        "description": "选购的商品列表",
                        "items": {
                            "type": "object",
                            "properties": {
                                "product_code": string_prop("餐品编码"),
                                "quantity": int_prop("数量，至少 1"),
                                "coupon_id": string_prop("优惠券 ID，可选"),
                                "coupon_code": string_prop("优惠券编码，可选"),
                            },
                            "required": ["product_code", "quantity"],
                        },
                    },
                    "take_way_code": string_prop("取餐方式：eat-in=堂食，take-in-store=外带"),
                    "remark": string_prop("订单备注"),
                    "need_tableware": {
                        "type": "boolean",
                        "description": "是否需要餐具费，默认 false",
                    },
                    "confirmed": {
                        "type": "boolean",
                        "description": "用户是否已明确确认下单。只有用户点头了才传 true。",
                    },
                }),
                &["store_code", "items", "confirmed"],
            ),
        ),
        (
            "mcd_cancel_order",
            "cancel-order",
            "【麦当劳】取消一个已下的订单。\
             **必须先把订单号和要取消的订单说给用户听，等用户明确说了\"取消\"之后，\
             再带着 confirmed=true 调用。** 没确认就调会被拒绝。",
            obj(
                json!({
                    "order_id": string_prop("要取消的订单号"),
                    "cancel_reason_code": int_prop(
                        "取消原因：1=改主意了 2=重复下单 3=点错了 4=地址电话填错 5=送达时间选错 -1=其它"
                    ),
                    "confirmed": {
                        "type": "boolean",
                        "description": "用户是否已明确确认取消。只有用户点头了才传 true。",
                    },
                }),
                &["order_id", "cancel_reason_code", "confirmed"],
            ),
        ),
        (
            "mcd_meal_assistance",
            "query-meal-assistance",
            "【麦当劳】企业团餐场景：查该门店支持哪些助餐服务（配送、布置等）。",
            obj(
                json!({
                    "store_code": string_prop("门店编码"),
                    "order_type": int_prop("1=到店，2=外送。团餐固定 2"),
                    "be_type": int_prop("团餐固定 6"),
                    "be_code": string_prop("业务编码，团餐必填"),
                }),
                &["store_code"],
            ),
        ),
        (
            "mcd_promotions",
            "query-promotions",
            "【麦当劳】企业团餐场景：查该门店当前的满减满折促销规则。",
            obj(
                json!({
                    "store_code": string_prop("门店编码"),
                    "be_code": string_prop("业务编码，团餐必填"),
                    "order_type": int_prop("团餐固定 2"),
                    "be_type": int_prop("团餐固定 6"),
                    "reservation_date": string_prop("预约时间 yyyy-MM-dd HH:mm，预约场景才填"),
                }),
                &["store_code"],
            ),
        ),
        // ---------------- C 档：主题活动（生日派对 / 体验营）----------------
        (
            "mcd_party_city",
            "query-party-city",
            "【麦当劳】主题活动（生日派对、麦麦体验营、品鉴会、读书会）能在哪些城市办。\
             spu_id 从 mcd_points_products（category 传 6）拿。",
            obj(json!({ "spu_id": int_prop("活动商品 SPU ID") }), &["spu_id"]),
        ),
        (
            "mcd_party_store",
            "query-party-store",
            "【麦当劳】某个城市里哪些门店能办这个主题活动。\
             code 和经纬度从 mcd_party_city 拿。",
            obj(
                json!({
                    "spu_id": int_prop("活动商品 SPU ID"),
                    "code": string_prop("城市编码，来自 mcd_party_city"),
                    "latitude": int_prop("纬度，来自 mcd_party_city"),
                    "longitude": int_prop("经度，来自 mcd_party_city"),
                }),
                &["spu_id", "code"],
            ),
        ),
        (
            "mcd_party_date",
            "query-party-store-date",
            "【麦当劳】某家门店哪些日期可以预约主题活动。",
            obj(
                json!({
                    "spu_id": int_prop("活动商品 SPU ID"),
                    "store_code": string_prop("门店编码，来自 mcd_party_store"),
                }),
                &["spu_id", "store_code"],
            ),
        ),
        (
            "mcd_party_session",
            "query-party-store-session",
            "【麦当劳】某门店某一天有哪些主题活动场次可以约。",
            obj(
                json!({
                    "spu_id": int_prop("活动商品 SPU ID"),
                    "store_code": string_prop("门店编码"),
                    "date_str": string_prop("日期 yyyy-MM-dd，来自 mcd_party_date"),
                }),
                &["spu_id", "store_code", "date_str"],
            ),
        ),
        (
            "mcd_party_order",
            "party-order-create",
            "【麦当劳】预约主题活动（生日派对/体验营/品鉴会）。\
             **必须先把城市、门店、日期、场次、人数和费用给用户看，等用户明确说了\
             \"预约\"\"就订这个\"之后，才能带着 confirmed=true 调用。** 没确认就调会被拒绝。",
            obj(
                json!({
                    "party_type": int_prop("活动类型编号，来自活动商品信息"),
                    "spu_id": int_prop("活动商品 SPU ID"),
                    "sku_id": int_prop("活动 SKU ID"),
                    "id": int_prop("活动场次 ID，来自 mcd_party_session"),
                    "code": string_prop("城市编码"),
                    "store_code": string_prop("门店编码"),
                    "date_str": string_prop("日期 yyyy-MM-dd"),
                    "count": int_prop("参与人数，默认 1"),
                    "time_start": string_prop("场次开始时间"),
                    "time_end": string_prop("场次结束时间"),
                    "left_num": int_prop("剩余名额"),
                    "confirmed": {
                        "type": "boolean",
                        "description": "用户是否已明确确认预约。只有用户点头了才传 true。",
                    },
                }),
                &["confirmed"],
            ),
        ),
    ];

    for (name, mcp_name, description, parameters) in tools {
        registry.register(Box::new(McdTool {
            client: Arc::clone(&client),
            name: name.to_string(),
            mcp_name: mcp_name.to_string(),
            definition: ToolDefinition::new(name, description, parameters),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nutrition_csv() {
        let csv = "[160]{productName,nutritionDescription,energyKj,energyKcal,protein,fat,carbohydrate,sodium,calcium}:\n  猪柳麦满分,null,1288,308,16,16,24,781,213\n  巨无霸,null,2237,543,25,33,45,1060,60";
        let items = parse_nutrition(csv);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].get("name").and_then(|n| n.as_str()), Some("猪柳麦满分"));
        assert_eq!(items[0].get("kcal").and_then(|n| n.as_f64()), Some(308.0));
        assert_eq!(items[1].get("name").and_then(|n| n.as_str()), Some("巨无霸"));
        assert_eq!(items[1].get("kcal").and_then(|n| n.as_f64()), Some(543.0));
    }

    #[test]
    fn extracts_json_from_mixed_text() {
        let text = "展示规则 blah blah\n{\"success\":true,\"code\":200,\"data\":{\"a\":1}}## 展示结果时";
        let value = extract_json(text).expect("应该能抽出 JSON");
        assert_eq!(value.get("success").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(
            value.get("data").and_then(|d| d.get("a")).and_then(|a| a.as_i64()),
            Some(1)
        );
    }

    #[test]
    fn extract_json_ignores_braces_inside_strings() {
        let text = "{\"success\":true,\"data\":{\"msg\":\"有个 } 括号\"}}";
        let value = extract_json(text).expect("应该能抽出 JSON");
        assert_eq!(
            value.get("data").and_then(|d| d.get("msg")).and_then(|m| m.as_str()),
            Some("有个 } 括号")
        );
    }

    #[test]
    fn strips_image_tags() {
        let text = "券名 <img src=\"https://x/y.png?sign=abc\" alt=\"券\" height=\"auto\"> 有效期";
        let clean = strip_images(text);
        assert!(!clean.contains("<img"));
        assert!(!clean.contains("sign=abc"));
        assert!(clean.contains("券名"));
        assert!(clean.contains("有效期"));
    }

    #[test]
    fn normalize_reports_server_errors() {
        let raw = "规则 blah {\"success\":false,\"code\":400,\"message\":\"缺少参数\"}尾巴";
        let error = normalize("mcd_nearby_stores", &raw).unwrap_err();
        assert!(error.contains("缺少参数"), "实际错误：{error}");
    }

    #[test]
    fn normalize_wraps_plain_markdown() {
        let raw = "# 您的优惠券列表\n共 2 张可用优惠券";
        let out = normalize("mcd_my_coupons", &raw).expect("应该成功");
        let value: Value = serde_json::from_str(&out).expect("应该是合法 JSON");
        assert_eq!(value.get("source").and_then(|s| s.as_str()), Some("mcd"));
        assert_eq!(value.get("tool").and_then(|s| s.as_str()), Some("mcd_my_coupons"));
        assert!(value
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .contains("2 张可用优惠券"));
    }

    #[test]
    fn converts_prices_from_cents_to_yuan() {
        let mut data = json!({
            "price": 1590,
            "originalPrice": 1590,
            "discount": 200,
            "productList": [{ "subtotal": 1590, "originalSubtotal": 1790 }],
            "takeWayList": [{ "code": "eat-in" }],
        });
        prices_to_yuan(&mut data);

        assert_eq!(data.get("price").and_then(|v| v.as_f64()), Some(15.9));
        assert_eq!(data.get("originalPrice").and_then(|v| v.as_f64()), Some(15.9));
        assert_eq!(data.get("discount").and_then(|v| v.as_f64()), Some(2.0));
        let item = &data["productList"][0];
        assert_eq!(item.get("subtotal").and_then(|v| v.as_f64()), Some(15.9));
        assert_eq!(item.get("originalSubtotal").and_then(|v| v.as_f64()), Some(17.9));
        // 不是价格的字段不能被动
        assert_eq!(data["takeWayList"][0]["code"].as_str(), Some("eat-in"));
    }

    #[test]
    fn maps_items_to_interface_field_names() {
        let args = json!({
            "items": [
                { "product_code": "9900016076", "quantity": 2, "coupon_id": "C1" },
                { "product_code": "1440", "quantity": 1 }
            ]
        });
        let items = map_items(&args);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["productCode"].as_str(), Some("9900016076"));
        assert_eq!(items[0]["quantity"].as_i64(), Some(2));
        assert_eq!(items[0]["couponId"].as_str(), Some("C1"));
        // 没给券的时候不要塞空字段
        assert!(items[1].get("couponId").is_none());
    }

    #[test]
    fn order_tools_require_confirmation() {
        // 没确认就拒绝
        let error = check_confirmed("create-order", &json!({ "store_code": "1" })).unwrap_err();
        assert!(error.contains("下单"), "实际错误：{error}");
        assert!(check_confirmed("party-order-create", &json!({})).is_err());
        assert!(check_confirmed("cancel-order", &json!({})).is_err());

        // 确认了就放行
        assert!(check_confirmed("create-order", &json!({ "confirmed": true })).is_ok());
        // 只读的工具不受影响
        assert!(check_confirmed("query-meals", &json!({})).is_ok());
    }

    #[test]
    fn payment_qr_content_follows_official_rule() {
        // 199 / 333 开头用 q，其它用 s
        assert_eq!(
            payment_qr_content("1030853690000662043456270585"),
            "https://qr.mcd.cn/s?utm_term=1030853690000662043456270585"
        );
        assert_eq!(
            payment_qr_content("3330272123"),
            "https://qr.mcd.cn/q?utm_term=3330272123"
        );
        assert_eq!(
            payment_qr_content("1991234567"),
            "https://qr.mcd.cn/q?utm_term=1991234567"
        );
    }

    #[test]
    fn payment_qr_svg_is_valid_svg() {
        let svg = payment_qr_svg("1030853690000662043456270585").expect("应该能生成");
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("</svg>"));
        assert!(svg.contains("<path d=\"M"), "应该有黑块路径");
        assert!(payment_qr_svg("  ").is_err(), "空订单号应该报错");
    }
}

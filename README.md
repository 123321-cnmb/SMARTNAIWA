# Naiwa Pet（奶蛙）

奶蛙——一只住在桌面上的 **Agent 桌宠**。Tauri 2 + Web 前端 + Rust 编排，桌宠外壳是本地渲染的，真正的「大脑」在 Rust 侧：它能调用工具查真实行情，能记住你让它记住的东西。

现在的主线能力是**股票行情助手**：你随口问一句，奶蛙自己去查、自己算，然后用它自己的口吻汇报。

奶蛙**内置了一个可用的 API Key**，下载解压就能直接聊、就能查行情，什么都不用配。
想换成自己的 Key（或者换用 DeepSeek 之类别的服务），在设置窗口里改就行，详见下面
「换成自己的 Key」。配置文件存在 `%APPDATA%\naiwa-pet\config.json`。

> 内置 Key 是所有使用者共用的，额度有限也会被滥用。开源分发前建议改成
> 「首次运行引导用户填自己的 Key」。

## 它能做什么

| 能力 | 说明 |
| --- | --- |
| 流式对话 | 逐字回复，不是转圈等半天 |
| 工具调用 | 模型自主决定调哪个工具、什么顺序、要调几次 |
| 股票搜索 | 名称 / 代码 / 拼音首字母都能搜 |
| 实时行情 | 现价、涨跌幅、开高低、成交量额、换手率、量比、PE、PB、市值、涨跌停价 |
| 分时图 | 价格线 + 均价线 + 昨收基准线，涨红跌绿填充，左价格右涨跌幅双轴，成交量副图 |
| K 线图 | 蜡烛图 + 成交量副图，日/周/月切换，MA5/10/20/60 均线可叠加，悬停看单根明细 |
| 自选股 | 增删查，持久化到 `%APPDATA%\naiwa-pet\watchlist.json` |
| 涨跌提醒 | 每 60 秒轮询自选股，涨跌幅超 2% 就在**聊天窗口**里说一声，**涨了还会播大笑动画** |
| 麦当劳门店 | 问一句「附近有麦当劳吗」，奶蛙自己去查门店、距离、营业时间 |
| 餐品热量 | 160 个餐品的能量、蛋白、脂肪、碳水、钠、钙，能配低卡套餐 |
| 优惠券 | 手上的券、能领的券都能看，一键领券也行 |
| 积分 | 麦享会积分余额、积分商城、积分抽奖活动与奖品 |
| 活动日历 | 当月麦当劳的联名和促销，含价格和开售时间 |
| **点餐下单** | 查菜单 → 查券 → 算价 → 问你要不要下单 → 点头才下单，给**支付二维码** |
| **取消订单** | 把订单念给你听，你说了「取消」才真的取消 |
| **主题活动** | 生日派对、麦麦体验营、品鉴会的城市/门店/日期/场次查询与预约 |
| 托盘图标 | 常驻后台也能找到它：显示/隐藏、打开聊天、设置、退出 |

> 麦当劳的能力走**麦当劳中国官方 MCP**（`https://mcp.mcd.cn`），在 Rust 侧实现，
> 对模型暴露成一组 `mcd_` 开头的工具，**入口就是「和奶蛙聊天」，不开新窗口**。
> 实现在 `src-tauri/src/agent/tools/mcd.rs`。
>
> **MCP Token 是配置项，不是内置的。** 它绑定的是**个人麦当劳账户**（手机号登录后
> 申请），能查券、下单、查订单和收货地址——打进 exe 等于把账户开放给所有拿到包的人。
>
> **开通入口就在聊天窗口里**：没配 Token 时，聊天窗顶部会出现一张「开通麦当劳」卡片——
> 点「在这里直接登录」会在卡片里**直接嵌入麦当劳官方的开放平台页**，全程不切浏览器：
> 点右上角菜单 →「登录」→ **手机扫二维码**登录 → 回首页点「控制台」→「激活」申请 Token
> → 复制回来粘到卡片的输入框里点保存，**立刻生效，不用重启**（Rust 侧会重建工具表）。
>
> ⚠️ 嵌入的必须是**首页** `open.mcd.cn/mcp`，不是登录页：
> - `/mcp/login` 是**扫码登录**页（对，要用微信/企微扫码）
> - `/mcp/auth/login` 是**手机号+验证码**页，但登录完会跳 `/mcp/auth` OAuth 授权页，
>   那个页要 `client_id` 查询参数 → 直接报「**缺少clientId参数**」。
>
> 官方页面没有 iframe 限制（实测所有路由都没有 `X-Frame-Options` / CSP），
> 所以能直接嵌。跨域读不到 iframe 里的内容，所以 Token 仍要用户自己复制回来。
> 链接 `open.mcd.cn/mcp` 也保留了，用系统浏览器打开（手机上开更方便）。
>
> ⚠️ 跨域 iframe 默认**没有 `clipboard-write` 权限**（Permissions Policy），
> 官方页面点「复制」调 `navigator.clipboard.writeText` 会直接 reject，页面上弹「复制失败」。
> 所以 iframe 上必须显式挂 `allow="clipboard-write; clipboard-read"`（`src/main.ts`）。
> 实测对照（可信鼠标事件，非合成 `click`）：无 `allow` → 权限 `denied` → `NotAllowedError`；
> 有 `allow` → 权限 `granted` → 写入成功。
> 万一还是不行，点 Token 输入框 `Ctrl+A` 全选、`Ctrl+C` 手动复制一样能用。

### 首次启动

**第一次启动**（`%APPDATA%\naiwa-pet\config.json` 还不存在）时，聊天窗顶部是一张
「欢迎使用奶蛙」卡片，一次把两样都收了：

- **API Key** —— 调模型用，必填
- **麦当劳 MCP Token** —— 想查麦当劳才填，选填

填完点保存，之后就不再打扰了。**分发包里不含任何人的 API Key 或 MCP Token**：
模型 Key 是内置的共享 Key，麦当劳 Token 完全是使用者自己的。

> app 拥有 `config.json`：程序运行期间在外部改这个文件会被它覆盖回去（心跳保存
> 情绪时会重写整个文件）。要改配置请用设置窗口或聊天里的卡片。
>
> 也可以用环境变量 `MCD_MCP_TOKEN` 覆盖，或者在设置窗口里填（那里还能一键清除）。

## 窗口布局

桌宠只有**一个本体窗口**，其它都是它的延伸，而且**都跟着奶蛙一起移动**：

| 窗口 | 位置 | 打开方式 |
| --- | --- | --- |
| 奶蛙 | — | 启动就在 |
| 奶蛙聊天 | 奶蛙**左侧**，底边对齐 | 右键奶蛙 →「和奶蛙聊一聊」 |
| K 线图 | 奶蛙**右上** | 聊天里点行情卡片 |
| 设置 | 奶蛙**右下** | 右键奶蛙 →「设置…」，右上角 `×` 关闭 |

**所有文字都走聊天窗口**——奶蛙拖拽时的吐槽、涨跌提醒、心跳主动冒泡，
全部出现在聊天里，桌宠不会往屏幕上到处贴小纸条。

右键奶蛙的菜单只有三项：**和奶蛙聊一聊** / **设置…** / **退出**。
表演性的动作（大笑、害羞、睡觉）不在菜单里——那是奶蛙自己决定要不要做的。

## 桌面效果

![Naiwa Pet 桌面演示](public/demo.webp)

## 快速上手

### 一、开箱即用

**下载解压就能用，不需要装、不需要配任何东西。** 奶蛙内置了一个可用的 API Key。

Windows 可能弹「Windows 已保护你的电脑」，点「更多信息 → 仍要运行」——
这是因为程序没有买代码签名证书，不是病毒。

右键奶蛙 →「和奶蛙聊一聊」，问一句「查一下贵州茅台现在的行情」试试。

### 二、换成自己的 Key（可选）

内置 Key 是所有使用者共用的，额度有限。换成自己的更稳，也更自由
（DeepSeek、GPT-4o 之类任何 OpenAI 兼容端点都行）。

**方式一：界面配置（推荐）** —— 右键托盘图标 →「设置…」→ 填 Key → 保存。
配置文件在 `%APPDATA%\naiwa-pet\config.json`。

**方式二：环境变量** —— `MINIMAX_API_KEY`、`MINIMAX_BASE_URL`、`MINIMAX_MODEL`。
环境变量优先级高于配置文件。

**方式三：直接改配置文件**

```json
{
  "api_key": "sk-你的key",
  "base_url": "https://api.minimaxi.com/v1",
  "model": "MiniMax-M3"
}
```

`base_url` 和 `model` 留空会用默认值。换 DeepSeek 就填
`https://api.deepseek.com/v1` + 模型 `deepseek-chat`。

> 模型需要支持 function calling（工具调用），否则奶蛙只会聊天不会干活。
> MiniMax-M3、DeepSeek、GPT-4o 系列都支持。

### 三、跑起来（开发）

```sh
npm install
npm run tauri:dev
```

### 四、打包给别人

```sh
npm run build:win
```

产出两个东西：

| 产物 | 说明 |
| --- | --- |
| `src-tauri/target/.../bundle/nsis/奶蛙_0.1.0_x64-setup.exe` | 安装版，双击安装 |
| `dist-portable.zip` | **免安装版**，解压双击 `奶蛙.exe` 就能用 |

`build:win` 会用 `vswhere` 探测 MSVC：有就用 MSVC，没有就降级 GNU + 一套 workaround，
两种环境都能出包。

> 装好的程序依赖系统的 **WebView2 Runtime**。安装包内嵌了 bootstrapper：
> 目标机器没有 WebView2 时安装过程会自动联网补装；目标机器完全不能联网时，
> 把 `tauri.conf.json` 里的 `webviewInstallMode` 改成 `"offlineInstaller"`
> （安装包会涨到约 230 MB，发不动的话慎重）。

## 奶蛙是个什么性格

人格写在 `src-tauri/src/agent/persona.rs`，每次对话都会带上。核心几条：

- **爱笑** —— 看到有意思的事会忍不住笑。笑是**真的笑**（播动画），不是文字里打「哈哈哈」。
- **黏人** —— 长时间没人理它会自己找点事做，但一天最多主动说两三次。
- **实诚** —— 查不到就说查不到，**绝不假装自己做过什么**。

### 奶蛙的动作

| 动作 | 表现 | 怎么来的 |
| --- | --- | --- |
| `laugh` | 会心一笑 | 现成的帧段动画（0-200 帧） |
| `big_laugh` | 哈哈大笑 | 现成的帧段动画（200-438 帧） |
| `shy` | 缩小、左右小幅摇 | 待机帧 + CSS 变换 |
| `poke` | 放大往上冲 | 待机帧 + CSS 变换 |
| `sleep` | 闭眼、缩小下沉、变暗 | 待机帧 + CSS 变换 + 强制闭眼 |

没有专门素材的动作用**同一套素材程序化演出来**，风格天然统一，不用另外找一套画风一致的图。

两个细节：

- **身体和眼球是两个叠放的元素**，做变换时必须一起动，否则会错位。
- **睡觉是闭着眼**的——靠眼球层的「强制闭眼」状态实现，而不是简单隐藏图层（藏了图层就只剩空白了）。

另外待机时奶蛙会**偶尔眨眼**（眼球纵向压成一条缝），这个小动作让它显得是活着的，而不是一张会动的贴图。

### 自主行为

奶蛙不是「问一句答一句」的机器人，有两套自发机制：

| 机制 | 触发 | 行为 |
| --- | --- | --- |
| **表达工具** | 对话中，模型自己觉得好笑/开心/被夸得不好意思时 | 调用 `express` 工具，**真的播动画**（会心一笑 / 哈哈大笑） |
| **心跳** | 每 3~9 分钟醒一次 | 自己决定说点什么，或者调用 `stay_quiet` 保持安静 |

**关键设计**：表情动作必须走 `express` 工具，由 Rust 侧广播给桌宠窗口播动画。
如果只在提示词里写「你开心就笑」，模型会开始演独角戏——文字里写「哈哈哈」，
实际什么都没发生。

心跳的频率可以用环境变量调（调试用）：

```sh
NAIWA_HEARTBEAT_STARTUP_SECS=12    # 启动后多久第一次心跳
NAIWA_HEARTBEAT_SECS=25             # 每轮间隔
NAIWA_HEARTBEAT_COOLDOWN_SECS=30    # 两次主动开口的最小间隔
```

### 情绪

奶蛙有三个情绪维度（开心 / 精力 / 好奇），会影响说话的语气，也会影响它想不想主动找你。
情绪随时间衰减，聊天会变开心，深夜会犯困——**存在 `config.json` 里，重启不丢**。

## 常用命令

```sh
npm install
npm run assets:naiwa        # 从 naiwa.mp4 重新生成宠物素材
npm run tauri:dev           # 开发模式
npm run build:win           # 打包 NSIS 安装包（自动挑工具链，推荐）
npm run tauri:build -- --target x86_64-pc-windows-msvc --bundles nsis   # 手动指定 MSVC
```

## 代码签名（可选）

Windows SmartScreen 会拦未签名的程序（用户得点「更多信息 → 仍要运行」）。
**这不影响使用**，多数人点一次就过了。

想要彻底消掉这个提示，开源项目可以走 **SignPath Foundation** 的免费签名通道：

1. 仓库必须是**公开**的、且带 OSI 许可证（本项目用 MIT ✓）
2. 在 <https://signpath.org/apply> 申请 Open Source 免费计划，
   填仓库地址、许可证、简介、你的邮箱
3. 审批通过后，在 <https://app.signpath.io> 里：
   - 建 Organization，记下 Organization ID
   - 建 Project（slug 随意），关联本仓库的 GitHub.com trusted build system
   - 加 Artifact Configuration，内容用 `.signpath/artifact-configuration.xml`（slug 建议 `windows-installer`）
   - 加 Signing Policy，绑定 Foundation 证书，**要求手动审批**（slug 建议 `release-signing`）
   - 建一个带 submitter 权限的 API Token
4. 在仓库 Settings → Secrets and variables → Actions 里加：

   | 种类 | 名称 | 值 |
   | --- | --- | --- |
   | Secret | `SIGNPATH_API_TOKEN` | 上一步的 API Token |
   | Variable | `SIGNPATH_ORGANIZATION_ID` | Organization ID |
   | Variable | `SIGNPATH_PROJECT_SLUG` | 项目 slug |
   | Variable | `SIGNPATH_SIGNING_POLICY_SLUG` | 签名策略 slug |
   | Variable | `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG` | 产物配置 slug |

5. 打 tag 触发签名：

```sh
git tag v0.1.0
git push origin v0.1.0
```

没配 SignPath 的话，workflow 会跳过签名、照常发未签名的包，不会卡住。

签名策略见 [CODE_SIGNING_POLICY.md](CODE_SIGNING_POLICY.md)。

> SignPath 的 action 输入项偶尔会改名，配置时以
> <https://github.com/signpath/github-action-submit-signing-request> 的 README 为准。

## 目录结构

```
src/
├── main.ts               # 4 种窗口模式：pet / chat / chart / settings
└── styles.css
src-tauri/src/
├── lib.rs                # 命令注册、会话状态、托盘图标、自选股监控
└── agent/
    ├── llm.rs            # MiniMax 流式客户端，tool_calls 按 index 归并
    ├── sse.rs            # SSE 帧解析（按空行切，不能按 "data:" 切）
    ├── runner.rs         # Agent 主循环，最多 8 轮工具
    ├── message.rs        # OpenAI 兼容的消息结构
    ├── events.rs         # 推给前端的事件协议
    └── tools/
        ├── stock.rs      # 行情数据源 + 4 个股票工具
        ├── watchlist.rs  # 自选股持久化 + 3 个自选股工具
        └── mcd.rs        # 麦当劳 MCP 客户端 + 34 个麦当劳工具
scripts/
├── build-windows.ps1     # 自动挑工具链并打包（npm run build:win）
├── windres-shim.cmd      # GNU 降级路径的资源编译器适配
└── sign-windows.ps1      # 代码签名（需自备代码签名证书）
```

## 数据源

A股行情走腾讯财经的公开接口，**不需要任何 API key、账号或注册**，也没有请求频率限制：

- 实时行情 `qt.gtimg.cn/q=sh600000`（支持逗号分隔批量）
- K 线 `web.ifzq.gtimg.cn/appstock/app/newfqkline/get`
- 分时 `web.ifzq.gtimg.cn/appstock/app/day/query`
- 搜索 `smartbox.gtimg.cn/s3/?q=茅台`

响应编码**不统一**，这是最容易踩的坑：

- `qt.gtimg.cn`（实时行情）和 `smartbox.gtimg.cn`（搜索）是 **GBK**
- `web.ifzq.gtimg.cn` 的 `day/query`（分时）和 `newfqkline/get`（K线）是 **UTF-8**

Rust 侧的做法是：JSON 接口优先严格 UTF-8 解码，失败再退回 GBK（`encoding_rs` 的 `UTF_8.decode()` 会返回 `had_errors` 标志）。用 GBK 去解 UTF-8 会把中文吞掉，JSON 结构都可能坏掉。

A 股习惯**红涨绿跌**，跟欧美相反，UI 里别搞反。

> **腾讯会下线老接口**。2026-10-23 就发现原来的 `fqkline/get`（K线）和
> `minute/query`（分时）双双返回 501，换成了 `newfqkline/get` 和 `day/query`。
> K线那边还留了 `kline/kline` 做降级源，分时只有一个可用源。
> 以后再遇到「加载失败」，先用 `curl` 试接口，多半是腾讯又换路径了。

> 全市场涨跌幅排行 / 板块排行目前拿不到（东财和新浪的排行接口在部分网络环境不可达），所以奶蛙不做「今日涨幅榜」，自选股的排序是本地算的。

### 麦当劳 MCP

麦当劳的能力走[麦当劳中国官方 MCP](https://open.mcd.cn/mcp)：

- 接入地址 `https://mcp.mcd.cn`，Streamable HTTP 传输
- 每个请求带 `Authorization: Bearer <token>`，**没有 token 一个工具都调不了**（403）
- 限流 600 次/分钟
- Token 在 <https://open.mcd.cn/mcp> 用手机号登录后申请

Rust 侧自己实现了 MCP 客户端（`src-tauri/src/agent/tools/mcd.rs`），没引新依赖。
这里有两个必须处理的坑：

1. **返回值是「markdown 展示规则 + JSON」的混合体。** 规则那一大段是给 MCP 客户端的
   展示约定（"必须完整展示 storeCode"之类），对模型没用还占 token。所以 Rust 侧
   扫平衡括号把 JSON 抽出来单独返回。
2. **但不是所有接口都返回 JSON。** 券列表、活动日历返回的是纯 markdown，
   没有 JSON 可抽——两条路都得走。

另外要注意**请求体必须是 UTF-8 字节**：用 PowerShell 之类默认按 ANSI 编码发中文，
服务端会报「缺少参数」，而 schema 看着是对的，很容易查半天。

麦当劳的工具在模型那边叫 `mcd_` 开头（`mcd_nearby_stores` / `mcd_nutrition` /
`mcd_my_coupons` / `mcd_my_account` / `mcd_lottery_info` / `mcd_store_meals` /
`mcd_calculate_price` / `mcd_create_order` …），一共 34 个。

### 下单是分两步的

写操作（`mcd_create_order` / `mcd_cancel_order` / `mcd_party_order`）在 Rust 侧有
一道**硬闸**：模型必须带 `confirmed: true` 才会真的发请求，否则直接被拒。

系统提示词里也写死了流程：先查菜单和券 → `mcd_calculate_price` 算价 →
把餐品、优惠、总价说给用户听并问一句「要我下单吗？」→ 用户点头了才调下单。

另外几个容易踩的细节：

- 算价/下单接口返回的价格单位是**分**（1590 = 15.90 元），Rust 侧统一换算成元，
  并在数据里标 `unit`，免得模型再除一次 100。菜单里的 `currentPrice` 本来就是元，不要动。
- **到店和外送是两套菜单，编码不一样**（巨无霸：到店 `1100`、外送 `901100`）。
  混用了编码，接口不会报错，而是把商品**悄悄丢掉**，只返回配送费——算出来是 6 元。
  Rust 侧发现 `productList` 为空会直接报错，让模型用当前场景的菜单编码重查。
- 支付链接 `m.mcd.cn/mcp/scanToPay?orderId=xxx` 在**电脑上打不开**（那是给手机用的
  H5）。所以下单后奶蛙会按官方前端的规则拼出扫码地址
  （`https://qr.mcd.cn/{q|s}?utm_term={orderId}`），自己画成二维码贴进聊天里，
  手机扫一下就能付。

## 宠物素材

默认宠物素材位于 `public/pets/naiwa`。

- `manifest.json` 描述动画（帧数、fps、宽高、音频）。
- `frames/*.webp` 是透明背景的动画帧。
- `audio.m4a` 是从源素材中提取的音频。

如果需要从 `naiwa.mp4` 重新生成内置素材：

```sh
npm run assets:naiwa
```

以后要添加其他宠物，创建结构相同的素材目录，并更新 `src/main.ts` 中的 `DEFAULT_PET_ID`。

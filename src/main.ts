import { Menu, MenuItem, PredefinedMenuItem } from "@tauri-apps/api/menu";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  getCurrentWindow,
  LogicalPosition,
  LogicalSize,
  PhysicalPosition,
} from "@tauri-apps/api/window";
import { Channel, invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import "./styles.css";

/* ------------------------------------------------------------------ *
 * 类型
 * ------------------------------------------------------------------ */

type PetManifest = {
  id: string;
  name: string;
  width: number;
  height: number;
  fps: number;
  frameCount: number;
  idleFrame: string;
  framePattern: string;
  audio: string;
};

type LaughSegment = {
  id: string;
  text: string;
  startFrame: number;
  endFrame: number;
};

// 奶蛙能做的动作。laugh / big_laugh 用现成的帧段，
// 其余用待机帧 + CSS 变换程序化演出来。
type PetAction = "laugh" | "big_laugh" | "shy" | "poke" | "sleep";

type WindowMode = "pet" | "chat" | "chart" | "settings";

// Rust 侧 Agent 推回来的事件。
type AgentEvent =
  | { type: "assistant"; content: string }
  | { type: "tool_call"; name: string; args: Record<string, unknown> }
  | { type: "tool_result"; name: string; result: string }
  | { type: "tool_error"; name: string; error: string }
  | { type: "express"; action: string; reason: string }
  | { type: "done"; final_text: string }
  | { type: "error"; message: string };

type QuotePayload = {
  code: string;
  name: string;
  price: number;
  change: number;
  change_pct: number;
  open: number;
  high: number;
  low: number;
  prev_close: number;
  volume: number;
  amount: number;
  turnover: number;
  pe: number;
  total_mcap: number;
};

type MatchPayload = {
  matches: { code: string; name: string; pinyin: string }[];
};

// 麦当劳 MCP 工具返回的统一结构（Rust 侧归一化过的）。
type McdPayload = {
  source: "mcd";
  tool: string;
  data?: unknown;
  text?: string;
};

type KlinePoint = {
  date: string;
  open: number;
  close: number;
  high: number;
  low: number;
  volume: number;
};

type KlineSeries = {
  code: string;
  period: string;
  items: KlinePoint[];
};

type IntradayPoint = {
  time: string;
  price: number;
  volume: number;
  amount: number;
};

type IntradaySeries = {
  code: string;
  name: string;
  prev_close: number;
  price: number;
  change_pct: number;
  date: string;
  items: IntradayPoint[];
};

type ConfigView = {
  has_key: boolean;
  key_hint: string;
  using_builtin: boolean;
  base_url: string;
  model: string;
  config_path: string;
  /** 有没有配麦当劳 MCP Token（只给"有没有"，不给内容） */
  mcd_token_set: boolean;
  /** 是不是第一次启动（配置文件还不存在） */
  first_run: boolean;
};

/* ------------------------------------------------------------------ *
 * 常量
 * ------------------------------------------------------------------ */

const DEFAULT_PET_ID = "naiwa";
const APP_NAME = "奶蛙";

// 笑的强度分段。素材本身就是一段"越来越夸张的大笑"（438 帧 @30fps ≈ 14.6 秒）：
//   帧 0    ~200  待机 → 仰头大笑
//   帧 ~200 ~438  双手举过头顶捶打 → 笑到躺地上打滚
// 只按帧号切分，音频起点用 startFrame / fps 换算成秒，画面和声音严格对齐。
const LAUGH_SEGMENTS: LaughSegment[] = [
  { id: "laugh", text: "大笑", startFrame: 0, endFrame: 200 },
  { id: "super-laugh", text: "超级大笑", startFrame: 200, endFrame: 438 },
];

// 大笑结束到常态之间做交叉淡入淡出，不要硬切。
const FADE_MS = 420;

// 待机时眼球左右转的周期和幅度：幅度大一些、周期慢一点，看着像在东张西望。
const EYE_SWAY_PERIOD = 3200;
const EYE_SWAY_AMPLITUDE = 9;

// 眨眼节奏：每隔多久眨一次，一次眨多久。
const BLINK_PERIOD = 3800;
const BLINK_DURATION = 130;

const BASE_PET_WIDTH = 180;
// 窗口与奶蛙的**重叠量**（物理像素）：轻微压上去，比留空隙更贴合
const SIDE_OVERLAP = 10;
// 聊天框宽度 = 高度的 0.78
const CHAT_WIDTH_RATIO = 0.78;
const CHAT_MIN_SIZE = { width: 280, height: 340 };
const CHART_SIZE = { width: 520, height: 400 };
const SETTINGS_SIZE = { width: 380, height: 570 };

// 侧边窗口的 label，以及它待在奶蛙的哪一侧。
type SideSlot = "left" | "right-top" | "right-bottom";
const SIDE_WINDOWS: Record<string, SideSlot> = {
  "naiwa-chat": "left",
  "naiwa-chart": "right-top",
  "naiwa-settings": "right-bottom",
};

// 奶蛙本体的窗口 label（tauri.conf.json 里配的）。
const PET_WINDOW_LABEL = "main";

// 工具名的中文说法，显示在工具卡片上。
const TOOL_LABELS: Record<string, string> = {
  search_stock: "搜索股票",
  get_quote: "查询行情",
  get_kline: "拉取K线",
  get_intraday: "拉取分时",
  add_watchlist: "加入自选",
  remove_watchlist: "移出自选",
  list_watchlist: "读取自选",
  mcd_nearby_stores: "找附近的店",
  mcd_my_addresses: "读你的地址",
  mcd_nutrition: "查餐品热量",
  mcd_campaign_calendar: "看活动日历",
  mcd_my_coupons: "看你的券",
  mcd_available_coupons: "看能领的券",
  mcd_bind_coupons: "领券",
  mcd_my_account: "查积分",
  mcd_lottery_info: "看抽奖",
  mcd_draw_lottery: "抽奖",
  mcd_my_prizes: "看奖品",
  mcd_points_products: "看积分商城",
  mcd_product_detail: "看商品详情",
  mcd_redeem: "积分兑换",
  mcd_order_list: "查历史订单",
  mcd_order_detail: "查订单",
  mcd_mall_order_list: "查商城订单",
  mcd_mall_order_detail: "查商城订单",
  mcd_survey_coupon: "查满意度答卷",
  mcd_delivery_stores: "查能配送的店",
  mcd_create_address: "新增地址",
  mcd_store_meals: "看菜单",
  mcd_meal_detail: "看餐品详情",
  mcd_store_coupons: "看门店券",
  mcd_calculate_price: "算价",
  mcd_create_order: "创建订单",
  mcd_cancel_order: "取消订单",
  mcd_meal_assistance: "查助餐服务",
  mcd_promotions: "查团餐优惠",
  mcd_party_city: "查活动城市",
  mcd_party_store: "查活动门店",
  mcd_party_date: "查可约日期",
  mcd_party_session: "查活动场次",
  mcd_party_order: "预约活动",
};

// 没有对应动画的动作，用台词表达。
const EXPRESS_FALLBACK: Record<string, string> = {
  shy: "突然有点不好意思嘿嘿",
  poke: "戳你一下～",
  sleep: "我先眯一会儿……",
};

// 被拖动时奶蛙的随机吐槽，每句不超过 8 个字。
const PET_LINES = [
  "不要摸我啦~~",
  "别拽我啦！",
  "轻点轻点~",
  "奶蛙亲亲你哦！",
  "不要动奶蛙哦~",
  "奶蛙很忙的",
  "再动奶蛙试试",
  "奶蛙痒痒的",
  "别碰奶蛙",
  "讨厌啦~~",
];

const EXPRESS_VERB: Record<string, string> = {
  laugh: "笑了一下",
  big_laugh: "哈哈大笑",
  shy: "害羞了",
  poke: "戳了戳你",
  sleep: "打了个瞌睡",
};

// A 股红涨绿跌，跟欧美的习惯相反。
const UP_COLOR = "#d23c31";
const DOWN_COLOR = "#1a9450";

const PERIOD_TABS = [
  { id: "intraday", label: "分时" },
  { id: "day", label: "日线" },
  { id: "week", label: "周线" },
  { id: "month", label: "月线" },
];

// 均线周期和配色，跟国内行情软件的习惯一致。
const MA_PERIODS = [5, 10, 20, 60];
const MA_COLORS: Record<number, string> = {
  5: "#ffffff",
  10: "#f0c040",
  20: "#c07ce8",
  60: "#3fbf7f",
};

/* ------------------------------------------------------------------ *
 * 全局句柄
 * ------------------------------------------------------------------ */

const searchParams = new URLSearchParams(window.location.search);
const mode = (searchParams.get("mode") as WindowMode | null) ?? "pet";

const appWindow = getCurrentWindow();
const app = document.querySelector<HTMLDivElement>("#app");

if (!app) {
  throw new Error("Missing #app root element");
}

/* ------------------------------------------------------------------ *
 * 侧边窗口：聊天是唯一出口
 * ------------------------------------------------------------------ *
 *
 *   ┌────────────┐        ┌──────┐     ┌────────────┐
 *   │   聊天窗   │        │ 奶蛙 │     │  K线图     │
 *   │ （唯一出口）│        └──────┘     └────────────┘
 *   └────────────┘
 *                                   ┌────────────┐
 *                                   │   设置     │
 *                                   └────────────┘
 *
 * 位置：聊天在奶蛙左侧（底边齐平），K线图在右上，设置在右下。
 * 三个窗口都**跟着奶蛙移动**。
 *
 * 以前还有个独立的气泡窗口（桌宠在屏幕边上贴小纸条），已经去掉——
 * 所有文字统一进聊天窗口，桌宠不再往屏幕上到处贴东西。
 */

type SideMetrics = {
  size: { width: number; height: number };
  x: number;
  y: number;
};

// 侧边窗口永远相对**奶蛙**定位。
//
// 注意：main.ts 会在每个窗口（奶蛙 / 聊天 / 图表 / 设置）里各跑一份，
// 这里的 appWindow 指的是"当前这个窗口"。从聊天窗口点行情卡片打开K线时，
// 如果拿当前窗口去算位置，K线就会贴在聊天窗旁边而不是奶蛙旁边。
const petWindow = async () => {
  const main = await WebviewWindow.getByLabel(PET_WINDOW_LABEL);
  return main ?? appWindow;
};

// 算每个侧边窗口的尺寸和位置。
//
// 缩放比例从窗口**实际**尺寸推算（物理像素 ÷ scaleFactor 才是逻辑像素）。
const sideMetrics = async (): Promise<Record<string, SideMetrics>> => {
  const pet = await petWindow();
  const position = await pet.outerPosition();
  const outerSize = await pet.outerSize();
  const scaleFactor = await pet.scaleFactor();
  const logicalHeight = outerSize.height / scaleFactor;
  const scale = outerSize.width / scaleFactor / BASE_PET_WIDTH;

  const px = (value: number) => Math.round(value * scaleFactor);
  const overlap = Math.round(SIDE_OVERLAP * scale * scaleFactor);
  const petRight = position.x + outerSize.width;

  // 聊天：高度跟奶蛙走（1.8 倍），宽度是高度的 0.78
  const chatHeight = Math.max(CHAT_MIN_SIZE.height, Math.round(logicalHeight * 1.8));
  const chatWidth = Math.max(CHAT_MIN_SIZE.width, Math.round(chatHeight * CHAT_WIDTH_RATIO));

  const place = (slot: SideSlot, width: number, height: number) => {
    if (slot === "left") {
      // 左侧：底边与奶蛙底边齐平
      return {
        x: position.x - px(width) + overlap,
        y: position.y + outerSize.height - px(height),
      };
    }
    const x = petRight - overlap;
    if (slot === "right-top") {
      return { x, y: position.y };
    }
    // 右下：底边与奶蛙底边齐平
    return { x, y: position.y + outerSize.height - px(height) };
  };

  return {
    "naiwa-chat": {
      size: { width: chatWidth, height: chatHeight },
      ...place("left", chatWidth, chatHeight),
    },
    "naiwa-chart": {
      size: CHART_SIZE,
      ...place("right-top", CHART_SIZE.width, CHART_SIZE.height),
    },
    "naiwa-settings": {
      size: SETTINGS_SIZE,
      ...place("right-bottom", SETTINGS_SIZE.width, SETTINGS_SIZE.height),
    },
  };
};

// 桌宠移动或改大小后，侧边窗口的位置和尺寸一起跟着走。
const syncSideWindows = async () => {
  const metrics = await sideMetrics();

  for (const [label, metric] of Object.entries(metrics)) {
    const existing = await WebviewWindow.getByLabel(label);
    if (!existing) {
      continue;
    }

    await existing.setSize(new LogicalSize(metric.size.width, metric.size.height));
    await existing.setPosition(new PhysicalPosition(Math.round(metric.x), Math.round(metric.y)));
  }
};

// 等窗口真正注册进 Tauri 的窗口表。
//
// WebviewWindow 的构造函数**不会等**创建完成（它只是把 create 命令丢出去），
// 所以刚 new 完就 getByLabel 是查不到的。不等一下的话，紧接着的第二次请求
// 也会认为"窗口不存在"，于是开出两个同 label 的窗口。
const waitForWindow = async (label: string) => {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    const win = await WebviewWindow.getByLabel(label);
    if (win) {
      return win;
    }
    await new Promise((resolve) => window.setTimeout(resolve, 50));
  }
  return null;
};

// 同一个 label 的并发请求复用同一个任务，避免重复开窗。
const sideWindowTasks = new Map<string, Promise<WebviewWindow>>();

// 打开（或复用）一个侧边窗口。已开着就挪到位 + 通知它更新内容。
const openSideWindow = async (
  label: string,
  url: string,
  payload: Record<string, unknown> = {},
) => {
  const running = sideWindowTasks.get(label);
  if (running) {
    const win = await running;
    await win.show();
    await win.emit("panel-show", payload);
    return win;
  }

  const task = createSideWindow(label, url);
  sideWindowTasks.set(label, task);
  try {
    return await task;
  } finally {
    sideWindowTasks.delete(label);
  }
};

const createSideWindow = async (label: string, url: string) => {
  const metrics = await sideMetrics();
  const metric = metrics[label];
  const scaleFactor = await (await petWindow()).scaleFactor();

  const existing = await WebviewWindow.getByLabel(label);
  if (existing) {
    await existing.setSize(new LogicalSize(metric.size.width, metric.size.height));
    await existing.setPosition(new PhysicalPosition(Math.round(metric.x), Math.round(metric.y)));
    await existing.show();
    return existing;
  }

  const sideWindow = new WebviewWindow(label, {
    url,
    title: APP_NAME,
    width: metric.size.width,
    height: metric.size.height,
    // 注意：新建窗口的 x/y 是**逻辑像素**，而上面的 metric 是物理像素，
    // 这里必须换算，否则窗口会跑到右下方去。
    x: Math.round(metric.x / scaleFactor),
    y: Math.round(metric.y / scaleFactor),
    resizable: label !== "naiwa-chat",
    decorations: false,
    transparent: true,
    backgroundColor: "#00000000",
    alwaysOnTop: true,
    skipTaskbar: label !== "naiwa-chat",
    shadow: false,
  });

  sideWindow.once("tauri::error", (event) => {
    console.error("Failed to create side window", event.payload);
  });

  // 等它真的进窗口表了再收工。
  await waitForWindow(label);

  return sideWindow;
};

// 奶蛙说话的唯一出口：所有文字都进聊天窗口。
//
// 之前桌宠会在屏幕边上贴独立的气泡小纸条，现在统一走聊天，
// 桌宠不再往屏幕上到处贴东西。
const petSay = async (text: string) => {
  if (!text) {
    return;
  }
  await emit("pet-say", text);
};

const openChatWindow = async () => {
  await openSideWindow("naiwa-chat", "/?mode=chat");
};

const openSettingsWindow = () =>
  openSideWindow("naiwa-settings", "/?mode=settings", {});

const openChartWindow = (code: string, name: string) =>
  openSideWindow(
    "naiwa-chart",
    `/?mode=chart&code=${encodeURIComponent(code)}&name=${encodeURIComponent(name)}`,
    { code, name },
  );

/* ------------------------------------------------------------------ *
 * 素材与工具
 * ------------------------------------------------------------------ */

const loadManifest = async (petId: string): Promise<PetManifest> => {
  const response = await fetch(`/pets/${petId}/manifest.json`);
  if (!response.ok) {
    throw new Error(`Failed to load pet manifest: ${response.status}`);
  }

  return response.json() as Promise<PetManifest>;
};

const formatFramePath = (manifest: PetManifest, index: number) => {
  const frameIndex = String(index).padStart(4, "0");
  return `/pets/${manifest.id}/${manifest.framePattern.replace("{index}", frameIndex)}`;
};

const loadImage = (src: string) =>
  new Promise<HTMLImageElement>((resolve, reject) => {
    const image = new Image();
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error(`Failed to load image: ${src}`));
    image.src = src;
  });

const preloadImages = async (manifest: PetManifest) => {
  const promises = Array.from({ length: manifest.frameCount }, (_, index) => {
    const image = new Image();
    image.decoding = "async";
    image.src = formatFramePath(manifest, index);
    return image.decode().catch(() => undefined);
  });

  await Promise.all(promises);
};

const formatNumber = (value: number, digits = 2) =>
  Number.isFinite(value) ? value.toFixed(digits) : "--";

const formatVolume = (value: number) => {
  if (!Number.isFinite(value) || value <= 0) {
    return "--";
  }
  if (value >= 1e8) {
    return `${(value / 1e8).toFixed(2)}亿手`;
  }
  if (value >= 1e4) {
    return `${(value / 1e4).toFixed(2)}万手`;
  }
  return `${value}手`;
};

// 模型偶尔不听话，冒出 markdown 或者英文。提示词管不住的，这里兜住。
// 界面上的文字永远是干净的普通话。
const cleanModelText = (text: string): string =>
  text
    .replace(/\*\*(.+?)\*\*/g, "$1")
    .replace(/^#{1,6}\s+/gm, "")
    .replace(/^[-*+]\s+/gm, "")
    .replace(/^\d+[.、]\s*/gm, "")
    .replace(/`/g, "")
    .replace(/\n{3,}/g, "\n\n");

/* ------------------------------------------------------------------ *
 * 眼球特效：左右转 + 眨眼
 * ------------------------------------------------------------------ */

type EyeRect = { x: number; y: number; w: number; h: number };

// 奶蛙身上只有眼睛是绿色的，用绿色像素自动定位，不用写死坐标。
const findEyeRect = (ctx: CanvasRenderingContext2D, width: number, height: number): EyeRect | null => {
  const { data } = ctx.getImageData(0, 0, width, height);
  let minX = width;
  let minY = height;
  let maxX = -1;
  let maxY = -1;

  for (let y = 0; y < height; y += 2) {
    for (let x = 0; x < width; x += 2) {
      const i = (y * width + x) * 4;
      const r = data[i];
      const g = data[i + 1];
      const b = data[i + 2];
      if (g > r + 5 && g > b + 40) {
        if (x < minX) minX = x;
        if (x > maxX) maxX = x;
        if (y < minY) minY = y;
        if (y > maxY) maxY = y;
      }
    }
  }

  if (maxX < 0) {
    return null;
  }

  const pad = 6;
  const x = Math.max(0, minX - pad);
  const y = Math.max(0, minY - pad);
  return {
    x,
    y,
    w: Math.min(width, maxX + pad) - x,
    h: Math.min(height, maxY + pad) - y,
  };
};

// 眼睛周围一圈皮肤的平均色，用来把原眼睛"擦掉"。
const sampleSkin = (ctx: CanvasRenderingContext2D, eye: EyeRect) => {
  const width = ctx.canvas.width;
  const height = ctx.canvas.height;
  const x0 = Math.max(0, eye.x - 24);
  const y0 = Math.max(0, eye.y - 24);
  const x1 = Math.min(width, eye.x + eye.w + 24);
  const y1 = Math.min(height, eye.y + eye.h + 24);
  const { data } = ctx.getImageData(x0, y0, x1 - x0, y1 - y0);

  let r = 0;
  let g = 0;
  let b = 0;
  let count = 0;
  for (let i = 0; i < data.length; i += 4) {
    const pr = data[i];
    const pg = data[i + 1];
    const pb = data[i + 2];
    if (pg > pb + 40 && pg > pr) {
      continue;
    }
    r += pr;
    g += pg;
    b += pb;
    count += 1;
  }

  if (count === 0) {
    return "#f5c242";
  }

  return `rgb(${Math.round(r / count)}, ${Math.round(g / count)}, ${Math.round(b / count)})`;
};

type EyeControl = { setClosed: (closed: boolean) => void };

const startEyeIdle = async (
  manifest: PetManifest,
  canvas: HTMLCanvasElement,
): Promise<EyeControl> => {
  const image = await loadImage(`/pets/${manifest.id}/${manifest.idleFrame}`);

  canvas.width = manifest.width;
  canvas.height = manifest.height;
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return { setClosed: () => {} };
  }

  ctx.drawImage(image, 0, 0);
  const eye = findEyeRect(ctx, manifest.width, manifest.height);
  if (!eye) {
    return { setClosed: () => {} };
  }

  const skin = sampleSkin(ctx, eye);
  let lastOffset = Number.NaN;
  let lastBlinking = false;
  let raf = 0;
  // 强制闭眼（睡觉用）：一直保持眯着，不参与眨眼节奏。
  let forceClosed = false;
  let forceRedraw = false;

  const draw = (now: number) => {
    const phase = (now % EYE_SWAY_PERIOD) / EYE_SWAY_PERIOD;
    const offset = Math.round(Math.sin(phase * Math.PI * 2) * EYE_SWAY_AMPLITUDE);

    // 眨眼：偶尔把眼球纵向压成一条缝。
    // 桌宠有这个小动作才显得是"活着"的，而不是一张会动的贴图。
    const blinking = forceClosed || now % BLINK_PERIOD < BLINK_DURATION;

    if (offset !== lastOffset || blinking !== lastBlinking || forceRedraw) {
      lastOffset = offset;
      lastBlinking = blinking;
      forceRedraw = false;
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      ctx.drawImage(image, 0, 0);
      ctx.fillStyle = skin;
      ctx.fillRect(eye.x, eye.y, eye.w, eye.h);

      // 眨眼时把眼睛压扁并垂直居中，看起来就是闭了一条缝。
      const eyeHeight = blinking ? Math.max(2, Math.round(eye.h * 0.22)) : eye.h;
      const eyeTop = eye.y + Math.round((eye.h - eyeHeight) / 2);
      ctx.drawImage(
        image,
        eye.x,
        eye.y,
        eye.w,
        eye.h,
        eye.x + offset,
        eyeTop,
        eye.w,
        eyeHeight,
      );
    }

    raf = window.requestAnimationFrame(draw);
  };

  raf = window.requestAnimationFrame(draw);

  return {
    /// 强制闭眼（睡觉用），false 恢复眨眼。
    setClosed: (closed: boolean) => {
      forceClosed = closed;
      forceRedraw = true;
    },
  };
};

/* ------------------------------------------------------------------ *
 * 动画播放器
 * ------------------------------------------------------------------ */

const createPetPlayer = async (
  manifest: PetManifest,
  pet: HTMLImageElement,
  eyeCanvas: HTMLCanvasElement,
  eyeControl: EyeControl,
) => {
  const audio = new Audio(`/pets/${manifest.id}/${manifest.audio}`);
  audio.preload = "auto";

  let animationFrame = 0;
  let playing = false;
  let startTime = 0;
  let startFrame = 0;
  let endFrame = manifest.frameCount;

  // 淡出时长和 CSS 里的过渡共用同一个常量
  pet.style.transitionDuration = `${FADE_MS}ms`;

  // 回到常态：最后一帧淡出，眼球动画同时淡入，交叉过渡不硬切。
  const showIdle = () => {
    pet.classList.remove("is-ready");
    eyeCanvas.style.display = "block";
  };
  const showFrame = () => {
    eyeCanvas.style.display = "none";
    pet.classList.add("is-ready");
  };

  const setIdle = () => {
    playing = false;
    window.cancelAnimationFrame(animationFrame);
    audio.pause();
    showIdle();
  };

  const render = (now: number) => {
    const elapsed = now - startTime;
    const frame = startFrame + Math.floor((elapsed / 1000) * manifest.fps);

    if (frame >= endFrame) {
      setIdle();
      return;
    }

    pet.src = formatFramePath(manifest, frame);
    animationFrame = window.requestAnimationFrame(render);
  };

  // currentTime 必须等 metadata 加载完才能设，否则浏览器会抛 InvalidStateError
  const ensureMetadata = async () => {
    if (audio.readyState >= 1) {
      return;
    }

    await new Promise<void>((resolve) => {
      audio.addEventListener("loadedmetadata", () => resolve(), { once: true });
      audio.addEventListener("error", () => resolve(), { once: true });
    });
  };

  const playSegment = async (segment?: LaughSegment) => {
    // 帧段动画要恢复睁眼：否则「睡着时触发了笑」会变成闭着眼笑。
    eyeControl.setClosed(false);

    const from = segment ? segment.startFrame : 0;
    const to = segment ? Math.min(segment.endFrame, manifest.frameCount) : manifest.frameCount;

    window.cancelAnimationFrame(animationFrame);
    await ensureMetadata();
    audio.pause();
    audio.currentTime = from / manifest.fps;

    startFrame = from;
    endFrame = to;
    playing = true;
    startTime = performance.now();
    showFrame();
    pet.src = formatFramePath(manifest, from);
    animationFrame = window.requestAnimationFrame(render);

    try {
      await audio.play();
    } catch {
      // 浏览器策略可能拦了自动播放，但动画照走。
    }
  };

  // 程序化动作：没有专门素材的动作，用待机帧 + CSS 变换演出来。
  // 这样风格天然统一，不用另外去找一套画风一致的素材。
  const PROGRAMMATIC_ACTIONS = ["shy", "poke", "sleep"];
  let actionTimer = 0;

  // 新动作开始前，把上一个动作的残留清干净。
  // 不清的话：sleep 期间触发 poke，act-sleep 的 class 还在，
  // 而 setClosed(false) 已经执行了 —— 变成闭着眼却在睡觉。
  const clearAction = () => {
    window.clearTimeout(actionTimer);
    for (const name of PROGRAMMATIC_ACTIONS) {
      pet.classList.remove(`act-${name}`);
      eyeCanvas.classList.remove(`act-${name}`);
    }
    eyeControl.setClosed(false);
  };

  const playAction = async (action: PetAction) => {
    clearAction();

    if (action === "laugh" || action === "big_laugh") {
      await playSegment(action === "big_laugh" ? LAUGH_SEGMENTS[1] : LAUGH_SEGMENTS[0]);
      return;
    }

    window.cancelAnimationFrame(animationFrame);
    audio.pause();
    playing = false;

    // 程序化动作统一走眼球层：它带眨眼，睡觉时还能强制闭眼。
    // 身体帧保持隐藏（透明），由眼球层画出完整的奶蛙。
    eyeCanvas.style.display = "block";
    eyeControl.setClosed(action === "sleep");
    pet.classList.remove("is-ready");
    pet.src = formatFramePath(manifest, 0);

    // 强制重排，保证连续触发同一个动作时动画也会重播。
    void pet.offsetWidth;
    // 身体和眼球是两个叠放的元素，必须同时变形，否则会错位。
    pet.classList.add(`act-${action}`);
    eyeCanvas.classList.add(`act-${action}`);

    const holdMs = action === "sleep" ? 6000 : 1400;
    actionTimer = window.setTimeout(() => {
      pet.classList.remove(`act-${action}`);
      eyeCanvas.classList.remove(`act-${action}`);
      eyeControl.setClosed(false);
    }, holdMs);
  };

  // 原生菜单的回调不是浏览器认可的"用户手势"，可能触发自动播放拦截。
  // 在真实的 pointerdown 里先静音播放一次，把这个 audio 元素解锁。
  const unlockAudio = async () => {
    try {
      audio.muted = true;
      await audio.play();
      audio.pause();
      audio.currentTime = 0;
    } catch {
      // 解锁失败就算了，动画不受影响
    } finally {
      audio.muted = false;
    }
  };

  audio.addEventListener("ended", () => {
    if (!playing) {
      audio.currentTime = 0;
    }
  });

  setIdle();

  return { play: playAction, playSegment, unlockAudio };
};

/* ------------------------------------------------------------------ *
 * 卡片渲染
 * ------------------------------------------------------------------ */

// 工具调用进行中的卡片。拿到结果后会被替换成行情卡片。
const createToolCard = (name: string, args: Record<string, unknown>) => {
  const card = document.createElement("div");
  card.className = "tool-card";

  const head = document.createElement("div");
  head.className = "tool-head";

  const label = document.createElement("span");
  label.className = "tool-label";
  label.textContent = TOOL_LABELS[name] ?? name;

  const status = document.createElement("span");
  status.className = "tool-status";
  status.textContent = "进行中…";

  head.append(label, status);

  const detail = document.createElement("div");
  detail.className = "tool-detail";
  detail.textContent = describeToolArgs(name, args);

  card.append(head, detail);
  return card;
};

const describeToolArgs = (name: string, args: Record<string, unknown>) => {
  if (name === "search_stock") {
    return String(args.keyword ?? "");
  }
  if (name === "get_kline") {
    return `${String(args.code ?? "")} · ${String(args.period ?? "day")}`;
  }
  if (
    name === "get_quote" ||
    name === "get_intraday" ||
    name === "add_watchlist" ||
    name === "remove_watchlist"
  ) {
    return String(args.code ?? "");
  }
  return "";
};

// 行情卡片：现价大字 + 涨跌幅 + 关键指标。
const createQuoteCard = (quote: QuotePayload) => {
  const card = document.createElement("div");
  card.className = "quote-card";

  const rising = quote.change_pct >= 0;
  const color = rising ? UP_COLOR : DOWN_COLOR;

  const header = document.createElement("div");
  header.className = "quote-header";

  const name = document.createElement("span");
  name.className = "quote-name";
  name.textContent = quote.name;

  const code = document.createElement("span");
  code.className = "quote-code";
  code.textContent = quote.code;

  header.append(name, code);

  const priceRow = document.createElement("div");
  priceRow.className = "quote-price-row";

  const price = document.createElement("span");
  price.className = "quote-price";
  price.textContent = formatNumber(quote.price);
  price.style.color = color;

  const delta = document.createElement("span");
  delta.className = "quote-delta";
  delta.textContent = `${rising ? "▲" : "▼"} ${formatNumber(Math.abs(quote.change))}  ${
    rising ? "+" : ""
  }${formatNumber(quote.change_pct)}%`;
  delta.style.color = color;

  priceRow.append(price, delta);

  const stats = document.createElement("div");
  stats.className = "quote-stats";

  const addStat = (label: string, value: string) => {
    const item = document.createElement("span");
    item.className = "quote-stat";
    item.textContent = `${label} ${value}`;
    stats.append(item);
  };

  addStat("今开", formatNumber(quote.open));
  addStat("昨收", formatNumber(quote.prev_close));
  addStat("最高", formatNumber(quote.high));
  addStat("最低", formatNumber(quote.low));
  addStat("成交量", formatVolume(quote.volume));
  if (quote.amount) {
    addStat("成交额", `${(quote.amount / 1e8).toFixed(2)}亿`);
  }
  addStat("换手", `${formatNumber(quote.turnover)}%`);
  if (quote.pe) {
    addStat("市盈", formatNumber(quote.pe));
  }
  if (quote.total_mcap) {
    addStat("总市值", `${formatNumber(quote.total_mcap)}亿`);
  }

  card.append(header, priceRow, stats);

  // 点卡片开K线图。
  card.classList.add("is-clickable");
  card.addEventListener("click", () => {
    void openChartWindow(quote.code, quote.name);
  });

  return card;
};

// 搜索命中的候选列表。
const createMatchCard = (payload: MatchPayload) => {
  const card = document.createElement("div");
  card.className = "match-card";

  payload.matches.slice(0, 8).forEach((item) => {
    const row = document.createElement("div");
    row.className = "match-row";

    const name = document.createElement("span");
    name.className = "match-name";
    name.textContent = item.name;

    const code = document.createElement("span");
    code.className = "match-code";
    code.textContent = item.code;

    row.append(name, code);
    card.append(row);
  });

  return card;
};

// K线摘要卡片。
const createKlineCard = (payload: KlineSeries) => {
  const card = document.createElement("div");
  card.className = "kline-card";

  const items = payload.items;
  if (items.length === 0) {
    return card;
  }

  const first = items[0];
  const last = items[items.length - 1];
  const changePct = first.close ? ((last.close - first.close) / first.close) * 100 : 0;
  const rising = changePct >= 0;
  const color = rising ? UP_COLOR : DOWN_COLOR;

  const periodLabel =
    payload.period === "week" ? "周线" : payload.period === "month" ? "月线" : "日线";

  const head = document.createElement("div");
  head.className = "kline-head";

  const title = document.createElement("span");
  title.className = "kline-title";
  title.textContent = `${payload.code} ${periodLabel} · ${items.length}根`;

  const change = document.createElement("span");
  change.className = "kline-change";
  change.textContent = `${rising ? "+" : ""}${formatNumber(changePct)}%`;
  change.style.color = color;

  head.append(title, change);

  const body = document.createElement("div");
  body.className = "kline-body";
  body.textContent = `${first.date} 开 ${formatNumber(first.open)} 收 ${formatNumber(first.close)}\n${last.date} 开 ${formatNumber(
    last.open,
  )} 收 ${formatNumber(last.close)}`;

  card.append(head, body);
  return card;
};

// 麦当劳工具名的中文说法，显示在卡片标题上。
const MCD_TOOL_LABELS: Record<string, string> = {
  mcd_nearby_stores: "附近的麦当劳",
  mcd_my_addresses: "我的配送地址",
  mcd_nutrition: "餐品营养",
  mcd_campaign_calendar: "活动日历",
  mcd_my_coupons: "我的优惠券",
  mcd_available_coupons: "可领优惠券",
  mcd_bind_coupons: "领券结果",
  mcd_my_account: "我的积分",
  mcd_lottery_info: "积分抽奖",
  mcd_draw_lottery: "抽奖结果",
  mcd_my_prizes: "我的奖品",
  mcd_points_products: "积分商城",
  mcd_product_detail: "商品详情",
  mcd_redeem: "兑换结果",
  mcd_order_list: "历史订单",
  mcd_order_detail: "订单详情",
  mcd_mall_order_list: "商城订单",
  mcd_mall_order_detail: "商城订单详情",
  mcd_survey_coupon: "满意度答卷",
  mcd_delivery_stores: "可配送的门店",
  mcd_create_address: "新增地址",
  mcd_store_meals: "在售菜单",
  mcd_meal_detail: "餐品详情",
  mcd_store_coupons: "门店可用券",
  mcd_calculate_price: "价格明细",
  mcd_create_order: "订单",
  mcd_cancel_order: "取消订单",
  mcd_meal_assistance: "助餐服务",
  mcd_promotions: "团餐优惠",
  mcd_party_city: "可办活动城市",
  mcd_party_store: "可办活动门店",
  mcd_party_date: "可约日期",
  mcd_party_session: "活动场次",
  mcd_party_order: "活动预约",
};

// 麦当劳接口有的返回 markdown（券、活动），把标记去掉再显示。
const stripMarkdown = (text: string) =>
  text
    .replace(/^#{1,6}\s*/gm, "")
    .replace(/\*\*/g, "")
    .replace(/^>\s?/gm, "")
    .replace(/^\|.*\|$/gm, "")
    .replace(/\n{3,}/g, "\n\n")
    .trim();

const formatDistance = (meters: number) => {
  if (!Number.isFinite(meters) || meters <= 0) {
    return "";
  }
  return meters >= 1000 ? `${(meters / 1000).toFixed(1)}km` : `${Math.round(meters)}m`;
};

// 从任意深度的数据里把「支付链接」捞出来。
//
// 不能只看是不是 http 链接——菜单里每个餐品都带一张图片 URL（menu-img.mcd.cn/xx.png），
// 全抓出来会变成一排没用的「打开支付链接」。所以按**字段名**筛：只有叫
// pay/payment/link/order/jump/href/url 的字段才算。
const collectUrls = (value: unknown, out: { key: string; url: string }[] = []) => {
  if (typeof value === "string") {
    return out;
  }
  if (Array.isArray(value)) {
    value.forEach((item) => collectUrls(item, out));
    return out;
  }
  if (value && typeof value === "object") {
    Object.entries(value as Record<string, unknown>).forEach(([key, item]) => {
      if (typeof item === "string" && /^https?:\/\//i.test(item)) {
        out.push({ key, url: item });
      } else {
        collectUrls(item, out);
      }
    });
  }
  return out;
};

const PAYMENT_KEY = /pay|payment|link|order|jump|href/i;

// 麦当劳卡片：门店 / 营养 / 地址 / 积分 / 菜单 / 算价 / 券 / 纯文本。
const createMcdCard = (payload: McdPayload) => {
  const card = document.createElement("div");
  card.className = "mcd-card";

  const title = document.createElement("div");
  title.className = "mcd-title";
  title.textContent = MCD_TOOL_LABELS[payload.tool] ?? "麦当劳";
  card.append(title);

  // 支付链接之类的，放最上面，用户一眼能点到。
  const urls = collectUrls(payload.data)
    .filter((item) => PAYMENT_KEY.test(item.key))
    .filter((item, index, all) => all.findIndex((x) => x.url === item.url) === index);
  if (urls.length > 0) {
    const links = document.createElement("div");
    links.className = "mcd-links";
    urls.slice(0, 3).forEach((item) => {
      const link = document.createElement("a");
      link.className = "mcd-link";
      link.href = item.url;
      link.target = "_blank";
      link.rel = "noreferrer";
      link.textContent = "打开支付链接";
      links.append(link);
    });
    card.append(links);
  }

  const addRow = (name: string, detail: string, extra?: string) => {
    const row = document.createElement("div");
    row.className = "mcd-row";

    const left = document.createElement("span");
    left.className = "mcd-name";
    left.textContent = name;

    const right = document.createElement("span");
    right.className = "mcd-detail";
    right.textContent = detail;

    row.append(left, right);
    if (extra) {
      const tag = document.createElement("span");
      tag.className = "mcd-tag";
      tag.textContent = extra;
      row.append(tag);
    }
    card.append(row);
  };

  const data = payload.data;
  const record = (data && typeof data === "object" && !Array.isArray(data)
    ? (data as Record<string, unknown>)
    : {}) as Record<string, unknown>;
  const list = Array.isArray(data) ? (data as Record<string, unknown>[]) : null;

  // 算价明细：商品行 + 合计。
  if (Array.isArray(record.productList)) {
    (record.productList as Record<string, unknown>[]).forEach((item) => {
      const subtotal = Number(item.subtotal ?? 0);
      const original = Number(item.originalSubtotal ?? 0);
      addRow(
        String(item.productName ?? ""),
        `×${item.quantity ?? 1}  ${subtotal} 元`,
        original > subtotal ? `原价 ${original} 元` : "",
      );
    });
    const total = Number(record.price ?? 0);
    const discount = Number(record.discount ?? 0);
    addRow("合计", `${total} 元`, discount > 0 ? `优惠 ${discount} 元` : "");
    return card;
  }

  // 菜单：分类 + 餐品。分类里只有编码，名称和价格在 data.meals 那张表里。
  if (Array.isArray(record.categories)) {
    const full = (record.meals ?? {}) as Record<string, Record<string, unknown>>;
    (record.categories as Record<string, unknown>[]).forEach((category) => {
      const head = document.createElement("div");
      head.className = "mcd-group";
      head.textContent = String(category.name ?? "");
      card.append(head);

      const meals = (category.meals as { code: string; tags?: string[] }[]) ?? [];
      meals.slice(0, 8).forEach((meal) => {
        const detail = full[meal.code];
        const name = String(detail?.name ?? meal.code);
        const price = detail?.currentPrice ? `${detail.currentPrice} 元` : "";
        const original = detail?.originalPrice ? `原价 ${detail.originalPrice}` : "";
        addRow(name, price, original);
      });
    });
    return card;
  }

  // 门店列表：接口直接返回一个数组。
  if (list && list.length > 0 && "storeName" in list[0]) {
    list.slice(0, 6).forEach((store) => {
      const name = String(store.storeName ?? "麦当劳");
      const address = String(store.address ?? "");
      const distance = formatDistance(Number(store.distance ?? 0));
      addRow(name, address, distance);
    });
    return card;
  }

  // 券列表：门店可用券。
  if (list && list.length > 0 && "title" in list[0]) {
    list.slice(0, 6).forEach((coupon) => {
      const products = (coupon.products as Record<string, unknown>[]) ?? [];
      const names = products.map((item) => String(item.productName ?? "")).join("、");
      const row = document.createElement("div");
      row.className = "mcd-row mcd-row-stack";

      const name = document.createElement("span");
      name.className = "mcd-name";
      name.textContent = String(coupon.title ?? "优惠券");

      const detail = document.createElement("span");
      detail.className = "mcd-detail";
      detail.textContent = names;

      const valid = document.createElement("span");
      valid.className = "mcd-valid";
      valid.textContent = String(coupon.tradeDateTime ?? "");

      row.append(name, detail, valid);
      card.append(row);
    });
    return card;
  }

  // 订单结果：订单号、支付链接、状态、二维码。
  if (typeof record.orderId === "string" || typeof record.orderId === "number") {
    const orderId = String(record.orderId);
    addRow("订单号", orderId);
    const status = record.orderStatusText ?? record.status ?? record.statusText;
    if (status) {
      addRow("状态", String(status));
    }
    const amount = record.payAmount ?? record.totalAmount ?? record.price;
    if (amount !== undefined) {
      addRow("应付", `${amount} 元`);
    }
    const pickup = record.pickupCode;
    if (pickup) {
      addRow("取餐码", String(pickup));
    }

    // 支付链接在电脑上打不开（那是给手机用的 H5），
    // 所以这里生成二维码贴出来，用户拿手机扫一下。
    const qrBox = document.createElement("div");
    qrBox.className = "mcd-qr";

    const qrHint = document.createElement("div");
    qrHint.className = "mcd-qr-hint";
    qrHint.textContent = "手机扫这个码付款";
    qrBox.append(qrHint);

    const qrImage = document.createElement("div");
    qrImage.className = "mcd-qr-image";
    qrBox.append(qrImage);
    card.append(qrBox);

    void invoke<string>("mcd_qr_svg", { orderId })
      .then((svg) => {
        qrImage.innerHTML = svg;
      })
      .catch(() => {
        qrHint.textContent = "二维码生成失败，用订单号在麦当劳 App 里查也行";
      });

    return card;
  }

  // 营养成分：Rust 侧转成了 { count, items }。
  if (Array.isArray(record.items)) {
    const items = record.items as Record<string, unknown>[];
    const count = Number(record.count ?? items.length);
    addRow(`共 ${count} 个餐品`, count > items.length ? `显示前 ${items.length} 个` : "");
    items.slice(0, 12).forEach((item) => {
      const kcal = Number(item.kcal ?? 0);
      const protein = Number(item.protein_g ?? 0);
      const fat = Number(item.fat_g ?? 0);
      const sodium = Number(item.sodium_mg ?? 0);
      addRow(
        String(item.name ?? ""),
        `${kcal} 大卡 · 蛋白 ${protein}g · 脂肪 ${fat}g · 钠 ${sodium}mg`,
      );
    });
    return card;
  }

  // 配送地址。
  if (Array.isArray(record.addresses)) {
    (record.addresses as Record<string, unknown>[]).slice(0, 5).forEach((item) => {
      addRow(
        String(item.contactName ?? ""),
        String(item.fullAddress ?? ""),
        String(item.phone ?? ""),
      );
    });
    return card;
  }

  // 积分账户。
  if (typeof record.availablePoint === "string") {
    addRow("可用积分", `${record.availablePoint} 分`);
    addRow("累计积分", `${record.accumulativePoint ?? "0"} 分`);
    if (Number(record.expiredPoint ?? 0) > 0) {
      addRow("已过期", `${record.expiredPoint} 分`);
    }
    return card;
  }

  // 券列表、活动日历这类是纯 markdown。
  if (payload.text) {
    const body = document.createElement("div");
    body.className = "mcd-text";
    body.textContent = stripMarkdown(payload.text).slice(0, 1200);
    card.append(body);
    return card;
  }

  // 兜底：把没认出来的字段按「键: 值」列出来，别让用户对着空白卡片发呆。
  const fallback = Object.entries(record)
    .filter(([key, value]) => key !== "unit" && typeof value !== "object")
    .slice(0, 6);
  fallback.forEach(([key, value]) => {
    addRow(key, String(value ?? ""));
  });

  return card;
};

/* ------------------------------------------------------------------ *
 * 图表
 * ------------------------------------------------------------------ */

// 移动均线。数据不够周期时返回 null。
const calcMA = (items: KlinePoint[], period: number): (number | null)[] => {
  const result: (number | null)[] = [];
  let sum = 0;
  for (let index = 0; index < items.length; index += 1) {
    sum += items[index].close;
    if (index >= period) {
      sum -= items[index - period].close;
    }
    result.push(index >= period - 1 ? sum / period : null);
  }
  return result;
};

// 画蜡烛图 + 成交量副图 + 均线叠加。A 股红涨绿跌。
const drawCandles = (
  canvas: HTMLCanvasElement,
  items: KlinePoint[],
  hoverIndex: number,
  maPeriods: number[],
) => {
  const dpr = window.devicePixelRatio || 1;
  const cssWidth = canvas.clientWidth;
  const cssHeight = canvas.clientHeight;
  if (cssWidth <= 0 || cssHeight <= 0 || items.length === 0) {
    return;
  }

  canvas.width = Math.round(cssWidth * dpr);
  canvas.height = Math.round(cssHeight * dpr);
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, cssWidth, cssHeight);

  const padLeft = 8;
  const padRight = 52;
  const padTop = 8;
  const padBottom = 20;
  const gap = 10;

  const plotWidth = cssWidth - padLeft - padRight;
  const priceHeight = cssHeight * 0.72 - padTop - padBottom - gap;
  const priceTop = padTop;
  const priceBottom = priceTop + priceHeight;
  const volumeTop = priceBottom + gap;
  const volumeBottom = cssHeight - padBottom;

  const maxPrice = Math.max(...items.map((item) => item.high));
  const minPrice = Math.min(...items.map((item) => item.low));
  const rawSpan = maxPrice - minPrice || 1;
  const top = maxPrice + rawSpan * 0.05;
  const bottom = minPrice - rawSpan * 0.05;
  const span = top - bottom;

  const toY = (price: number) => priceTop + ((top - price) / span) * priceHeight;
  const slot = plotWidth / items.length;
  const bodyWidth = Math.max(1.5, Math.min(14, slot * 0.62));

  // 价格网格
  ctx.font = "10px system-ui, sans-serif";
  ctx.textBaseline = "middle";
  for (let i = 0; i <= 4; i += 1) {
    const price = top - (span * i) / 4;
    const y = toY(price);
    ctx.strokeStyle = "rgba(74, 58, 24, 0.08)";
    ctx.beginPath();
    ctx.moveTo(padLeft, y);
    ctx.lineTo(padLeft + plotWidth, y);
    ctx.stroke();

    ctx.fillStyle = "#a89b7c";
    ctx.textAlign = "left";
    ctx.fillText(price.toFixed(2), padLeft + plotWidth + 4, y);
  }

  // 蜡烛
  items.forEach((item, index) => {
    const x = padLeft + slot * index + slot / 2;
    const rising = item.close >= item.open;
    ctx.strokeStyle = rising ? UP_COLOR : DOWN_COLOR;
    ctx.fillStyle = rising ? UP_COLOR : DOWN_COLOR;

    ctx.beginPath();
    ctx.moveTo(x, toY(item.high));
    ctx.lineTo(x, toY(item.low));
    ctx.stroke();

    const yOpen = toY(item.open);
    const yClose = toY(item.close);
    const bodyTop = Math.min(yOpen, yClose);
    const bodyHeight = Math.max(1, Math.abs(yClose - yOpen));
    ctx.fillRect(x - bodyWidth / 2, bodyTop, bodyWidth, bodyHeight);
  });

  // 均线叠加
  maPeriods.forEach((period) => {
    const values = calcMA(items, period);
    ctx.strokeStyle = MA_COLORS[period];
    ctx.lineWidth = 1;
    ctx.beginPath();
    let started = false;
    values.forEach((value, index) => {
      if (value === null) {
        return;
      }
      const x = padLeft + slot * index + slot / 2;
      const y = toY(value);
      if (!started) {
        ctx.moveTo(x, y);
        started = true;
      } else {
        ctx.lineTo(x, y);
      }
    });
    ctx.stroke();
  });

  // 成交量副图
  const maxVolume = Math.max(...items.map((item) => item.volume)) || 1;
  items.forEach((item, index) => {
    const x = padLeft + slot * index + slot / 2;
    const rising = item.close >= item.open;
    ctx.fillStyle = rising ? "rgba(210, 60, 49, 0.5)" : "rgba(26, 148, 80, 0.5)";
    const height = (item.volume / maxVolume) * (volumeBottom - volumeTop);
    ctx.fillRect(x - bodyWidth / 2, volumeBottom - height, bodyWidth, height);
  });

  // 日期标签
  ctx.fillStyle = "#a89b7c";
  ctx.textAlign = "center";
  [0, Math.floor(items.length / 2), items.length - 1].forEach((index) => {
    const x = padLeft + slot * index + slot / 2;
    ctx.fillText(items[index].date.slice(5), x, cssHeight - padBottom / 2);
  });

  // 悬停十字线
  if (hoverIndex >= 0 && hoverIndex < items.length) {
    const x = padLeft + slot * hoverIndex + slot / 2;
    ctx.strokeStyle = "rgba(74, 58, 24, 0.35)";
    ctx.setLineDash([3, 3]);
    ctx.beginPath();
    ctx.moveTo(x, priceTop);
    ctx.lineTo(x, volumeBottom);
    ctx.stroke();
    ctx.setLineDash([]);
  }
};

// 分时的 X 轴槽位：09:30-11:30 占 0-120，13:00-15:00 占 121-241。
const INTRADAY_SLOTS = 241;

const timeToSlot = (time: string): number => {
  const hours = Number(time.slice(0, 2));
  const minutes = Number(time.slice(2, 4));
  const total = hours * 60 + minutes;
  const morningStart = 9 * 60 + 30;
  const noon = 11 * 60 + 30;
  const afternoonStart = 13 * 60;
  if (total <= noon) {
    return total - morningStart;
  }
  if (total >= afternoonStart) {
    return 120 + (total - afternoonStart);
  }
  return 120;
};

// 均价 = 累计成交额 / (累计成交量手数 × 100)
const avgPrice = (item: IntradayPoint): number =>
  item.volume > 0 ? item.amount / (item.volume * 100) : 0;

// 画分时图：价格线 + 均价线 + 昨收基准线，涨红跌绿填充，右轴同步涨跌幅。
const drawIntraday = (
  canvas: HTMLCanvasElement,
  payload: IntradaySeries,
  hoverIndex: number,
) => {
  const dpr = window.devicePixelRatio || 1;
  const cssWidth = canvas.clientWidth;
  const cssHeight = canvas.clientHeight;
  if (cssWidth <= 0 || cssHeight <= 0 || payload.items.length === 0) {
    return;
  }

  canvas.width = Math.round(cssWidth * dpr);
  canvas.height = Math.round(cssHeight * dpr);
  const ctx = canvas.getContext("2d");
  if (!ctx) {
    return;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, cssWidth, cssHeight);

  const padLeft = 44;
  const padRight = 52;
  const padTop = 8;
  const padBottom = 20;
  const gap = 10;

  const plotWidth = cssWidth - padLeft - padRight;
  const priceHeight = cssHeight * 0.74 - padTop - padBottom - gap;
  const priceTop = padTop;
  const priceBottom = priceTop + priceHeight;
  const volumeTop = priceBottom + gap;
  const volumeBottom = cssHeight - padBottom;

  const prevClose = payload.prev_close || payload.items[0].price;
  const prices = payload.items.map((item) => item.price);
  const maxPrice = Math.max(prevClose, ...prices);
  const minPrice = Math.min(prevClose, ...prices);
  const rawSpan = maxPrice - minPrice || Math.max(prevClose * 0.01, 0.01);
  const top = maxPrice + rawSpan * 0.12;
  const bottom = minPrice - rawSpan * 0.12;
  const span = top - bottom;

  const toY = (price: number) => priceTop + ((top - price) / span) * priceHeight;
  const toX = (slot: number) => padLeft + (slot / INTRADAY_SLOTS) * plotWidth;
  const pctOf = (price: number) => ((price - prevClose) / prevClose) * 100;

  // 网格 + 左右双轴
  ctx.font = "10px system-ui, sans-serif";
  ctx.textBaseline = "middle";
  for (let i = 0; i <= 4; i += 1) {
    const price = top - (span * i) / 4;
    const y = toY(price);
    ctx.strokeStyle = "rgba(74, 58, 24, 0.08)";
    ctx.beginPath();
    ctx.moveTo(padLeft, y);
    ctx.lineTo(padLeft + plotWidth, y);
    ctx.stroke();

    ctx.fillStyle = "#a89b7c";
    ctx.textAlign = "right";
    ctx.fillText(price.toFixed(2), padLeft - 6, y);

    const pct = pctOf(price);
    ctx.fillStyle = pct >= 0 ? UP_COLOR : DOWN_COLOR;
    ctx.fillText(`${pct >= 0 ? "+" : ""}${pct.toFixed(2)}%`, cssWidth - 4, y);
  }

  const yBase = toY(prevClose);

  // 昨收基准线
  ctx.strokeStyle = "rgba(120, 110, 90, 0.55)";
  ctx.setLineDash([4, 3]);
  ctx.beginPath();
  ctx.moveTo(padLeft, yBase);
  ctx.lineTo(padLeft + plotWidth, yBase);
  ctx.stroke();
  ctx.setLineDash([]);

  // 涨红跌绿填充
  for (let i = 1; i < payload.items.length; i += 1) {
    const previous = payload.items[i - 1];
    const current = payload.items[i];
    const x0 = toX(timeToSlot(previous.time));
    const x1 = toX(timeToSlot(current.time));
    const y0 = toY(previous.price);
    const y1 = toY(current.price);

    if (previous.price >= prevClose && current.price >= prevClose) {
      ctx.fillStyle = "rgba(210, 60, 49, 0.16)";
      ctx.fillRect(x0, Math.min(y0, y1), Math.max(1, x1 - x0), Math.abs(y1 - y0));
    } else if (previous.price < prevClose && current.price < prevClose) {
      ctx.fillStyle = "rgba(26, 148, 80, 0.16)";
      ctx.fillRect(x0, Math.min(y0, y1), Math.max(1, x1 - x0), Math.abs(y1 - y0));
    } else {
      // 跨越昨收线，拆成红绿两段
      const denominator = current.price - previous.price;
      const ratio = Math.abs(denominator) < 1e-9 ? 0.5 : (prevClose - previous.price) / denominator;
      const crossX = x0 + (x1 - x0) * ratio;
      ctx.fillStyle = "rgba(210, 60, 49, 0.16)";
      ctx.beginPath();
      ctx.moveTo(x0, y0);
      ctx.lineTo(crossX, yBase);
      ctx.lineTo(x1, Math.min(y1, yBase));
      ctx.closePath();
      ctx.fill();
      ctx.fillStyle = "rgba(26, 148, 80, 0.16)";
      ctx.beginPath();
      ctx.moveTo(x0, Math.max(y0, yBase));
      ctx.lineTo(crossX, yBase);
      ctx.lineTo(x1, y1);
      ctx.closePath();
      ctx.fill();
    }
  }

  // 均价线
  ctx.strokeStyle = "#f0c040";
  ctx.lineWidth = 1.2;
  ctx.beginPath();
  payload.items.forEach((item, index) => {
    const x = toX(timeToSlot(item.time));
    const y = toY(avgPrice(item));
    if (index === 0) {
      ctx.moveTo(x, y);
    } else {
      ctx.lineTo(x, y);
    }
  });
  ctx.stroke();

  // 价格线
  ctx.strokeStyle = "#2f2a1c";
  ctx.lineWidth = 1.1;
  ctx.beginPath();
  payload.items.forEach((item, index) => {
    const x = toX(timeToSlot(item.time));
    const y = toY(item.price);
    if (index === 0) {
      ctx.moveTo(x, y);
    } else {
      ctx.lineTo(x, y);
    }
  });
  ctx.stroke();

  // 成交量副图：每分钟增量
  let maxVolume = 0;
  const deltas: number[] = [];
  payload.items.forEach((item, index) => {
    const previous = index > 0 ? payload.items[index - 1].volume : 0;
    const delta = Math.max(0, item.volume - previous);
    deltas.push(delta);
    if (delta > maxVolume) {
      maxVolume = delta;
    }
  });

  if (maxVolume > 0) {
    payload.items.forEach((item, index) => {
      const x = toX(timeToSlot(item.time));
      const height = (deltas[index] / maxVolume) * (volumeBottom - volumeTop);
      ctx.fillStyle =
        item.price >= prevClose ? "rgba(210, 60, 49, 0.5)" : "rgba(26, 148, 80, 0.5)";
      ctx.fillRect(x, volumeBottom - height, Math.max(1, plotWidth / INTRADAY_SLOTS), height);
    });
  }

  // 时间轴
  ctx.fillStyle = "#a89b7c";
  ctx.textAlign = "center";
  [
    [0, "09:30"],
    [60, "10:30"],
    [120, "11:30/13:00"],
    [180, "14:00"],
    [240, "15:00"],
  ].forEach(([slot, label]) => {
    ctx.fillText(label as string, toX(slot as number), cssHeight - padBottom / 2);
  });

  // 悬停十字线
  if (hoverIndex >= 0 && hoverIndex < payload.items.length) {
    const item = payload.items[hoverIndex];
    const x = toX(timeToSlot(item.time));
    ctx.strokeStyle = "rgba(74, 58, 24, 0.35)";
    ctx.setLineDash([3, 3]);
    ctx.beginPath();
    ctx.moveTo(x, priceTop);
    ctx.lineTo(x, volumeBottom);
    ctx.stroke();
    ctx.setLineDash([]);
  }
};

/* ------------------------------------------------------------------ *
 * 聊天窗口
 * ------------------------------------------------------------------ */

const bootChat = async () => {
  const panel = document.createElement("div");
  panel.className = "chat-panel";

  const closeButton = document.createElement("button");
  closeButton.className = "chat-close";
  closeButton.type = "button";
  closeButton.textContent = "×";
  closeButton.title = "收起";
  closeButton.addEventListener("click", () => {
    void appWindow.hide();
  });

  const log = document.createElement("div");
  log.className = "chat-log";

  const form = document.createElement("form");
  form.className = "chat-form";

  const input = document.createElement("input");
  input.className = "chat-input";
  input.type = "text";
  input.placeholder = "问问奶蛙某只股票…";
  input.maxLength = 200;
  input.autocomplete = "off";

  const sendButton = document.createElement("button");
  sendButton.className = "chat-send";
  sendButton.type = "submit";
  sendButton.textContent = "发送";

  form.append(input, sendButton);
  panel.append(log, form, closeButton);
  app.append(panel);

  let sending = false;

  const appendBubble = (role: string, content: string) => {
    const row = document.createElement("div");
    row.className = `chat-row ${role}`;
    const bubble = document.createElement("div");
    bubble.className = "chat-bubble";
    // 只有奶蛙说的话需要清洗，用户自己打的字原样显示。
    bubble.textContent = role === "assistant" ? cleanModelText(content) : content;
    row.append(bubble);
    log.append(row);
    log.scrollTop = log.scrollHeight;
  };

  const scrollToBottom = () => {
    log.scrollTop = log.scrollHeight;
  };

  const setBusy = (busy: boolean) => {
    sending = busy;
    sendButton.disabled = busy;
    sendButton.textContent = busy ? "…" : "发送";
  };

  const send = async () => {
    const text = input.value.trim();
    if (!text || sending) {
      return;
    }

    input.value = "";
    appendBubble("user", text);

    // 模型回复的气泡，边收边填。
    const replyRow = document.createElement("div");
    replyRow.className = "chat-row assistant";
    const replyBubble = document.createElement("div");
    replyBubble.className = "chat-bubble";
    replyBubble.textContent = "…";
    replyRow.append(replyBubble);
    log.append(replyRow);
    scrollToBottom();
    let rawReply = "";

    // 正在调用的工具卡片，等结果回来再决定渲染成什么。
    let toolCard: HTMLElement | null = null;

    const channel = new Channel<AgentEvent>();
    channel.onmessage = (event) => {
      switch (event.type) {
        case "assistant": {
          rawReply += event.content;
          replyBubble.textContent = cleanModelText(rawReply);
          scrollToBottom();
          break;
        }
        case "tool_call": {
          toolCard = createToolCard(event.name, event.args ?? {});
          log.append(toolCard);
          scrollToBottom();
          break;
        }
        case "tool_result": {
          const parsed = tryParseJson(event.result);
          let card: HTMLElement | null = null;

          if (isQuotePayload(parsed)) {
            card = createQuoteCard(parsed);
          } else if (isMcdPayload(parsed)) {
            // 必须排在 K线判断前面：营养成分的载荷里也有 items 数组。
            card = createMcdCard(parsed);
          } else if (parsed && Array.isArray((parsed as MatchPayload).matches)) {
            card = createMatchCard(parsed as MatchPayload);
          } else if (parsed && Array.isArray((parsed as KlineSeries).items)) {
            card = createKlineCard(parsed as KlineSeries);
          }

          if (card && toolCard) {
            toolCard.replaceWith(card);
          } else if (toolCard) {
            const status = toolCard.querySelector(".tool-status");
            if (status) {
              status.textContent = "完成";
            }
            const detail = toolCard.querySelector(".tool-detail");
            if (detail && !detail.textContent) {
              detail.textContent = event.result.slice(0, 120);
            }
          }
          toolCard = null;
          scrollToBottom();
          break;
        }
        case "tool_error": {
          if (toolCard) {
            const status = toolCard.querySelector(".tool-status");
            if (status) {
              status.textContent = "失败";
              status.classList.add("is-error");
            }
            const detail = toolCard.querySelector(".tool-detail");
            if (detail) {
              detail.textContent = event.error;
            }
          }
          toolCard = null;
          scrollToBottom();
          break;
        }
        case "express": {
          // 动画由 Rust 侧统一广播给桌宠窗口，这里只留一句记录。
          appendBubble("assistant", `（奶蛙${EXPRESS_VERB[event.action] ?? "动了动"}）`);
          break;
        }
        case "done": {
          break;
        }
        case "error": {
          appendBubble("error", event.message);
          break;
        }
      }
    };

    setBusy(true);
    try {
      await invoke("agent_chat", { message: text, channel });
    } catch (error) {
      appendBubble("error", error instanceof Error ? error.message : "奶蛙走神了，再试一次");
    } finally {
      setBusy(false);
      input.focus();
    }
  };

  form.addEventListener("submit", (event) => {
    event.preventDefault();
    void send();
  });

  await appWindow.listen("panel-show", () => {
    log.scrollTop = log.scrollHeight;
    input.focus();
  });

  // 奶蛙主动说的话（拖拽吐槽、涨跌提醒、主动冒泡都走这个事件）。
  await appWindow.listen("pet-say", (event) => {
    const text = String(event.payload ?? "");
    if (text) {
      appendBubble("assistant", text);
    }
  });

  // 先读配置：问候语和提示都要跟着配置走。
  let view: ConfigView | null = null;
  try {
    view = await invoke<ConfigView>("get_config");
  } catch {
    // 读不到配置就先不提示，不影响正常聊天。
  }

  // 没配麦当劳 Token 就别提麦当劳，不然奶蛙会满口承诺自己查不了的东西。
  appendBubble(
    "assistant",
    view?.mcd_token_set
      ? "嗨！我是奶蛙。问我股票行情，或者附近哪家麦当劳～"
      : "嗨！我是奶蛙。问我股票行情吧～",
  );

  if (view?.first_run) {
    // 第一次启动：一张卡片把 API Key 和麦当劳 Token 一起收了，
    // 填完就能聊，之后再也不打扰。
    const row = document.createElement("div");
    row.className = "chat-row assistant";
    row.append(createMcdSetupCard({ welcome: true }));
    log.append(row);
  } else {
    // 还没配 API Key 就给个入口，不然用户对着空气说话。
    if (view && !view.has_key) {
      const row = document.createElement("div");
      row.className = "chat-row assistant";
      const bubble = document.createElement("div");
      bubble.className = "chat-bubble chat-bubble-action";
      bubble.textContent = "还没配 API Key，点我去设置 →";
      bubble.addEventListener("click", () => {
        void openSettingsWindow();
      });
      row.append(bubble);
      log.append(row);
    }

    // 没开通麦当劳就把开通卡片放在这儿：申请页 + 粘 Token，全在聊天里完成。
    if (view && !view.mcd_token_set) {
      const row = document.createElement("div");
      row.className = "chat-row assistant";
      row.append(createMcdSetupCard());
      log.append(row);
    }
  }

  input.focus();
};

const tryParseJson = (text: string): unknown => {
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return null;
  }
};

const isQuotePayload = (value: unknown): value is QuotePayload => {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return typeof record.price === "number" && typeof record.change_pct === "number";
};

const isMcdPayload = (value: unknown): value is McdPayload => {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  return (value as Record<string, unknown>).source === "mcd";
};

// 麦当劳 MCP Token 的申请页。
const MCD_TOKEN_PAGE = "https://open.mcd.cn/mcp";
// 内嵌用的页面：**首页**，不是登录页。
//
// 登录页有两个，千万别用错：
// - `/mcp/auth/login` 是给第三方应用做 OAuth 授权用的，登录完会跳 `/mcp/auth`
//   授权页，而那个页面要 `client_id` 查询参数 → 直接报「缺少clientId参数」。
// - `/mcp/login` 才是普通入口：没登录显示登录表单，已登录跳回首页。
//
// 所以统一用首页：没登录就点右上角「登录」，已登录就点「控制台」，
// 两条路都在同一个页面里，永远不会碰到 OAuth 的 clientId。
const MCD_LOGIN_PAGE = "https://open.mcd.cn/mcp";

// 「开通麦当劳」卡片。
//
// 申请 Token 要手机号 + 短信验证码，官方页面没有 iframe 限制（实测所有路由
// 都没有 X-Frame-Options / CSP），所以直接把登录页嵌进聊天窗里，
// 不用切到浏览器。登录页默认折叠，点开才加载，免得每次开聊天都去请求人家。
// Token 仍然要用户自己复制回来（跨域读不到 iframe 里的内容）。
const createMcdSetupCard = (options: { welcome?: boolean } = {}) => {
  const welcome = options.welcome === true;
  const card = document.createElement("div");
  card.className = "mcd-setup";

  const title = document.createElement("div");
  title.className = "mcd-setup-title";
  title.textContent = welcome ? "欢迎使用奶蛙" : "开通麦当劳";
  card.append(title);

  const desc = document.createElement("div");
  desc.className = "mcd-setup-desc";
  desc.textContent = welcome
    ? "填好下面两样就能开始聊天了。API Key 用来调模型，麦当劳 Token 想查麦当劳才需要。"
    : "奶蛙还没有麦当劳的接口凭证，两步开通：";
  card.append(desc);

  // 首次启动：顺带把 API Key 也收了，省得再去设置窗口翻一遍。
  let apiInput: HTMLInputElement | null = null;
  if (welcome) {
    const apiLabel = document.createElement("div");
    apiLabel.className = "mcd-setup-step";
    apiLabel.textContent = "① API Key";

    apiInput = document.createElement("input");
    apiInput.className = "mcd-setup-input";
    apiInput.type = "password";
    apiInput.placeholder = "sk-…（MiniMax / DeepSeek 都行）";

    card.append(apiLabel, apiInput);
  }

  // ① 申请 Token
  const step1 = document.createElement("div");
  step1.className = "mcd-setup-step";
  step1.append(document.createTextNode(welcome ? "② 麦当劳 MCP Token（可选）：" : "① "));

  const link = document.createElement("a");
  link.className = "mcd-setup-link";
  link.href = MCD_TOKEN_PAGE;
  link.textContent = "open.mcd.cn/mcp";
  link.title = "用系统浏览器打开";
  link.addEventListener("click", (event) => {
    event.preventDefault();
    void invoke("plugin:opener|open_url", { url: MCD_TOKEN_PAGE, with: null }).catch(() => {
      // 插件不可用就退回应用内打开，总比点了没反应强。
      window.open(MCD_TOKEN_PAGE, "_blank");
    });
  });
  step1.append(link, document.createTextNode(" 申请 Token（扫码登录）"));
  card.append(step1);

  // 粘贴 Token
  const step2 = document.createElement("div");
  step2.className = "mcd-setup-step";
  step2.textContent = welcome ? "   把申请到的 Token 粘到下面：" : "② 把申请到的 Token 粘到下面：";
  card.append(step2);

  const row = document.createElement("div");
  row.className = "mcd-setup-row";

  const input = document.createElement("input");
  input.className = "mcd-setup-input";
  input.type = "text";
  input.placeholder = "粘贴 MCP Token";

  const save = document.createElement("button");
  save.className = "mcd-setup-save";
  save.type = "button";
  save.textContent = "保存";

  const hint = document.createElement("div");
  hint.className = "mcd-setup-hint";

  save.addEventListener("click", () => {
    void (async () => {
      const token = input.value.trim();
      const apiKey = apiInput ? apiInput.value.trim() : "";

      if (welcome && !apiKey) {
        hint.textContent = "先填一下 API Key，不然奶蛙没法说话";
        hint.classList.add("is-error");
        return;
      }
      if (!welcome && !token) {
        hint.textContent = "先把 Token 粘进来";
        hint.classList.add("is-error");
        return;
      }

      save.disabled = true;
      save.textContent = "…";
      try {
        const result = await invoke<ConfigView>("save_config", {
          apiKey,
          // Base URL 和模型留空 = 不修改
          baseUrl: "",
          model: "",
          // 空 = 不修改（首次启动时本来就没配）
          mcdMcpToken: token || null,
        });
        const ok = welcome ? result.has_key : result.mcd_token_set;
        if (ok) {
          card.replaceWith(createMcdSetupDone(welcome));
        } else {
          hint.textContent = token
            ? "Token 好像没保存上，再试一次"
            : "Key 好像没保存上，再试一次";
          hint.classList.add("is-error");
          save.disabled = false;
          save.textContent = "保存";
        }
      } catch (error) {
        hint.textContent = error instanceof Error ? error.message : "保存失败";
        hint.classList.add("is-error");
        save.disabled = false;
        save.textContent = "保存";
      }
    })();
  });

  row.append(input, save);
  card.append(row, hint);

  // ---- 内嵌登录页（默认折叠）----
  const toggle = document.createElement("button");
  toggle.className = "mcd-setup-toggle";
  toggle.type = "button";
  toggle.textContent = "在这里直接登录（不用切浏览器）";
  card.append(toggle);

  const holder = document.createElement("div");
  holder.className = "mcd-setup-frame";
  holder.style.display = "none";
  card.append(holder);

  toggle.addEventListener("click", () => {
    if (holder.style.display === "none") {
      if (!holder.firstChild) {
        const frame = document.createElement("iframe");
        frame.className = "mcd-setup-iframe";
        // 必须显式授权剪贴板写入。
        // 跨域 iframe 默认没有 `clipboard-write` 权限（Permissions Policy），
        // 官方页面点「复制」调 navigator.clipboard.writeText 会直接 reject，
        // 页面上就弹「复制失败」。
        //
        // 实测（可信鼠标事件，非合成 click）：
        //   无 allow 属性 → 权限 denied → NotAllowedError
        //   有 allow 属性 → 权限 granted → 写入成功
        //
        // 顺序有讲究：权限是 iframe 插入文档那一刻继承的，所以 allow 必须
        // 排在 src 前面。
        frame.setAttribute("allow", "clipboard-write; clipboard-read");
        frame.src = MCD_LOGIN_PAGE;
        frame.title = "麦当劳 MCP 平台登录";
        frame.setAttribute("scrolling", "yes");
        // 万一以后官方把 token 放进跳转 URL 里，这里能顺手捡回来。
        frame.addEventListener("load", () => {
          const match = /[?&](?:apiKey|token|mcpToken)=([^&]+)/i.exec(frame.src);
          if (match && !input.value.trim()) {
            input.value = decodeURIComponent(match[1]);
            hint.textContent = "从登录页取到 Token 了，点保存就行";
          }
        });
        holder.append(frame);

        const howto = document.createElement("div");
        howto.className = "mcd-setup-howto";
        howto.textContent =
          "点右上角菜单 →「登录」，用手机扫二维码登录；登录后点「控制台」→「激活」申请 Token，" +
          "复制后粘回上面那个框 → 点保存。" +
          "复制按钮没反应的话，点 Token 输入框 Ctrl+A 全选、Ctrl+C 手动复制一样能用。";
        holder.append(howto);
      }
      holder.style.display = "block";
      toggle.textContent = "收起登录页";
      // 展开后把登录页滚进视野，不然用户以为没反应。
      window.setTimeout(() => {
        holder.scrollIntoView({ block: "nearest", behavior: "smooth" });
      }, 60);
    } else {
      holder.style.display = "none";
      toggle.textContent = "在这里直接登录（不用切浏览器）";
    }
  });

  return card;
};

// 开通成功后的提示。
const createMcdSetupDone = (welcome = false) => {
  const bubble = document.createElement("div");
  bubble.className = "chat-bubble chat-bubble-action";
  bubble.textContent = welcome
    ? "好啦，奶蛙醒过来了～问我股票行情，或者附近哪家麦当劳"
    : "好啦，麦当劳开通了！现在可以问「附近有麦当劳吗」「巨无霸多少大卡」了～";
  return bubble;
};

/* ------------------------------------------------------------------ *
 * 图表窗口
 * ------------------------------------------------------------------ */

const bootChart = async () => {
  let code = searchParams.get("code") ?? "";
  let name = searchParams.get("name") ?? "";
  let period = "day";
  let items: KlinePoint[] = [];
  let intraday: IntradaySeries | null = null;
  let maPeriods: number[] = [MA_PERIODS[0]];
  let hoverIndex = -1;

  const wrap = document.createElement("div");
  wrap.className = "chart-wrap";

  const header = document.createElement("div");
  header.className = "chart-header";

  const titleBox = document.createElement("div");
  titleBox.className = "chart-title";
  const titleName = document.createElement("span");
  titleName.className = "chart-name";
  const titleCode = document.createElement("span");
  titleCode.className = "chart-code";
  titleBox.append(titleName, titleCode);

  const priceBox = document.createElement("div");
  priceBox.className = "chart-price-box";
  const priceValue = document.createElement("span");
  priceValue.className = "chart-price";
  const priceDelta = document.createElement("span");
  priceDelta.className = "chart-delta";
  priceBox.append(priceValue, priceDelta);

  header.append(titleBox, priceBox);

  const closeButton = document.createElement("button");
  closeButton.className = "chart-close";
  closeButton.type = "button";
  closeButton.textContent = "×";
  closeButton.title = "收起";
  closeButton.addEventListener("click", () => {
    void appWindow.hide();
  });

  const tabs = document.createElement("div");
  tabs.className = "chart-tabs";
  const tabButtons = PERIOD_TABS.map((tab) => {
    const button = document.createElement("button");
    button.className = "chart-tab";
    button.type = "button";
    button.textContent = tab.label;
    button.addEventListener("click", () => {
      if (period === tab.id) {
        return;
      }
      period = tab.id;
      tabButtons.forEach((item) => item.button.classList.toggle("is-active", item.id === tab.id));
      maRow.style.display = period === "intraday" ? "none" : "flex";
      hoverIndex = -1;
      hint.textContent = "";
      void load();
    });
    tabs.append(button);
    return { id: tab.id, button };
  });
  tabButtons[1].button.classList.add("is-active");

  // 均线开关
  const maRow = document.createElement("div");
  maRow.className = "chart-ma-row";
  MA_PERIODS.forEach((value) => {
    const button = document.createElement("button");
    button.className = "chart-ma";
    button.type = "button";
    button.textContent = `MA${value}`;
    button.classList.toggle("is-active", maPeriods.includes(value));
    button.addEventListener("click", () => {
      if (maPeriods.includes(value)) {
        maPeriods = maPeriods.filter((item) => item !== value);
      } else {
        maPeriods = [...maPeriods, value].sort((a, b) => a - b);
      }
      button.classList.toggle("is-active", maPeriods.includes(value));
      draw();
    });
    maRow.append(button);
  });

  const canvas = document.createElement("canvas");
  canvas.className = "chart-canvas";

  const hint = document.createElement("div");
  hint.className = "chart-hint";

  wrap.append(header, tabs, maRow, canvas, hint, closeButton);
  app.append(wrap);

  const draw = () => {
    if (period === "intraday") {
      if (intraday) {
        drawIntraday(canvas, intraday, hoverIndex);
      }
      return;
    }
    drawCandles(canvas, items, hoverIndex, maPeriods);
  };

  const loadQuote = async () => {
    try {
      const raw = await invoke<string>("fetch_quote", { code });
      const quote = JSON.parse(raw) as QuotePayload;
      titleName.textContent = quote.name || name;
      titleCode.textContent = quote.code;
      priceValue.textContent = formatNumber(quote.price);
      const rising = quote.change_pct >= 0;
      priceValue.style.color = rising ? UP_COLOR : DOWN_COLOR;
      priceDelta.textContent = `${rising ? "▲" : "▼"} ${formatNumber(Math.abs(quote.change_pct))}%`;
      priceDelta.style.color = rising ? UP_COLOR : DOWN_COLOR;
    } catch {
      titleName.textContent = name;
      titleCode.textContent = code;
    }
  };

  const load = async () => {
    try {
      if (period === "intraday") {
        const raw = await invoke<string>("fetch_intraday", { code });
        intraday = JSON.parse(raw) as IntradaySeries;
        items = [];
      } else {
        const raw = await invoke<string>("fetch_kline", { code, period, count: 120 });
        const series = JSON.parse(raw) as KlineSeries;
        items = series.items;
        intraday = null;
      }
      draw();
    } catch (error) {
      items = [];
      intraday = null;
      hint.textContent = error instanceof Error ? error.message : "加载失败";
    }
  };

  canvas.addEventListener("mousemove", (event) => {
    const rect = canvas.getBoundingClientRect();
    const isIntraday = period === "intraday";
    const padLeft = isIntraday ? 44 : 8;
    const padRight = 52;

    if (isIntraday) {
      if (!intraday || intraday.items.length === 0) {
        return;
      }
      const plotWidth = rect.width - padLeft - padRight;
      const slot = ((event.clientX - rect.left - padLeft) / plotWidth) * INTRADAY_SLOTS;

      let best = -1;
      let bestDistance = Number.POSITIVE_INFINITY;
      intraday.items.forEach((item, index) => {
        const distance = Math.abs(timeToSlot(item.time) - slot);
        if (distance < bestDistance) {
          bestDistance = distance;
          best = index;
        }
      });
      if (best < 0) {
        return;
      }

      hoverIndex = best;
      const item = intraday.items[best];
      const avg = avgPrice(item);
      const change = item.price - intraday.prev_close;
      const pct = intraday.prev_close ? (change / intraday.prev_close) * 100 : 0;
      const rising = change >= 0;
      hint.innerHTML = `${item.time.slice(0, 2)}:${item.time.slice(2, 4)}　价 <b style="color:${
        rising ? UP_COLOR : DOWN_COLOR
      }">${formatNumber(item.price)}</b>　均 ${formatNumber(avg)}　涨跌 <b style="color:${
        rising ? UP_COLOR : DOWN_COLOR
      }">${rising ? "+" : ""}${formatNumber(change)} (${rising ? "+" : ""}${formatNumber(pct)}%)</b>`;
      draw();
      return;
    }

    if (items.length === 0) {
      return;
    }
    const plotWidth = rect.width - padLeft - padRight;
    const slot = plotWidth / items.length;
    const index = Math.floor((event.clientX - rect.left - padLeft) / slot);
    if (index < 0 || index >= items.length) {
      return;
    }
    hoverIndex = index;
    const item = items[index];
    const rising = item.close >= item.open;
    hint.innerHTML = `${item.date}　开 ${formatNumber(item.open)}　高 ${formatNumber(
      item.high,
    )}　低 ${formatNumber(item.low)}　收 <b style="color:${rising ? UP_COLOR : DOWN_COLOR}">${formatNumber(
      item.close,
    )}</b>　量 ${formatVolume(item.volume)}`;
    draw();
  });

  canvas.addEventListener("mouseleave", () => {
    hoverIndex = -1;
    hint.textContent = "";
    draw();
  });

  await appWindow.listen("panel-show", (event) => {
    const payload = event.payload as { code?: string; name?: string } | null;
    if (payload?.code && payload.code !== code) {
      code = payload.code;
      name = payload.name ?? "";
      void loadQuote();
      void load();
    }
  });

  await loadQuote();
  await load();
};

/* ------------------------------------------------------------------ *
 * 设置窗口
 * ------------------------------------------------------------------ */

const bootSettings = async () => {
  const wrap = document.createElement("div");
  wrap.className = "settings-wrap";

  const title = document.createElement("div");
  title.className = "settings-title";
  title.textContent = "奶蛙设置";

  const closeButton = document.createElement("button");
  closeButton.className = "settings-close";
  closeButton.type = "button";
  closeButton.textContent = "×";
  closeButton.title = "关闭";
  closeButton.addEventListener("click", () => {
    void appWindow.hide();
  });

  const intro = document.createElement("div");
  intro.className = "settings-intro";
  intro.textContent =
    "奶蛙已经内置了一个可用的 Key，下载即用、什么都不用配。想换成自己的、或换用 DeepSeek 之类别的服务，就在这里改。";

  const status = document.createElement("div");
  status.className = "settings-status";

  const makeField = (
    label: string,
    value: string,
    placeholder: string,
    type: string,
    hint?: string,
  ) => {
    const field = document.createElement("div");
    field.className = "settings-field";

    const name = document.createElement("div");
    name.className = "settings-label";
    name.textContent = label;

    const input = document.createElement("input");
    input.className = "settings-input";
    input.type = type;
    input.value = value;
    input.placeholder = placeholder;
    input.autocomplete = "off";

    field.append(name, input);

    if (hint) {
      const tip = document.createElement("div");
      tip.className = "settings-hint";
      tip.textContent = hint;
      field.append(tip);
    }

    return { field, input };
  };

  let view: ConfigView;
  try {
    view = await invoke<ConfigView>("get_config");
  } catch {
    view = {
      has_key: false,
      key_hint: "",
      using_builtin: false,
      base_url: "",
      model: "",
      config_path: "",
      mcd_token_set: false,
      first_run: false,
    };
  }

  const keyField = makeField(
    "API Key",
    "",
    "sk-...",
    "password",
    view.using_builtin
      ? "当前用的是内置的共享 Key，额度有限，建议换成自己的。"
      : view.has_key
        ? `当前已配置（${view.key_hint}）。留空表示不修改。`
        : undefined,
  );
  const baseField = makeField(
    "Base URL",
    view.base_url,
    "https://api.minimaxi.com/v1",
    "text",
    "任何 OpenAI 兼容端点都行，DeepSeek 之类也可以填。",
  );
  const modelField = makeField("模型", view.model, "MiniMax-M3", "text");

  // 麦当劳 MCP Token。这个 Token 绑定的是**个人麦当劳账户**，
  // 所以不内置，得使用者自己填；不填就等于没开这个能力。
  const mcdField = makeField(
    "麦当劳 MCP Token",
    "",
    "19Cc8KoXV…",
    "password",
    view.mcd_token_set
      ? "已配置，奶蛙能查麦当劳。留空表示不修改。"
      : "没填就不开麦当劳功能。在 https://open.mcd.cn/mcp 用手机号登录后申请。",
  );

  const clearMcdButton = document.createElement("button");
  clearMcdButton.className = "settings-clear";
  clearMcdButton.type = "button";
  clearMcdButton.textContent = "清除麦当劳 Token";
  // 要点两下才真的清：一次误点就把 Token 弄没了，太亏。
  let clearArmed = false;
  let clearTimer = 0;
  clearMcdButton.addEventListener("click", () => {
    if (!clearArmed) {
      clearArmed = true;
      clearMcdButton.textContent = "再点一次确认清除";
      clearMcdButton.classList.add("is-armed");
      window.clearTimeout(clearTimer);
      clearTimer = window.setTimeout(() => {
        clearArmed = false;
        clearMcdButton.textContent = "清除麦当劳 Token";
        clearMcdButton.classList.remove("is-armed");
      }, 4000);
      return;
    }

    window.clearTimeout(clearTimer);
    clearArmed = false;
    clearMcdButton.textContent = "清除麦当劳 Token";
    clearMcdButton.classList.remove("is-armed");

    void (async () => {
      try {
        const result = await invoke<ConfigView>("save_config", {
          apiKey: keyField.input.value.trim(),
          baseUrl: baseField.input.value.trim(),
          model: modelField.input.value.trim(),
          mcdMcpToken: "",
        });
        status.classList.remove("is-error");
        status.classList.add("is-ok");
        status.textContent = result.mcd_token_set
          ? "已保存"
          : "已保存（麦当劳 Token 已清除，麦当劳功能关闭）";
      } catch (error) {
        status.classList.remove("is-ok");
        status.classList.add("is-error");
        status.textContent = error instanceof Error ? error.message : "保存失败";
      }
    })();
  });

  const pathBox = document.createElement("div");
  pathBox.className = "settings-path";
  pathBox.textContent = view.config_path ? `配置文件：${view.config_path}` : "";

  const saveButton = document.createElement("button");
  saveButton.className = "settings-save";
  saveButton.type = "button";
  saveButton.textContent = "保存";

  saveButton.addEventListener("click", () => {
    void (async () => {
      try {
        const mcdValue = mcdField.input.value.trim();
        const result = await invoke<ConfigView>("save_config", {
          apiKey: keyField.input.value.trim(),
          baseUrl: baseField.input.value.trim(),
          model: modelField.input.value.trim(),
          // 空 = 不修改（没动过这个框）；有值 = 覆盖。
          mcdMcpToken: mcdValue ? mcdValue : null,
        });
        status.classList.remove("is-error");
        status.classList.add("is-ok");
        status.textContent = result.has_key
          ? `已保存，Key ${result.key_hint}${result.mcd_token_set ? "，麦当劳已开启" : ""}`
          : "已保存（但还没填 API Key）";
      } catch (error) {
        status.classList.remove("is-ok");
        status.classList.add("is-error");
        status.textContent = error instanceof Error ? error.message : "保存失败";
      }
    })();
  });

  wrap.append(
    title,
    closeButton,
    intro,
    status,
    keyField.field,
    baseField.field,
    modelField.field,
    mcdField.field,
    clearMcdButton,
    pathBox,
    saveButton,
  );
  app.append(wrap);
};

/* ------------------------------------------------------------------ *
 * 桌宠窗口
 * ------------------------------------------------------------------ */

let contextMenuPromise: Promise<Menu> | null = null;
let lastContextMenuPosition = { x: 64, y: 64 };

// 右键菜单。表演性的动作不在这里——那些由奶蛙自己发起。
const createContextMenu = async () => {
  const chatItem = await MenuItem.new({
    id: "pet-menu-chat",
    text: "和奶蛙聊一聊",
    action: () => {
      void openChatWindow();
    },
  });

  const settingsItem = await MenuItem.new({
    id: "pet-menu-settings",
    text: "设置…",
    action: () => {
      void openSettingsWindow();
    },
  });

  const exitItem = await MenuItem.new({
    id: "pet-menu-exit",
    text: "退出",
    action: () => {
      void invoke("exit_app");
    },
  });

  return Menu.new({
    items: [
      chatItem,
      settingsItem,
      await PredefinedMenuItem.new({ item: "Separator" }),
      exitItem,
    ],
  });
};

const getContextMenu = () => {
  contextMenuPromise ??= createContextMenu();
  return contextMenuPromise;
};

const enableContextMenu = () => {
  app.addEventListener("contextmenu", async (event) => {
    event.preventDefault();
    lastContextMenuPosition = { x: event.screenX, y: event.screenY };

    const menu = await getContextMenu();
    await menu.popup(new LogicalPosition(event.clientX, event.clientY), appWindow);
  });
};

// 拖拽阈值（像素）。必须等指针真的移动了才开始拖窗：
// startDragging() 走 Win32 的 WM_NCLBUTTONDOWN，系统会独占鼠标，
// webview 再也收不到 mouseup，浏览器就没法合成 click —— 那样点一下就永远触发不了。
const DRAG_THRESHOLD = 4;

// 监听要挂在**当前真正显示着奶蛙的那一层**上：
// 待机时是眼球层（canvas，覆盖整个窗口），播大笑帧段时才是身体帧（img）。
// 只挂 img 的话，待机时 img 是 0×0 的空盒子，监听等于挂在空气上，拖不动。
const enableWindowDrag = (targets: HTMLElement[], onInteract: () => void) => {
  let startX = 0;
  let startY = 0;
  let dragging = false;

  const onPointerDown = (event: PointerEvent) => {
    if (event.button !== 0) {
      return;
    }

    startX = event.screenX;
    startY = event.screenY;
    dragging = false;
  };

  const onPointerMove = (event: PointerEvent) => {
    if (dragging || event.buttons === 0) {
      return;
    }

    const movedX = Math.abs(event.screenX - startX);
    const movedY = Math.abs(event.screenY - startY);
    if (movedX < DRAG_THRESHOLD && movedY < DRAG_THRESHOLD) {
      return;
    }

    dragging = true;
    onInteract();
    void appWindow.startDragging();
  };

  // 拖拽结束时系统仍会补发一次 click，这里挡掉，避免拖一下就误触发
  const onClick = () => {
    if (dragging) {
      return;
    }
    onInteract();
  };

  for (const target of targets) {
    target.addEventListener("pointerdown", onPointerDown);
    target.addEventListener("pointermove", onPointerMove);
    target.addEventListener("click", onClick);
  }
};

// 桌宠移动或改大小后，侧边窗口跟着走。
let panelSyncScheduled = false;
const schedulePanelSync = () => {
  if (panelSyncScheduled) {
    return;
  }

  panelSyncScheduled = true;
  window.setTimeout(() => {
    panelSyncScheduled = false;
    void syncSideWindows();
  }, 16);
};

const bootPet = async () => {
  const pet = document.createElement("img");
  pet.className = "pet";
  // 这张图只在播「大笑」帧段时才有内容；待机时由眼球层画出完整的奶蛙。
  // 空 src 的 img 会显示破图图标和 alt 文字，所以这里不留 alt 也不给 src。
  pet.alt = "";
  pet.draggable = false;

  const eyeCanvas = document.createElement("canvas");
  eyeCanvas.className = "pet pet-canvas";

  const hint = document.createElement("div");
  hint.className = "loading";
  hint.textContent = "Loading...";

  app.append(hint, eyeCanvas, pet);

  try {
    const manifest = await loadManifest(DEFAULT_PET_ID);
    document.documentElement.style.setProperty("--pet-aspect", `${manifest.width} / ${manifest.height}`);

    let eyeControl = { setClosed: (_closed: boolean) => {} };
    try {
      eyeControl = await startEyeIdle(manifest, eyeCanvas);
    } catch (error) {
      console.error("eye idle failed", error);
      // 眼球层画不出来时，退回用身体帧显示奶蛙，不然桌宠会整个隐身。
      pet.src = formatFramePath(manifest, 0);
      pet.classList.add("is-ready");
    }

    const { play, unlockAudio } = await createPetPlayer(manifest, pet, eyeCanvas, eyeControl);

    hint.remove();
    // 注意：这里**不能**给身体帧加 is-ready。
    // 待机状态是眼球层（canvas）画出完整的奶蛙，身体帧只在播大笑帧段时才显示。
    // 提前显示它，浏览器就会在破图位置渲染 alt 文字（"Naiwa desktop pet"）。

    // 右键也是 pointerdown，先在这里把音频解锁，菜单里的动作才带声音
    app.addEventListener("pointerdown", () => {
      void unlockAudio();
    }, { once: true });

    // 拖拽/点击要挂在两层上：待机时是眼球层，播大笑帧段时才是身体帧。
    enableWindowDrag([eyeCanvas, pet], () => {
      void petSay(PET_LINES[Math.floor(Math.random() * PET_LINES.length)]);
    });
    enableContextMenu();
    void preloadImages(manifest);

    // 托盘菜单里的「和奶蛙聊一聊」。
    await appWindow.listen("open-chat", () => {
      void openChatWindow();
    });

    // 托盘菜单里的「设置…」。
    await appWindow.listen("open-settings", () => {
      void openSettingsWindow();
    });

    // 奶蛙自己决定做个动作——真的播动画，不是文字描述。
    await appWindow.listen("pet-animate", (event) => {
      const payload = event.payload as { action?: string; reason?: string } | null;
      const action = (payload?.action ?? "laugh") as PetAction;
      void play(action);

      // 笑的动作本身就是表达，不用再多说一句。
      // 其它动作配一句它在想什么，动画加台词才完整。
      if (action !== "laugh" && action !== "big_laugh" && payload?.reason) {
        void petSay(payload.reason);
      }
    });

    // 奶蛙主动冒泡（不是用户问的）。
    await appWindow.listen("pet-spontaneous", (event) => {
      const text = String(event.payload ?? "");
      if (text) {
        void petSay(text);
      }
    });

    // 自选股涨跌提醒。
    await appWindow.listen("watchlist-alert", (event) => {
      const payload = event.payload as
        | { name?: string; direction?: string; change_pct?: number }
        | null;
      if (!payload?.name) {
        return;
      }
      const pct = formatNumber(Math.abs(payload.change_pct ?? 0));
      if (payload.direction === "up") {
        void petSay(`${payload.name} +${pct}%，嘿嘿！`);
        void play("laugh");
      } else {
        void petSay(`${payload.name} -${pct}%，有点蔫…`);
      }
    });

    // 桌宠移动/改大小后，旁边的窗口跟着走
    await appWindow.onMoved(() => {
      schedulePanelSync();
    });
    await appWindow.onResized(() => {
      schedulePanelSync();
    });
    // 拖拽结束的兜底：再对一次位置
    window.addEventListener("pointerup", () => {
      schedulePanelSync();
    });
  } catch (error) {
    hint.textContent = error instanceof Error ? error.message : "Failed to load pet";
  }
};

/* ------------------------------------------------------------------ *
 * 入口
 * ------------------------------------------------------------------ */

const boot = async () => {
  try {
    if (mode === "chat") {
      await bootChat();
      return;
    }

    if (mode === "chart") {
      await bootChart();
      return;
    }

    if (mode === "settings") {
      await bootSettings();
      return;
    }

    await bootPet();
  } catch (error) {
    const hint = document.createElement("div");
    hint.className = "loading";
    hint.textContent = error instanceof Error ? error.message : "Failed to load pet";
    app.append(hint);
  }
};

void boot();

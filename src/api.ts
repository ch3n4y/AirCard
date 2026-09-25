/**
 * 后端命令的封装。
 *
 * 只有这一处拼命令名和参数，界面里不再出现字符串形式的命令名。有两个容易踩的地方，
 * 都在这里挡住了：
 *
 * - Tauri 默认用 camelCase 传参，所以后端的参数名一律用单词（`image` 而不是
 *   `image_path`），两边写法就一致了。
 * - 返回值的字段名按 Rust 里的写法来（`has_original` 这类），与已有的 `DeviceInfo`
 *   保持同一种风格。
 *
 * 命令失败时抛出的是**已经写好的中文句子**，界面直接显示，不要再翻译一次。
 */
import { invoke } from "@tauri-apps/api/core";

export type DeviceInfo = {
  udid: string;
  name: string;
  product: string;
  version: string;
  build: string;
  language: string;
  locale: string;
  connection: string;
  bold_text: boolean | null;
};

export type AppPaths = {
  backups: string;
  artwork_cache: string;
  log_file: string;
  cards_file: string;
};

export type Card = {
  /** 卡片在设备上的名字，可能含 + / = 等字符 */
  hash: string;
  /** 原始图像已存储在这台 Mac 上 */
  has_original: boolean;
  /** 已经从 iPhone 读过卡面并留在本机，因此列表里能显示它 */
  has_artwork: boolean;
  /** 最近一次扫描里出现过 */
  seen_in_last_scan: boolean;
};

export type ScanStatus = {
  running: boolean;
  lines_read: number;
  found: string[];
  /** 扫描自己停下来时的原因（中文），正常停止时没有 */
  problem?: string;
};

export type FlashResult = {
  card: string;
  ok: boolean;
  message: string;
};

export type Leftover = {
  name: string;
  /** 是本应用生成的名称；否者只列出，不清理 */
  ours: boolean;
};

export const api = {
  devices: () => invoke<DeviceInfo[]>("list_devices"),
  paths: () => invoke<AppPaths>("app_paths"),

  cards: (udid: string) => invoke<Card[]>("cards", { udid }),
  addCard: (hash: string) => invoke<void>("add_card", { hash }),
  forgetCards: (hashes: string[]) => invoke<void>("forget_cards", { hashes }),

  thumbnail: (udid: string, hash: string) =>
    invoke<string | null>("card_thumbnail", { udid, hash }),
  forgetArtwork: (udid: string, hash: string) =>
    invoke<void>("forget_artwork", { udid, hash }),
  imagePreview: (path: string) =>
    invoke<string | null>("image_preview", { path }),

  readArtwork: (udid: string, hash: string) =>
    invoke<void>("read_artwork", { udid, hash }),
  saveOriginal: (udid: string, hash: string) =>
    invoke<void>("save_original", { udid, hash }),
  restoreOriginal: (udid: string, hash: string) =>
    invoke<void>("restore_original", { udid, hash }),

  startScan: (udid: string) => invoke<void>("start_scan", { udid }),
  scanStatus: () => invoke<ScanStatus>("scan_status"),
  stopScan: () => invoke<void>("stop_scan"),
  foldScan: (udid: string) => invoke<Card[]>("fold_scan", { udid }),
  clearScanRecord: (udid: string) =>
    invoke<void>("clear_scan_record", { udid }),

  flash: (udid: string, hashes: string[], image: string) =>
    invoke<FlashResult[]>("flash_cards", { udid, hashes, image }),

  leftovers: (udid: string) => invoke<Leftover[]>("leftovers", { udid }),
  sweepLeftovers: (udid: string, names: string[]) =>
    invoke<void>("sweep_leftovers", { udid, names }),

  logTail: () => invoke<string>("log_tail"),
  openPath: (path: string) => invoke<void>("open_path", { path }),
};

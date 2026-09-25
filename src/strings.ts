/**
 * 界面文案。
 *
 * 凡是旧版（SwiftUI，见 legacy/locales/zh-Hans.lproj/Localizable.strings）里已经
 * 有的说法，这里逐字沿用：那是这个 App 用了一整年的措辞，用的人已经认得它，换个说法
 * 反而要多想一次。旧版没有的地方另写，语气保持一致——短句、直接、不用「漏洞」之类
 * 的词。
 */
export const t = {
  app: "AirCard",
  subtitle: "替换 Apple 钱包卡面",
  guide: ["连接 iPhone", "扫描卡片", "存储原图", "刷入皮肤"],

  // 设备
  device: "设备",
  refreshDevices: "刷新设备连接",
  switchDevice: "切换设备",
  devicesConnected: (count: number) => `已连接 ${count} 台`,
  connectedTo: (name: string) => `已连接到 ${name}`,
  noDevice: "未找到 iPhone。请通过 USB 连接。",
  connectHint: "请先连接 iPhone 并信任此电脑。",
  checkingDevices: "正在检查已连接的设备…",
  deviceGone: "所选 iPhone 已断开连接。",
  model: "机型",
  system: "系统",
  connection: "连接",
  language: "语言",
  usb: "USB",
  network: "网络",
  unknown: "未知",

  // 卡片
  cards: "卡片",
  scanCards: "扫描卡片",
  stopScanning: "停止扫描",
  scanningNow: "扫描进行中",
  scanProgress: (lines: number, found: number) =>
    `已读取 ${lines} 行日志 · 发现 ${found} 张卡片`,
  scanHint: "现在请在 iPhone 上打开「钱包」，逐张滑过卡片。",
  readingFaces: "正在读取卡面",
  stopReadingFaces: "停止读取",
  faceProgress: (done: number, total: number) => `正在读取卡面 ${done}/${total}…`,
  facesFailed: (count: number) => `${count} 张卡面未能读取。请查看日志。`,
  scanRecord: "扫描记录",
  clearScanRecord: "清除扫描记录",
  removeUnseenCards: "移除上次扫描未发现的卡片",
  addManually: "手动添加",
  addManuallyHint: "粘贴一个或多个卡片哈希（用空格、逗号或换行分隔）：",
  addToList: "添加到列表",
  cancel: "取消",
  ok: "好",
  selectAll: "全选",
  deselectAll: "取消全选",
  removeFromList: "从列表中移除",
  noCards: "尚未检测到卡片",
  noCardsHint:
    "把 iPhone 用 USB 连接并解锁，点「扫描卡片」，然后逐张滑过「钱包」里的卡片。",
  copyHash: "拷贝完整哈希",
  copied: "已拷贝！",
  originalSaved: "原始图像已存储",
  originalMissing: "尚未存储原始图像",
  notSeenInLastScan: "上次扫描未发现",
  readArtwork: "从 iPhone 读取卡面",
  readArtworkHelp:
    "在列表中显示这张卡的卡面。读取时会把文件从 iPhone 移出再写回，因此大约需要一分钟。",
  saveOriginal: "存储原始图像",
  saveOriginalHelp: "存储这张卡片的原始图像，以后可随时恢复",
  restoreOriginal: "恢复原始图像",
  forgetArtwork: "不再显示卡面",
  removeAllTitle: "要从这个列表里移除全部卡片吗？",
  removeUnseenTitle: (count: number) =>
    `要从这个列表里移除 ${count} 张上次扫描未发现的卡片吗？`,
  removeHelp: "保存的原图仍留在这台 Mac 上，把卡加回来后依然可以还原。",
  selectedCount: (selected: number, total: number) =>
    `已选择 ${selected} 张 / 共 ${total} 张`,

  // 刷入
  flash: "刷入",
  chooseImage: "选择图片…",
  changeImage: "更改图片…",
  dropImage: "将图片拖到这里",
  loadingImage: "正在载入图片",
  imageReady: "图像已载入",
  pickedImage: "将要刷入的图片",
  targetCards: "目标卡片",
  flashSkins: "刷入 iPhone",
  flashingCards: "正在刷入卡片…",
  needImage: "请先选择要刷入的图片。",
  needCards: "请至少选择一张卡片。",
  flashDone:
    "所有选中的卡片都已应用新皮肤！\n\n请在 iPhone 上强制退出「钱包」App（或重新启动），即可看到新外观。",
  howToSee:
    "在 iPhone 上连按两下侧边按钮（Apple Pay），通过面容 ID 验证，然后轻点你的卡片。",
  flashWillSaveFirst: (count: number) =>
    `其中 ${count} 张尚未存储原始图像，会先自动存储原图再刷入。`,
  flashResults: "刷入结果",
  flashResultOk: "已刷入",
  flashResultFailed: "未刷入",

  // 维护
  maintenance: "维护",
  leftovers: "残留清理",
  leftoversHelp:
    "中断过的操作会在 iPhone 上留下暂存目录。这里列出它们，勾选后可以清理掉。",
  refresh: "刷新",
  sweepSelected: "清理选中",
  ourNamesOnly: "不是本应用生成的名称只列出，不会被清理。",
  noLeftovers: "没有发现残留。",
  oursTag: "本应用生成",
  foreignTag: "其他来源",
  cleanupDone: "残留已清理。",
  pathsOnThisMac: "这台 Mac 上的位置",
  backupsPath: "备份目录",
  artworkCachePath: "卡面缓存",
  cardsFilePath: "卡片列表",
  logPath: "日志",
  open: "打开",
  log: "日志",
  logEmpty: "日志还是空的。",

  // 状态
  ready: "就绪",
  selectedDevice: "当前设备",
  unknownDevice: "iPhone",
} as const;

import { useCallback, useEffect, useRef, useState } from "react";
import { App as AntApp, Button, ConfigProvider, Drawer } from "antd";
import { CreditCardOutlined, ToolOutlined } from "@ant-design/icons";
import { isTauri } from "@tauri-apps/api/core";
import zhCN from "antd/locale/zh_CN";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";

import {
  api,
  type AppPaths,
  type Card,
  type DeviceInfo,
  type FlashResult,
  type ScanStatus,
} from "./api";
import { CardsPanel } from "./components/CardsPanel";
import { DeviceBar } from "./components/DeviceBar";
import { FlashPanel } from "./components/FlashPanel";
import { MaintenancePanel } from "./components/MaintenancePanel";
import { t } from "./strings";
import type { Run } from "./ui";

const IMAGE_FILES = /\.(png|jpe?g|heic|webp|tiff|gif|bmp)$/i;

/**
 * 整个窗口。
 *
 * 三件东西放在这里，因为别的组件都要用：连着哪台 iPhone、选中了哪些卡片、以及**当前
 * 有没有操作正在跑**。最后一条不是界面上的讲究——同一台手机上不能有两个会话，否则进程
 * 直接崩，所以任何时刻只允许一个操作，界面靠这个字段把按钮关掉。
 */
export function AirCard() {
  const { message } = AntApp.useApp();
  const [paths, setPaths] = useState<AppPaths | null>(null);
  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [udid, setUdid] = useState<string | null>(null);
  const [deviceProblem, setDeviceProblem] = useState<string | null>(null);
  const [cards, setCards] = useState<Card[]>([]);
  const [selected, setSelected] = useState<string[]>([]);
  const [scan, setScan] = useState<ScanStatus | null>(null);
  const [image, setImage] = useState<{ path: string; preview: string } | null>(
    null,
  );
  const [results, setResults] = useState<FlashResult[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [faces, setFaces] = useState<{ done: number; total: number } | null>(
    null,
  );
  const [maintenanceOpen, setMaintenanceOpen] = useState(false);
  const [imageLoading, setImageLoading] = useState(false);
  const [dropping, setDropping] = useState(false);
  const operation = useRef(false);
  const facesStop = useRef(false);

  const run: Run = useCallback(
    async (label, work) => {
      if (operation.current) return;
      operation.current = true;
      setBusy(label);
      try {
        await work();
      } catch (error) {
        message.error(String(error));
      } finally {
        operation.current = false;
        setBusy(null);
      }
    },
    [message],
  );

  const refreshDevices = useCallback(async () => {
    try {
      const found = await api.devices();
      setDevices(found);
      setDeviceProblem(null);
      setUdid((current) =>
        current && found.some((item) => item.udid === current)
          ? current
          : (found[0]?.udid ?? null),
      );
    } catch (error) {
      setDevices([]);
      setUdid(null);
      setDeviceProblem(String(error));
    }
  }, []);

  const refreshCards = useCallback(async () => {
    if (!udid) {
      setCards([]);
      return;
    }
    setCards(await api.cards(udid));
  }, [udid]);

  /**
   * 把列表里还没有卡面的卡片一张张读回来。
   *
   * 扫描只能知道卡片存在；卡面要一张张从手机上读，而读一张大约一分钟。所以这件事
   * 自己开始、自带进度、也随时能停：一次失败不该挡住其余卡片。
   */
  const loadFaces = useCallback(
    async (list: Card[], device: string) => {
      const wanted = list
        .filter((card) => !card.has_artwork)
        .map((card) => card.hash);
      if (wanted.length === 0) return;
      facesStop.current = false;
      setFaces({ done: 0, total: wanted.length });
      let failed = 0;
      for (const [index, hash] of wanted.entries()) {
        if (facesStop.current) break;
        try {
          await api.readArtwork(device, hash);
        } catch {
          failed += 1;
        }
        setFaces({ done: index + 1, total: wanted.length });
        await refreshCards().catch(() => undefined);
      }
      setFaces(null);
      if (failed > 0) message.warning(t.facesFailed(failed));
    },
    [refreshCards, message],
  );

  useEffect(() => {
    void refreshDevices();
    void api
      .paths()
      .then(setPaths)
      .catch(() => setPaths(null));
  }, [refreshDevices]);

  useEffect(() => {
    setSelected([]);
    void refreshCards().catch(() => setCards([]));
  }, [refreshCards]);

  // 扫描期间每秒问一次后端读到哪儿了；扫描停下来就不再问。扫描结束后把结果并进
  // 列表——那是这次扫描唯一值得留下的东西。
  useEffect(() => {
    if (scan?.running !== true) return;
    let stopped = false;
    const timer = window.setInterval(() => {
      void api
        .scanStatus()
        .then((status) => {
          if (!stopped) setScan(status);
        })
        .catch(() => undefined);
    }, 1000);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [scan?.running]);

  useEffect(() => {
    if (scan?.running !== false || !udid || (scan.found.length ?? 0) === 0)
      return;
    void api
      .foldScan(udid)
      .then((list) => {
        setCards(list);
        // 扫描到之后就把卡面读回来，不用等人一张张点。
        return loadFaces(list, udid);
      })
      .catch(() => undefined);
  }, [scan?.running, scan?.found.length, udid, loadFaces]);

  const loadImage = useCallback(
    async (path: string) => {
      setImageLoading(true);
      try {
        const preview = await api.imagePreview(path);
        if (!preview) throw new Error(t.needImage);
        setImage({ path, preview });
        setResults(null);
      } catch (error) {
        message.error(String(error));
      } finally {
        setImageLoading(false);
      }
    },
    [message],
  );

  const pickImage = useCallback(async () => {
    const chosen = await open({
      multiple: false,
      directory: false,
      filters: [
        {
          name: "图片",
          extensions: [
            "png",
            "jpg",
            "jpeg",
            "heic",
            "webp",
            "tiff",
            "gif",
            "bmp",
          ],
        },
      ],
    });
    if (typeof chosen === "string") await loadImage(chosen);
  }, [loadImage]);

  // 拖进来的图片：原生拖放才能拿到真实路径，浏览器那套 onDrop 拿不到。
  useEffect(() => {
    if (!isTauri()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "over") {
          setDropping(true);
          return;
        }
        setDropping(false);
        if (event.payload.type !== "drop") return;
        const path = event.payload.paths.find((dropped) =>
          IMAGE_FILES.test(dropped),
        );
        if (path) void loadImage(path);
      })
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch((error) => message.error(String(error)));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [loadImage, message]);

  const startScan = () =>
    void run(t.scanCards, async () => {
      if (!udid) throw new Error(t.connectHint);
      setResults(null);
      await api.startScan(udid);
      setScan({ running: true, lines_read: 0, found: [] });
    });

  const flash = () =>
    void run(t.flashingCards, async () => {
      if (!udid) throw new Error(t.connectHint);
      if (!image) throw new Error(t.needImage);
      if (selected.length === 0) throw new Error(t.needCards);
      const outcome = await api.flash(udid, selected, image.path);
      setResults(outcome);
      await refreshCards();
    });

  const onCardsChanged = () => void refreshCards().catch(() => undefined);

  // 自动读取卡面期间，设备按钮同样要失效——同一台手机上不能有两个会话。
  const busyLabel =
    busy ?? (scan?.running ? t.scanningNow : faces ? t.readingFaces : null);

  const cardTab = (
    <CardsPanel
      udid={udid}
      cards={cards}
      scan={scan}
      selected={selected}
      selectionLocked={busy === t.flashingCards}
      busy={busyLabel}
      run={run}
      onSelectionChange={setSelected}
      onCardsChanged={onCardsChanged}
      onRefreshScanRecord={onCardsChanged}
      faces={faces}
      onStopFaces={() => {
        facesStop.current = true;
      }}
      onStartScan={startScan}
      onStopScan={() => void api.stopScan()}
    />
  );

  return (
    <div className="app-shell">
      <header className="app-header">
        <div className="brand">
          <span className="brand-mark">
            <CreditCardOutlined />
          </span>
          {t.app}
        </div>
        <div className="header-actions">
          <DeviceBar
            devices={devices}
            selected={udid}
            problem={deviceProblem}
            busy={busyLabel}
            onSelect={setUdid}
            onRefresh={() => void run(t.refreshDevices, refreshDevices)}
          />
          <Button
            className="maintenance-button"
            icon={<ToolOutlined />}
            aria-label="维护"
            title="维护"
            onClick={() => setMaintenanceOpen(true)}
          />
        </div>
      </header>
      <main className="workspace">
        <section className="cards-workspace" aria-label="卡片">
          <div className="page-heading">
            <h1>我的卡片</h1>
            <p>选择卡片，换上新卡面。</p>
          </div>
          {cardTab}
        </section>
        <aside className="composer" aria-label="刷入卡面">
          <FlashPanel
            cards={cards}
            selected={selected}
            image={image}
            results={results}
            busy={busyLabel}
            connected={udid !== null}
            imageLoading={imageLoading}
            dropping={dropping}
            onPick={() => void pickImage()}
            onFlash={flash}
          />
        </aside>
      </main>
      <Drawer
        title="维护"
        open={maintenanceOpen}
        onClose={() => setMaintenanceOpen(false)}
        size={580}
        destroyOnHidden
      >
        <MaintenancePanel
          udid={udid}
          paths={paths}
          busy={busyLabel}
          run={run}
        />
      </Drawer>
    </div>
  );
}

export default function App() {
  return (
    <ConfigProvider
      locale={zhCN}
      theme={{
        token: {
          colorPrimary: "#4378ed",
          colorText: "#202b41",
          colorTextSecondary: "#8793a6",
          colorBorder: "#e3e9f2",
          borderRadius: 10,
          fontFamily: '-apple-system, "PingFang SC", sans-serif',
          controlHeight: 36,
        },
      }}
    >
      <AntApp>
        <AirCard />
      </AntApp>
    </ConfigProvider>
  );
}

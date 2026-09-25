import { useEffect, useMemo, useState } from "react";
import {
  App as AntApp,
  Alert,
  Button,
  Checkbox,
  Dropdown,
  Empty,
  Flex,
  Input,
  Modal,
  Spin,
  Tag,
  Typography,
} from "antd";
import {
  CheckOutlined,
  CreditCardOutlined,
  EllipsisOutlined,
  PlusOutlined,
  SyncOutlined,
} from "@ant-design/icons";
import { api, type Card, type ScanStatus } from "../api";
import { t } from "../strings";
import { shortHash, type Run } from "../ui";

type Props = {
  udid: string | null;
  cards: Card[];
  scan: ScanStatus | null;
  faces: { done: number; total: number } | null;
  onStopFaces: () => void;
  selected: string[];
  selectionLocked: boolean;
  busy: string | null;
  run: Run;
  onSelectionChange: (hashes: string[]) => void;
  onCardsChanged: () => void;
  onRefreshScanRecord: () => void;
  onStartScan: () => void;
  onStopScan: () => void;
};

export function CardsPanel({
  udid,
  cards,
  scan,
  faces,
  onStopFaces,
  selected,
  selectionLocked,
  busy,
  run,
  onSelectionChange,
  onCardsChanged,
  onRefreshScanRecord,
  onStartScan,
  onStopScan,
}: Props) {
  const { message, modal } = AntApp.useApp();
  const [thumbs, setThumbs] = useState<Record<string, string>>({});
  const [adding, setAdding] = useState(false);
  const [pasted, setPasted] = useState("");
  const scanning = scan?.running === true;
  const unseen = useMemo(
    () =>
      cards.filter((card) => !card.seen_in_last_scan).map((card) => card.hash),
    [cards],
  );

  useEffect(() => setThumbs({}), [udid]);
  useEffect(() => {
    let alive = true;
    const wanted = cards.filter(
      (card) => card.has_artwork && !thumbs[card.hash],
    );
    if (!udid || wanted.length === 0) return;
    (async () => {
      const loaded: Record<string, string> = {};
      for (const card of wanted) {
        const thumbnail = await api
          .thumbnail(udid, card.hash)
          .catch(() => null);
        if (thumbnail) loaded[card.hash] = thumbnail;
      }
      if (alive && Object.keys(loaded).length > 0)
        setThumbs((current) => ({ ...current, ...loaded }));
    })();
    return () => {
      alive = false;
    };
  }, [udid, cards, thumbs]);

  const toggle = (hash: string, on: boolean) =>
    onSelectionChange(
      on ? [...selected, hash] : selected.filter((item) => item !== hash),
    );

  const addPasted = async () => {
    const hashes = pasted
      .split(/[\s,，、]+/)
      .map((item) => item.trim())
      .filter(Boolean);
    if (hashes.length === 0) return;
    await run("正在添加卡片", async () => {
      for (const hash of hashes) await api.addCard(hash);
      setPasted("");
      setAdding(false);
      onCardsChanged();
    });
  };

  const removeCards = (hashes: string[], title: string) => {
    if (hashes.length === 0) return;
    modal.confirm({
      title,
      content: t.removeHelp,
      okText: t.removeFromList,
      okButtonProps: { danger: true },
      cancelText: t.cancel,
      onOk: () =>
        run("正在更新列表", async () => {
          await api.forgetCards(hashes);
          onSelectionChange(selected.filter((hash) => !hashes.includes(hash)));
          onCardsChanged();
        }),
    });
  };

  const copy = (hash: string) =>
    void navigator.clipboard
      .writeText(hash)
      .then(() => message.success(t.copied))
      .catch(() => message.error("拷贝失败"));

  const cardMenu = (card: Card) => ({
    items: [
      { key: "copy", label: t.copyHash, onClick: () => copy(card.hash) },
      {
        key: "read",
        label: t.readArtwork,
        disabled: !udid || busy !== null,
        onClick: () =>
          void run(t.readArtwork, async () => {
            if (!udid) return;
            await api.readArtwork(udid, card.hash);
            onCardsChanged();
          }),
      },
      {
        key: "forget-art",
        label: t.forgetArtwork,
        disabled: !card.has_artwork || busy !== null,
        onClick: () =>
          void run(t.forgetArtwork, async () => {
            if (!udid) return;
            await api.forgetArtwork(udid, card.hash);
            setThumbs((current) => {
              const next = { ...current };
              delete next[card.hash];
              return next;
            });
            onCardsChanged();
          }),
      },
      { type: "divider" as const },
      {
        key: "save",
        label: t.saveOriginal,
        disabled: !udid || busy !== null,
        onClick: () =>
          void run(t.saveOriginal, async () => {
            if (!udid) return;
            await api.saveOriginal(udid, card.hash);
            onCardsChanged();
          }),
      },
      {
        key: "restore",
        label: t.restoreOriginal,
        disabled: !udid || busy !== null || !card.has_original,
        onClick: () =>
          void run(t.restoreOriginal, async () => {
            if (!udid) return;
            await api.restoreOriginal(udid, card.hash);
            onCardsChanged();
          }),
      },
      { type: "divider" as const },
      {
        key: "remove",
        label: t.removeFromList,
        danger: true,
        disabled: busy !== null,
        onClick: () => removeCards([card.hash], "要从列表中移除这张卡片吗？"),
      },
    ],
  });

  return (
    <Flex vertical gap={18}>
      <div className="cards-toolbar">
        <div className="cards-toolbar-left">
          <span className="step-dot">1</span>
          <strong>选择卡片</strong>
          <span className="muted">{cards.length} 张</span>
          {selected.length > 0 && (
            <span className="muted">· 已选 {selected.length} 张</span>
          )}
        </div>
        <div className="cards-toolbar-actions">
          {cards.length > 0 && (
            <Checkbox
              checked={selected.length === cards.length}
              indeterminate={
                selected.length > 0 && selected.length < cards.length
              }
              disabled={selectionLocked}
              onChange={(event) =>
                onSelectionChange(
                  event.target.checked ? cards.map((card) => card.hash) : [],
                )
              }
            >
              全选
            </Checkbox>
          )}
          <Dropdown
            menu={{
              items: [
                {
                  key: "clear",
                  label: t.clearScanRecord,
                  disabled: !udid || busy !== null,
                  onClick: () =>
                    void run("正在清除扫描记录", async () => {
                      if (!udid) return;
                      await api.clearScanRecord(udid);
                      onRefreshScanRecord();
                    }),
                },
                {
                  key: "unseen",
                  label: t.removeUnseenCards,
                  disabled: unseen.length === 0 || busy !== null,
                  onClick: () =>
                    removeCards(unseen, t.removeUnseenTitle(unseen.length)),
                },
                {
                  key: "all",
                  label: "移除全部卡片",
                  danger: true,
                  disabled: cards.length === 0 || busy !== null,
                  onClick: () =>
                    removeCards(
                      cards.map((card) => card.hash),
                      t.removeAllTitle,
                    ),
                },
              ],
            }}
            trigger={["click"]}
          >
            <Button icon={<EllipsisOutlined />} aria-label="列表操作" />
          </Dropdown>
          <Button
            icon={<PlusOutlined />}
            disabled={busy !== null}
            onClick={() => setAdding(true)}
          >
            {t.addManually}
          </Button>
          {scanning ? (
            <Button danger onClick={onStopScan}>
              {t.stopScanning}
            </Button>
          ) : (
            <Button
              type="primary"
              aria-label="扫描卡片"
              icon={<SyncOutlined />}
              disabled={!udid || busy !== null}
              onClick={onStartScan}
            >
              {t.scanCards}
            </Button>
          )}
        </div>
      </div>

      {scanning && (
        <Alert
          type="info"
          showIcon
          icon={<Spin size="small" />}
          title={
            t.scanProgress(scan?.lines_read ?? 0, scan?.found.length ?? 0) +
            " · " +
            t.scanHint
          }
        />
      )}
      {scan?.problem && <Alert type="warning" showIcon title={scan.problem} />}
      {faces && (
        <Alert
          type="info"
          showIcon
          icon={<Spin size="small" />}
          title={t.faceProgress(faces.done, faces.total)}
          action={
            <Button size="small" danger onClick={onStopFaces}>
              {t.stopReadingFaces}
            </Button>
          }
        />
      )}

      {cards.length === 0 ? (
        <Empty description={t.noCards}>
          <Typography.Text type="secondary">{t.noCardsHint}</Typography.Text>
        </Empty>
      ) : (
        <div className="card-grid">
          {cards.map((card, index) => {
            const chosen = selected.includes(card.hash);
            return (
              <article
                key={card.hash}
                className={"wallet-card" + (chosen ? " selected" : "")}
              >
                <button
                  type="button"
                  className="card-choice"
                  aria-pressed={chosen}
                  aria-label={"选择卡片 " + String(index + 1).padStart(2, "0")}
                  disabled={selectionLocked}
                  onClick={() => toggle(card.hash, !chosen)}
                >
                  <span className="selection-indicator">
                    {chosen && <CheckOutlined />}
                  </span>
                  {thumbs[card.hash] ? (
                    <img
                      className="face-thumb"
                      src={thumbs[card.hash]}
                      alt=""
                    />
                  ) : (
                    <span className="face-thumb face-placeholder">
                      <CreditCardOutlined />
                      <span>暂无卡面</span>
                    </span>
                  )}
                  <span className="wallet-card-info">
                    <strong>卡片 {String(index + 1).padStart(2, "0")}</strong>
                    <span className="hash-short" title={card.hash}>
                      {shortHash(card.hash, 6)}
                    </span>
                  </span>
                </button>
                <div className="wallet-card-footer">
                  <Tag color={card.has_original ? "green" : "default"}>
                    {card.has_original ? "原图已保存" : "刷入时保存原图"}
                  </Tag>
                  {!card.seen_in_last_scan && (
                    <span className="unseen-dot" title={t.notSeenInLastScan} />
                  )}
                  <Dropdown menu={cardMenu(card)} trigger={["click"]}>
                    <Button
                      type="text"
                      size="small"
                      icon={<EllipsisOutlined />}
                      aria-label={
                        "卡片 " +
                        String(index + 1).padStart(2, "0") +
                        " 的更多操作"
                      }
                    />
                  </Dropdown>
                </div>
              </article>
            );
          })}
        </div>
      )}

      <Modal
        open={adding}
        title={t.addManually}
        okText={t.addToList}
        cancelText={t.cancel}
        confirmLoading={busy !== null}
        onOk={() => void addPasted()}
        onCancel={() => setAdding(false)}
      >
        <Flex vertical gap={8}>
          <Typography.Text type="secondary">
            {t.addManuallyHint}
          </Typography.Text>
          <Input.TextArea
            rows={4}
            value={pasted}
            onChange={(event) => setPasted(event.target.value)}
            placeholder="2Do5+0cj+vG1zMfmFbPt0D4GPKQ="
          />
        </Flex>
      </Modal>
    </Flex>
  );
}

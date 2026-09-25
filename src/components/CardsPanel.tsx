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
  Space,
  Spin,
  Tag,
  Tooltip,
  Typography,
} from "antd";
import {
  ArrowDownOutlined,
  DeleteOutlined,
  DownOutlined,
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
  selected: string[];
  busy: string | null;
  run: Run;
  onSelectionChange: (hashes: string[]) => void;
  onCardsChanged: () => void;
  onRefreshScanRecord: () => void;
  onStartScan: () => void;
  onStopScan: () => void;
};

/**
 * 卡片列表，以及找卡用的扫描。
 *
 * 扫描是唯一一件需要人在手机上配合的事：iOS 只在「钱包」真的渲染某张卡时才会把它的
 * 路径写进日志，所以没有任何别的办法知道一张卡存在。
 */
export function CardsPanel({
  udid,
  cards,
  scan,
  selected,
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
    () => cards.filter((card) => !card.seen_in_last_scan).map((card) => card.hash),
    [cards],
  );

  // 卡面是从 iPhone 读来的，读一次要一分钟，所以读到的会留在本机；这里只把已经存下
  // 来的那些取回来给列表显示。
  useEffect(() => {
    let alive = true;
    const wanted = cards.filter((card) => card.has_artwork && !thumbs[card.hash]);
    if (!udid || wanted.length === 0) return;
    (async () => {
      const loaded: Record<string, string> = {};
      for (const card of wanted) {
        const thumbnail = await api.thumbnail(udid, card.hash).catch(() => null);
        if (thumbnail) loaded[card.hash] = thumbnail;
      }
      if (alive && Object.keys(loaded).length > 0) {
        setThumbs((current) => ({ ...current, ...loaded }));
      }
    })();
    return () => {
      alive = false;
    };
  }, [udid, cards, thumbs]);

  const toggle = (hash: string, on: boolean) => {
    onSelectionChange(
      on ? [...selected, hash] : selected.filter((item) => item !== hash),
    );
  };

  const addPasted = async () => {
    const hashes = pasted
      .split(/[\s,，、]+/)
      .map((item) => item.trim())
      .filter((item) => item.length > 0);
    if (hashes.length === 0) return;
    await run("正在添加卡片", async () => {
      for (const hash of hashes) {
        await api.addCard(hash);
      }
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

  return (
    <Flex vertical gap={12}>
      <Flex gap={8} wrap align="center">
        {scanning ? (
          <Button danger onClick={onStopScan}>
            {t.stopScanning}
          </Button>
        ) : (
          <Button
            type="primary"
            icon={<SyncOutlined />}
            disabled={!udid || busy !== null}
            onClick={onStartScan}
          >
            {t.scanCards}
          </Button>
        )}
        <Dropdown
          menu={{
            items: [
              {
                key: "clear",
                label: t.clearScanRecord,
                disabled: !udid || busy !== null,
                onClick: () =>
                  run("正在清除扫描记录", async () => {
                    if (!udid) return;
                    await api.clearScanRecord(udid);
                    onRefreshScanRecord();
                  }),
              },
              {
                key: "unseen",
                label: t.removeUnseenCards,
                disabled: unseen.length === 0 || busy !== null,
                onClick: () => removeCards(unseen, t.removeUnseenTitle(unseen.length)),
              },
            ],
          }}
          trigger={["click"]}
        >
          <Button>
            {t.scanRecord} <DownOutlined />
          </Button>
        </Dropdown>
        <Button
          icon={<PlusOutlined />}
          disabled={busy !== null}
          onClick={() => setAdding(true)}
        >
          {t.addManually}
        </Button>
        <Button
          disabled={busy !== null || cards.length === 0}
          onClick={() =>
            onSelectionChange(
              selected.length === cards.length ? [] : cards.map((card) => card.hash),
            )
          }
        >
          {selected.length === cards.length && cards.length > 0
            ? t.deselectAll
            : t.selectAll}
        </Button>
        <Button
          icon={<DeleteOutlined />}
          danger
          disabled={busy !== null || cards.length === 0}
          onClick={() => removeCards(cards.map((card) => card.hash), t.removeAllTitle)}
        >
          {t.removeFromList}
        </Button>
        {cards.length > 0 && (
          <Typography.Text type="secondary">
            {t.selectedCount(selected.length, cards.length)}
          </Typography.Text>
        )}
      </Flex>

      {scanning && (
        <Alert
          type="info"
          showIcon
          icon={<Spin size="small" />}
          message={t.scanningNow}
          description={
            <Flex vertical gap={4}>
              <span>{t.scanProgress(scan?.lines_read ?? 0, scan?.found.length ?? 0)}</span>
              <Typography.Text type="secondary">{t.scanHint}</Typography.Text>
              {(scan?.found.length ?? 0) > 0 && (
                <Flex gap={4} wrap>
                  {scan?.found.map((hash) => (
                    <Tag key={hash} className="hash-short">
                      {shortHash(hash)}
                    </Tag>
                  ))}
                </Flex>
              )}
            </Flex>
          }
        />
      )}

      {scan?.problem && <Alert type="warning" showIcon message={scan.problem} />}

      {cards.length === 0 ? (
        <Empty description={t.noCards}>
          <Typography.Text type="secondary">{t.noCardsHint}</Typography.Text>
        </Empty>
      ) : (
        <Flex vertical>
          {cards.map((card) => (
            <Flex
              key={card.hash}
              align="center"
              gap={12}
              style={{
                padding: "12px 4px",
                borderBottom: "1px solid #f0f0f0",
                opacity: card.seen_in_last_scan ? 1 : 0.55,
              }}
            >
              <Checkbox
                checked={selected.includes(card.hash)}
                disabled={busy !== null}
                onChange={(event) => toggle(card.hash, event.target.checked)}
              />
              {thumbs[card.hash] ? (
                <img className="face-thumb" src={thumbs[card.hash]} alt="" />
              ) : (
                <div className="face-thumb" />
              )}

              <Flex vertical gap={4} style={{ flex: 1, minWidth: 0 }}>
                <Space size={8} wrap>
                  <Typography.Text className="hash-short">
                    {shortHash(card.hash, 14)}
                  </Typography.Text>
                  <Tooltip title={t.copyHash}>
                    <Button
                      size="small"
                      type="link"
                      onClick={() =>
                        void navigator.clipboard
                          .writeText(card.hash)
                          .then(() => message.success(t.copied))
                      }
                    >
                      {t.copyHash}
                    </Button>
                  </Tooltip>
                  {card.has_original ? (
                    <Tag color="green">{t.originalSaved}</Tag>
                  ) : (
                    <Tag color="orange">{t.originalMissing}</Tag>
                  )}
                  {!card.seen_in_last_scan && <Tag>{t.notSeenInLastScan}</Tag>}
                </Space>
                <Typography.Text type="secondary" className="hash">
                  {card.hash}
                </Typography.Text>
              </Flex>

              <Space size={4} wrap>
                <Tooltip title={t.readArtworkHelp}>
                  <Button
                    size="small"
                    disabled={!udid || busy !== null}
                    onClick={() =>
                      run(t.readArtwork, async () => {
                        if (!udid) return;
                        await api.readArtwork(udid, card.hash);
                        onCardsChanged();
                      })
                    }
                  >
                    {t.readArtwork}
                  </Button>
                </Tooltip>
                {card.has_artwork && (
                  <Button
                    size="small"
                    disabled={busy !== null}
                    onClick={() =>
                      run(t.forgetArtwork, async () => {
                        if (!udid) return;
                        await api.forgetArtwork(udid, card.hash);
                        setThumbs((current) => {
                          const { [card.hash]: _gone, ...rest } = current;
                          return rest;
                        });
                        onCardsChanged();
                      })
                    }
                  >
                    {t.forgetArtwork}
                  </Button>
                )}
                <Tooltip title={t.saveOriginalHelp}>
                  <Button
                    size="small"
                    disabled={!udid || busy !== null}
                    onClick={() =>
                      run(t.saveOriginal, async () => {
                        if (!udid) return;
                        await api.saveOriginal(udid, card.hash);
                        onCardsChanged();
                      })
                    }
                  >
                    {t.saveOriginal}
                  </Button>
                </Tooltip>
                <Button
                  size="small"
                  icon={<ArrowDownOutlined />}
                  disabled={!udid || busy !== null || !card.has_original}
                  onClick={() =>
                    run(t.restoreOriginal, async () => {
                      if (!udid) return;
                      await api.restoreOriginal(udid, card.hash);
                      onCardsChanged();
                    })
                  }
                >
                  {t.restoreOriginal}
                </Button>
              </Space>
            </Flex>
          ))}
        </Flex>
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
          <Typography.Text type="secondary">{t.addManuallyHint}</Typography.Text>
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

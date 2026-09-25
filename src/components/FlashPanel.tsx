import { Alert, Button, Card, Empty, Flex, Tag, Typography } from "antd";
import { PictureOutlined, ThunderboltOutlined } from "@ant-design/icons";

import type { Card as CardInfo, FlashResult } from "../api";
import { t } from "../strings";
import { shortHash } from "../ui";

type Props = {
  cards: CardInfo[];
  selected: string[];
  image: { path: string; preview: string } | null;
  results: FlashResult[] | null;
  busy: string | null;
  onPick: () => void;
  onFlash: () => void;
};

/**
 * 把一张图片刷到选中的卡片上。
 *
 * 刷入是唯一一件手机上也留不下退路的事，所以这里要说清两件：这张图会去哪里，以及
 * 哪些卡片还没有原图（后端会先自动存一份再刷）。
 */
export function FlashPanel({
  cards,
  selected,
  image,
  results,
  busy,
  onPick,
  onFlash,
}: Props) {
  const targets = cards.filter((card) => selected.includes(card.hash));
  const unsaved = targets.filter((card) => !card.has_original).length;
  const ready = image !== null && targets.length > 0 && busy === null;

  return (
    <Flex vertical gap={12}>
      <Card
        size="small"
        title={t.pickedImage}
        extra={
          image && (
            <Button size="small" disabled={busy !== null} onClick={onPick}>
              {t.changeImage}
            </Button>
          )
        }
      >
        {image ? (
          <Flex gap={12} align="flex-start">
            <img
              src={image.preview}
              alt=""
              style={{
                width: 240,
                borderRadius: 6,
                border: "1px solid #e5e5e5",
                background: "#f0f0f0",
              }}
            />
            <Flex vertical gap={4} style={{ minWidth: 0 }}>
              <Tag color="green">{t.imageReady}</Tag>
              <Typography.Text type="secondary" className="hash">
                {image.path}
              </Typography.Text>
            </Flex>
          </Flex>
        ) : (
          <Flex vertical gap={12}>
            <div className="drop-target">
              <PictureOutlined style={{ fontSize: 24 }} />
              <div style={{ marginTop: 8 }}>{t.dropImage}</div>
            </div>
            <Button
              type="primary"
              icon={<PictureOutlined />}
              disabled={busy !== null}
              onClick={onPick}
            >
              {t.chooseImage}
            </Button>
          </Flex>
        )}
      </Card>

      <Card size="small" title={t.targetCards}>
        {targets.length === 0 ? (
          <Empty description={t.needCards} image={Empty.PRESENTED_IMAGE_SIMPLE} />
        ) : (
          <Flex vertical gap={6}>
            <Typography.Text>
              {t.selectedCount(targets.length, cards.length)}
            </Typography.Text>
            <Flex gap={4} wrap>
              {targets.map((card) => (
                <Tag key={card.hash} className="hash-short">
                  {shortHash(card.hash, 10)}
                </Tag>
              ))}
            </Flex>
            {unsaved > 0 && (
              <Alert
                type="warning"
                showIcon
                message={t.flashWillSaveFirst(unsaved)}
              />
            )}
          </Flex>
        )}
      </Card>

      <Flex gap={12} align="center" wrap>
        <Button
          type="primary"
          size="large"
          icon={<ThunderboltOutlined />}
          disabled={!ready}
          onClick={onFlash}
        >
          {t.flashSkins}
        </Button>
        {busy && <Typography.Text type="secondary">{busy}…</Typography.Text>}
        {!image && <Typography.Text type="secondary">{t.needImage}</Typography.Text>}
        {image && targets.length === 0 && (
          <Typography.Text type="secondary">{t.needCards}</Typography.Text>
        )}
      </Flex>

      {results && (
        <Card size="small" title={t.flashResults}>
          <Flex vertical gap={8}>
            {results.map((result) => (
              <Flex key={result.card} gap={8} align="center" wrap>
                <Tag color={result.ok ? "green" : "red"} className="hash-short">
                  {shortHash(result.card, 10)}
                </Tag>
                <Typography.Text type={result.ok ? undefined : "danger"}>
                  {result.ok ? t.flashResultOk : t.flashResultFailed}
                </Typography.Text>
                <Typography.Text type="secondary">{result.message}</Typography.Text>
              </Flex>
            ))}
            {results.length > 0 && results.every((result) => result.ok) && (
              <Alert
                type="success"
                showIcon
                message={<span style={{ whiteSpace: "pre-line" }}>{t.flashDone}</span>}
                description={t.howToSee}
              />
            )}
            {results.some((result) => !result.ok) && (
              <Alert type="error" showIcon message={t.howToSee} />
            )}
          </Flex>
        </Card>
      )}
    </Flex>
  );
}

import { Alert, Button, Spin } from "antd";
import {
  CheckCircleFilled,
  PictureOutlined,
  ArrowRightOutlined,
  SafetyCertificateOutlined,
} from "@ant-design/icons";
import type { Card, FlashResult } from "../api";
import { t } from "../strings";
import { shortHash } from "../ui";

type Props = {
  cards: Card[];
  selected: string[];
  image: { path: string; preview: string } | null;
  results: FlashResult[] | null;
  busy: string | null;
  connected: boolean;
  imageLoading: boolean;
  dropping: boolean;
  onPick: () => void;
  onFlash: () => void;
};

/** The image, target summary and action stay visible beside the selectable cards. */
export function FlashPanel({
  cards,
  selected,
  image,
  results,
  busy,
  connected,
  imageLoading,
  dropping,
  onPick,
  onFlash,
}: Props) {
  const targets = cards.filter((card) => selected.includes(card.hash));
  const ready =
    connected && image !== null && targets.length > 0 && !busy && !imageLoading;
  const flashing = busy === t.flashingCards;
  const hint = !connected
    ? "请先连接 iPhone"
    : !targets.length
      ? "点击左侧卡片，选择刷入目标"
      : !image
        ? "选择一张新卡面"
        : busy
          ? busy
          : "已准备好";
  const succeeded = results?.filter((result) => result.ok).length ?? 0;

  return (
    <div className="flash-composer">
      <div className="composer-heading">
        <span className="step-dot">2</span>
        <h2>新卡面</h2>
      </div>
      <button
        type="button"
        className={
          "image-picker" +
          (image ? " has-image" : "") +
          (dropping ? " over" : "")
        }
        onClick={onPick}
        disabled={imageLoading || flashing}
        aria-label={image ? "更换图片" : "选择图片"}
      >
        {image ? (
          <img src={image.preview} alt="待刷入的卡面" />
        ) : (
          <span className="image-picker-empty">
            <span className="image-picker-icon">
              <PictureOutlined />
            </span>
            <strong>选择图片</strong>
            <span>或拖放到窗口</span>
          </span>
        )}
        {image && <span className="change-image">更换图片</span>}
        {imageLoading && (
          <span className="image-loading">
            <Spin />
          </span>
        )}
      </button>
      {image && (
        <div className="image-filename" title={image.path}>
          {image.path.split(/[\\/]/).pop()}
        </div>
      )}
      <div className="target-summary">
        <span>刷入目标</span>
        <strong data-testid="selected-count">{targets.length} 张卡片</strong>
      </div>
      <div className="target-chips" aria-label="已选目标">
        {targets.length ? (
          targets.map((card) => (
            <span key={card.hash} title={card.hash}>
              <CheckCircleFilled />
              卡片 {String(cards.indexOf(card) + 1).padStart(2, "0")}
            </span>
          ))
        ) : (
          <span className="target-empty">尚未选择</span>
        )}
      </div>
      <div className="backup-note">
        <SafetyCertificateOutlined />
        <span>刷入前自动保存原图，可随时恢复。</span>
      </div>
      <Button
        type="primary"
        size="large"
        block
        className="flash-button"
        icon={<ArrowRightOutlined />}
        aria-label="刷入 iPhone"
        loading={flashing}
        disabled={!ready}
        onClick={onFlash}
      >
        {flashing ? "正在刷入" : "刷入 iPhone"}
      </Button>
      <div className="composer-hint" role="status">
        {hint}
      </div>
      {results && (
        <div className="flash-results" aria-live="polite">
          <Alert
            showIcon
            type={succeeded === results.length ? "success" : "error"}
            title={
              succeeded === results.length ? "刷入完成" : "部分卡片未能刷入"
            }
            description={
              succeeded === results.length
                ? "重新打开「钱包」查看新卡面。"
                : results
                    .filter((result) => !result.ok)
                    .map(
                      (result) =>
                        shortHash(result.card, 4) + "：" + result.message,
                    )
                    .join("\n")
            }
          />
        </div>
      )}
    </div>
  );
}
